use crate::{
    Scope, SyntaxSet,
    syntax::{Capture, Pattern, ScopeTemplate, SyntaxDefinition},
};
use fancy_regex::{Regex, RegexBuilder, RegexInput};
use std::collections::{HashMap, HashSet, VecDeque};

/// Operations emitted by the [`Tokenizer`].
///
/// These variants describe the stream of parsing events. Consumers should interpret these
/// to construct the final list of tokens or syntax-highlighted regions.
///
/// # Handling State
/// The [`Tokenizer`] does not maintain the "scope stack" (the hierarchy of scopes like
/// `source.rust` -> `meta.function`). Instead, it emits [`Push`](TokenizerOp::Push) and
/// [`Pop`](TokenizerOp::Pop) operations. The consumer is responsible for maintaining a
/// `Vec<Scope>` if they need to know the full context of a [`Content`](TokenizerOp::Content) token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenizerOp<'a> {
    /// Indicates the start of a new scope.
    ///
    /// The consumer should push this [`Scope`] onto their active stack.
    Push(Scope),

    /// Indicates the end of the most recently pushed scope.
    ///
    /// The consumer should pop the top element from their active stack.
    Pop,

    /// A chunk of text content.
    ///
    /// This text belongs to the context defined by the current state of the consumer's
    /// scope stack (after processing all preceding `Push`/`Pop` operations).
    Content(&'a str),

    /// An explicit newline character.
    ///
    /// This is emitted separately to ensure that regex anchors (like `^` and `$`)
    /// are handled correctly by the parser. It should be treated as a newline in the output.
    Newline,
}

/// Internal state for a nested block (e.g., inside a string or comment).
struct StackFrame<'a> {
    /// The regex source that will close this block (already back-reference substituted).
    ///
    /// It is kept as a string and recompiled per scan because matching depends on the
    /// `\A`/`\G` anchor state at the current position. The root frame has no end regex
    /// and is never popped.
    end_regex: Option<String>,

    /// Captures for the `end` regex, mapping a regex group index to the scope (template)
    /// it should be assigned.
    end_captures: Option<&'a HashMap<usize, Capture>>,

    /// For a `begin`/`while` block: the regex (back-reference substituted) tested at the
    /// start of each line to decide whether the block continues.
    while_regex: Option<String>,

    /// Captures for the `while` regex.
    while_captures: Option<&'a HashMap<usize, Capture>>,

    /// The patterns that are valid inside this block.
    patterns: &'a [Pattern],

    /// The grammar that owns [`patterns`](Self::patterns). Includes referenced from
    /// these patterns resolve against this grammar's repository. This is what makes
    /// embedded grammars work: a block opened by a rule from grammar `B` continues to
    /// resolve `B`'s rules even while nested inside grammar `A`.
    syntax: &'a SyntaxDefinition,

    /// The scope this block pushed on entry (because the rule had a `name`), if any. It
    /// is part of the active scope stack while the block is open and must be balanced by
    /// a [`TokenizerOp::Pop`] when it closes.
    scope: Option<Scope>,

    /// The cursor position at which this block was entered, and the rule that opened it.
    /// Used to stop a zero-width `begin` from re-entering itself forever at one position.
    enter_pos: usize,
    enter_rule: *const Pattern,
}

/// A resolved capture span over `[start, end)`.
///
/// `scope` (if any) wraps the range in a `Push`/`Pop` pair; `sub` (if any) further
/// tokenizes the range with a nested pattern list (TextMate captures-with-`patterns`).
struct Span<'a> {
    start: usize,
    end: usize,
    scope: Option<Scope>,
    sub: Option<(&'a [Pattern], &'a SyntaxDefinition)>,
}

/// The result of running a regex: absolute byte offsets of the whole match and each group.
struct MatchData {
    start: usize,
    end: usize,
    /// One entry per capture group (index 0 is the whole match); `None` if the group
    /// did not participate in the match.
    groups: Vec<Option<(usize, usize)>>,
}

/// A line-based iterator that parses text according to a [`SyntaxDefinition`].
///
/// This struct manages the internal parsing state (the "grammar stack") but delegates
/// the management of the "scope stack" to the consumer via [`TokenizerOp`]s.
///
/// # Cross-grammar includes
/// When constructed with [`Tokenizer::new_in_set`] (or via [`SyntaxSet::tokenizer`]),
/// includes that reference other grammars (e.g. `include: source.json` or
/// `include: source.json#value`) are resolved against the provided [`SyntaxSet`]. This
/// is how embedded languages (CSS in HTML, code blocks in Markdown, ...) are handled.
/// When constructed with the standalone [`Tokenizer::new`], such includes are skipped.
///
/// Supports `begin`/`end` and `begin`/`while` blocks, `\A`/`\G` anchoring, capture
/// scopes with `$n` interpolation and nested `patterns`, and (via the [`SyntaxSet`])
/// injection grammars selected by scope.
pub struct Tokenizer<'a> {
    text: &'a str,
    /// Byte offset of the start of the line currently being scanned.
    line_start: usize,
    /// Byte offset of the next character to consume.
    cursor: usize,

    /// The stack of grammar rules currently being processed. The first frame is the
    /// root (the base grammar's top-level patterns) and is never popped.
    ///
    /// *Note:* This tracks the internal parsing state (which rules are valid),
    /// not the semantic scope stack used for highlighting.
    stack: Vec<StackFrame<'a>>,

    /// The base grammar tokenization started in. `$base` resolves against this.
    base: &'a SyntaxDefinition,

    /// The registry used to resolve cross-grammar includes, if any.
    set: Option<&'a SyntaxSet>,

    /// Operations produced but not yet handed to the consumer.
    pending: VecDeque<TokenizerOp<'a>>,

    /// Cache of compiled regexes, keyed by their source string.
    regex_cache: HashMap<String, Regex>,

    /// Number of consecutive scans at the current cursor that have not advanced it.
    /// Used to break pathological zero-width `begin`/`end` oscillations.
    stall_count: u32,

    /// The position the `\G` anchor matches at: the end of the most recent `begin` match
    /// on the current line. [`NO_ANCHOR`] (and reset at every newline) when there is no
    /// active anchor, so `\G` cannot match at an arbitrary position.
    anchor: usize,

    /// The scopes currently active at the cursor (the base grammar scope plus the `name`
    /// of every open begin/end block). Maintained as strings in parallel so injection
    /// selectors can be matched without re-rendering scopes each scan.
    active_scopes: Vec<String>,

    /// Set once the end of input has been processed (and trailing scopes popped).
    finished: bool,
}

/// Sentinel for "no `\G` anchor active" (no real cursor reaches `usize::MAX`).
const NO_ANCHOR: usize = usize::MAX;

/// Maximum number of zero-width (cursor-not-advancing) scans tolerated at a single
/// position before a character is force-consumed. Legitimate zero-width pushes (e.g.
/// lookahead-based embedding) nest only a handful deep, so this is generous headroom
/// while still bounding infinite loops.
const MAX_STALL: u32 = 64;

impl<'a> Tokenizer<'a> {
    /// Creates a standalone tokenizer for the given text and syntax definition.
    ///
    /// Cross-grammar includes cannot be resolved; use [`Tokenizer::new_in_set`] for that.
    pub fn new(text: &'a str, syntax: &'a SyntaxDefinition) -> Self {
        Self::build(text, syntax, None)
    }

    /// Creates a tokenizer that resolves cross-grammar includes against `set`.
    ///
    /// `syntax` is the base grammar to start tokenizing in (it should also be a member
    /// of `set`).
    pub fn new_in_set(text: &'a str, syntax: &'a SyntaxDefinition, set: &'a SyntaxSet) -> Self {
        Self::build(text, syntax, Some(set))
    }

    fn build(text: &'a str, syntax: &'a SyntaxDefinition, set: Option<&'a SyntaxSet>) -> Self {
        Self::build_with_patterns(text, &syntax.patterns, syntax, set)
    }

    /// Builds a tokenizer whose root frame uses an arbitrary pattern list (rather than
    /// `syntax.patterns`). Used to recursively tokenize a capture's `patterns`.
    fn build_with_patterns(
        text: &'a str,
        patterns: &'a [Pattern],
        syntax: &'a SyntaxDefinition,
        set: Option<&'a SyntaxSet>,
    ) -> Self {
        let root = StackFrame {
            end_regex: None,
            end_captures: None,
            while_regex: None,
            while_captures: None,
            patterns,
            syntax,
            scope: None,
            enter_pos: 0,
            enter_rule: std::ptr::null(),
        };
        Self {
            text,
            line_start: 0,
            cursor: 0,
            stack: vec![root],
            base: syntax,
            set,
            pending: VecDeque::new(),
            regex_cache: HashMap::new(),
            stall_count: 0,
            anchor: NO_ANCHOR,
            // The base grammar scope is always active at the bottom of the stack.
            active_scopes: vec![syntax.scope.to_string()],
            finished: false,
        }
    }

    /// Compiles `pattern` (caching the result) and returns it, or `None` if it fails
    /// to compile.
    fn compiled(&mut self, pattern: &str) -> Option<&Regex> {
        use std::collections::hash_map::Entry;
        match self.regex_cache.entry(pattern.to_string()) {
            Entry::Occupied(e) => Some(e.into_mut()),
            Entry::Vacant(v) => match compile_regex(pattern) {
                Ok(re) => Some(v.insert(re)),
                Err(_) => None,
            },
        }
    }

    /// Runs a (cached) regex against `line` starting at `pos`, returning absolute-offset
    /// match data. `line_start` is the absolute offset of `line`'s first byte.
    ///
    /// `allow_a`/`allow_g` control whether the `\A` (document start) and `\G` (anchor)
    /// assertions are permitted to match at this position.
    ///
    /// `\A` is neutered to a never-matching assertion when disallowed (there is no
    /// runtime override that suppresses `\A` without also suppressing `^`, which must
    /// keep matching at the start of every line slice). `\G` is handled by
    /// [`RegexInput::continue_from_previous_match_end`]: the engine matches `\G` at the
    /// search start by default, and we suppress it when the cursor is not on the anchor.
    fn run_regex(
        &mut self,
        pattern: &str,
        line: &'a str,
        pos: usize,
        line_start: usize,
        allow_a: bool,
        allow_g: bool,
    ) -> Option<MatchData> {
        let pattern = neuter_doc_start(pattern, allow_a);
        let re = self.compiled(&pattern)?;
        let mut input = RegexInput::new(line).from_pos(pos);
        if !allow_g {
            input = input.continue_from_previous_match_end(false);
        }
        let caps = re.captures_input(input).ok()??;
        Some(match_data(&caps, line_start))
    }

    /// Closes the top begin/end block against end-match `e`: emits any text up to the
    /// match, the end-capture spans, advances the cursor past the match, and pops the
    /// frame (and its scope, if any).
    fn close_top_block(&mut self, text: &'a str, e: &MatchData) {
        if e.start > self.cursor {
            self.pending
                .push_back(TokenizerOp::Content(&text[self.cursor..e.start]));
        }
        let frame = self.stack.last().unwrap();
        let spans = match frame.end_captures {
            Some(c) => capture_spans(c, e, text, frame.syntax),
            None => Vec::new(),
        };
        let has_scope = frame.scope.is_some();
        self.emit_spans(text, e.start, e.end, spans);
        self.cursor = e.end;
        self.stack.pop();
        if has_scope {
            self.active_scopes.pop();
            self.pending.push_back(TokenizerOp::Pop);
        }
    }

    /// At the end of a line, closes any open begin/end blocks whose `end` pattern matches
    /// (zero-width) at the end-of-line position before the newline is consumed. TextMate
    /// tokenizes the trailing newline as part of the line, so an `end` anchored at
    /// end-of-line (`$`, `(?=\n)`, …) must close its block here; otherwise blocks with
    /// `end: $` would leak into the following line. Returns `true` if a block was closed.
    fn close_ends_at_line_end(&mut self, line_start: usize, line_end: usize) -> bool {
        let text = self.text;
        let mut closed = false;
        // A `\G` anchor may still be live exactly at the cursor; `\A` only on line one.
        let allow_a = line_start == 0;
        // Two views of the line: one ending at the newline (so `$`/`(?=$)` ends match at
        // the line end) and one including the newline (so `(?=\n)`/`(?=[\n#])`-style ends
        // can see it). Many grammars' single-line blocks rely on the latter form.
        let no_nl = &text[line_start..line_end];
        let with_nl = if line_end < text.len() {
            &text[line_start..line_end + 1]
        } else {
            no_nl
        };
        while self.stack.len() > 1 {
            let allow_g = self.cursor == self.anchor;
            let Some(re) = self.stack.last().unwrap().end_regex.clone() else {
                break;
            };
            let pos = self.cursor - line_start;
            // Close when the end begins exactly at the line end: either a zero-width
            // anchored/lookahead end (`$`, `(?=\n)`) or one that consumes just the
            // trailing newline (`end: \n`). A non-zero-width end that consumes earlier
            // text would already have been handled by the in-line scan.
            let closes_here = |md: &MatchData| md.start == line_end && md.end <= line_end + 1;
            let e = self
                .run_regex(&re, no_nl, pos, line_start, allow_a, allow_g)
                .filter(closes_here)
                .or_else(|| {
                    self.run_regex(&re, with_nl, pos, line_start, allow_a, allow_g)
                        .filter(closes_here)
                });
            match e {
                Some(mut e) => {
                    // Never let the end swallow the newline itself: the trailing `\n` is
                    // emitted separately as a `Newline` op and tracked for line math, so
                    // clamp the match to a zero-width close at the line end.
                    e.end = line_end;
                    e.groups.truncate(1);
                    e.groups[0] = Some((line_end, line_end));
                    self.close_top_block(text, &e);
                    closed = true;
                }
                None => break,
            }
        }
        closed
    }

    /// Produces the next batch of operations for a single parsing event, appending them
    /// to `self.pending`. May produce nothing only when the input is exhausted.
    fn advance(&mut self) {
        let text = self.text;

        // End of input: close any open blocks and finish.
        if self.cursor >= text.len() {
            while self.stack.len() > 1 {
                let frame = self.stack.pop().unwrap();
                if frame.scope.is_some() {
                    self.active_scopes.pop();
                    self.pending.push_back(TokenizerOp::Pop);
                }
            }
            self.finished = true;
            return;
        }

        let line_end = text[self.line_start..]
            .find('\n')
            .map(|i| self.line_start + i)
            .unwrap_or(text.len());

        // End of the current line: first let any `end: $`-style blocks close at the
        // end-of-line position, then emit a newline and move to the next line. The `\G`
        // anchor does not carry across line boundaries.
        if self.cursor == line_end {
            if self.close_ends_at_line_end(self.line_start, line_end) {
                return;
            }
            // Then let a zero-width `begin` fire at the end-of-line position. An embedding
            // begin such as `(?<=>)` (HTML/Vue `<script>`/`<style>`/`<template>` bodies)
            // only matches here, where its lookbehind can still see the line's final `>`;
            // at the start of the next line the slice no longer includes that character,
            // so the embedded block would never open. TextMate scans this trailing
            // position too, opening the block before the newline.
            let line = &text[self.line_start..line_end];
            if self.try_open_eol_begin(line, self.line_start, line_end) {
                return;
            }
            self.pending.push_back(TokenizerOp::Newline);
            self.cursor = line_end + 1;
            self.line_start = self.cursor;
            self.anchor = NO_ANCHOR;
            return;
        }

        let line = &text[self.line_start..line_end];
        let line_start = self.line_start;

        // At the start of a line, evaluate `begin`/`while` continuation conditions: each
        // open while-block either consumes its `while` marker or is closed.
        if self.cursor == line_start && self.process_while_conditions(line, line_start) {
            return;
        }

        // Scan the remainder of the current line for the next match.
        let pos = self.cursor - line_start;

        // `\A` may match only on the first line; `\G` only at the current anchor.
        let allow_a = line_start == 0;
        let allow_g = self.cursor == self.anchor;

        // Candidate 1: the end of the current block.
        let end_str = self.stack.last().unwrap().end_regex.clone();
        let end_md = match &end_str {
            Some(re) => self.run_regex(re, line, pos, line_start, allow_a, allow_g),
            None => None,
        };

        // Candidate 2: the earliest-matching inner pattern, resolved (with embedded
        // grammars) against whichever grammar owns the current frame. Base patterns have
        // priority 0; injected patterns carry the priority of their selector (`L:` > 0
        // wins ties against base, `R:` < 0 loses).
        let candidates = self.gather_candidates();

        let mut best: Option<(MatchData, &'a Pattern, &'a SyntaxDefinition, i8)> = None;
        for (pat, syn, priority) in candidates {
            let is_begin = matches!(pat, Pattern::BeginEnd { .. } | Pattern::BeginWhile { .. });
            let md = match pat {
                Pattern::Match { regex, .. } => {
                    self.run_regex(regex, line, pos, line_start, allow_a, allow_g)
                }
                Pattern::BeginEnd { begin, .. } | Pattern::BeginWhile { begin, .. } => {
                    self.run_regex(begin, line, pos, line_start, allow_a, allow_g)
                }
                _ => None,
            };
            if let Some(md) = md {
                // Don't let a zero-width `begin` re-open the same block at the same
                // position over and over (e.g. a `(?=\S)` paragraph rule).
                if is_begin && md.end == md.start && self.would_reenter(pat, md.start) {
                    continue;
                }
                let better = best.as_ref().is_none_or(|(b, _, _, bp)| {
                    md.start < b.start || (md.start == b.start && priority > *bp)
                });
                if better {
                    best = Some((md, pat, syn, priority));
                }
            }
        }

        // The end pattern wins ties (TextMate's default `applyEndPatternLast = false`).
        let use_end = match (&end_md, &best) {
            (Some(e), Some((m, ..))) => e.start <= m.start,
            (Some(_), None) => true,
            _ => false,
        };

        let scan_cursor = self.cursor;
        let scan_depth = self.stack.len();

        if use_end {
            let e = end_md.unwrap();
            self.close_top_block(text, &e);
        } else if let Some((md, pat, syn, _priority)) = best {
            match pat {
                Pattern::Match {
                    scope, captures, ..
                } => {
                    if md.start > self.cursor {
                        self.pending
                            .push_back(TokenizerOp::Content(&text[self.cursor..md.start]));
                    }
                    // The whole-match `name` encloses the capture scopes, so it goes
                    // first: `emit_spans`' stable sort keeps it outer when it shares a
                    // range with a capture (e.g. a single-token match that is also its
                    // own group 0).
                    let mut spans = Vec::new();
                    if let Some(scope) = scope.as_ref().and_then(|t| resolve_scope(t, &md, text)) {
                        spans.push(Span {
                            start: md.start,
                            end: md.end,
                            scope: Some(scope),
                            sub: None,
                        });
                    }
                    spans.extend(capture_spans(captures, &md, text, syn));
                    self.emit_spans(text, md.start, md.end, spans);
                    self.cursor = md.end;
                }
                // `flatten_patterns` only yields concrete `Match`/`BeginEnd`/`BeginWhile`.
                Pattern::BeginEnd { .. } | Pattern::BeginWhile { .. } => {
                    self.open_begin(text, pat, syn, &md);
                }
                _ => unreachable!(),
            }
        } else {
            // Nothing matched on the rest of this line: emit it as plain content.
            if line_end > self.cursor {
                self.pending
                    .push_back(TokenizerOp::Content(&text[self.cursor..line_end]));
            }
            self.cursor = line_end;
        }

        // Anti-stall guard. A zero-width match can leave the cursor in place. That is
        // legitimate when it changed the grammar stack (e.g. a lookahead `begin` opening
        // an embedded block), so we only force progress when either nothing at all
        // changed, or a zero-width `begin`/`end` has been oscillating for too long.
        if self.cursor == scan_cursor {
            self.stall_count += 1;
            let depth_unchanged = self.stack.len() == scan_depth;
            if (depth_unchanged || self.stall_count > MAX_STALL) && self.cursor < line_end {
                let ch_len = text[self.cursor..].chars().next().unwrap().len_utf8();
                self.pending.push_back(TokenizerOp::Content(
                    &text[self.cursor..self.cursor + ch_len],
                ));
                self.cursor += ch_len;
                self.stall_count = 0;
            }
        } else {
            self.stall_count = 0;
        }
    }

    /// Collects the concrete candidate rules valid at the cursor: the current frame's
    /// patterns (resolved against whichever grammar owns the frame, so embedded grammars
    /// keep resolving their own includes) plus any injection grammars active for the
    /// current scope stack. Each rule is paired with its owning grammar and the priority
    /// of the source it came from (base patterns are `0`; injected patterns carry their
    /// selector's priority, `L:` > 0 / `R:` < 0).
    fn gather_candidates(&self) -> Vec<(&'a Pattern, &'a SyntaxDefinition, i8)> {
        let base = self.base;
        let set = self.set;
        let frame_patterns = self.stack.last().unwrap().patterns;
        let frame_syntax = self.stack.last().unwrap().syntax;
        let mut candidates: Vec<(&'a Pattern, &'a SyntaxDefinition, i8)> = Vec::new();
        {
            let mut concrete: Vec<(&'a Pattern, &'a SyntaxDefinition)> = Vec::new();
            let mut visited: HashSet<(Scope, &'a str)> = HashSet::new();
            flatten_patterns(set, base, frame_syntax, frame_patterns, &mut concrete, &mut visited);
            candidates.extend(concrete.into_iter().map(|(p, s)| (p, s, 0)));
        }
        if let Some(set) = set
            && set.has_injections()
        {
            let scopes = self.active_scopes.clone();
            // Grammars currently in play: the base plus any embedded grammar on the
            // stack. A grammar's own `injections` map only applies while it is active.
            let mut active_grammars: Vec<Scope> = vec![base.scope];
            active_grammars.extend(self.stack.iter().map(|f| f.syntax.scope));
            for (priority, patterns, syn) in set.matching_injections(&scopes, &active_grammars) {
                let mut concrete: Vec<(&'a Pattern, &'a SyntaxDefinition)> = Vec::new();
                let mut visited: HashSet<(Scope, &'a str)> = HashSet::new();
                flatten_patterns(Some(set), base, syn, patterns, &mut concrete, &mut visited);
                candidates.extend(concrete.into_iter().map(|(p, s)| (p, s, priority)));
            }
        }
        candidates
    }

    /// Opens the begin/end or begin/while block `pat` (owned by `syn`) whose `begin`
    /// matched at `md`: emits any text between the cursor and the match, pushes the
    /// content scope (if the rule has a `name`), emits the begin captures, advances the
    /// cursor past the begin marker, and pushes the new stack frame.
    ///
    /// Shared by the in-line scan and the end-of-line zero-width begin opener.
    fn open_begin(
        &mut self,
        text: &'a str,
        pat: &'a Pattern,
        syn: &'a SyntaxDefinition,
        md: &MatchData,
    ) {
        if md.start > self.cursor {
            self.pending
                .push_back(TokenizerOp::Content(&text[self.cursor..md.start]));
        }
        match pat {
            Pattern::BeginEnd {
                end,
                content_scope,
                begin_captures,
                end_captures,
                patterns,
                ..
            } => {
                // The end regex may reference captures from the begin match.
                let end_str = substitute_backrefs(end, md, text);
                if compile_regex(&end_str).is_ok() {
                    let resolved = content_scope
                        .as_ref()
                        .and_then(|t| resolve_scope(t, md, text));
                    if let Some(scope) = resolved {
                        self.pending.push_back(TokenizerOp::Push(scope));
                        self.active_scopes.push(scope.to_string());
                    }
                    let spans = capture_spans(begin_captures, md, text, syn);
                    self.emit_spans(text, md.start, md.end, spans);
                    self.cursor = md.end;
                    // `\G` may match at the end of this begin match.
                    self.anchor = md.end;
                    self.stack.push(StackFrame {
                        end_regex: Some(end_str),
                        end_captures: Some(end_captures),
                        while_regex: None,
                        while_captures: None,
                        patterns: patterns.as_slice(),
                        syntax: syn,
                        scope: resolved,
                        enter_pos: md.start,
                        enter_rule: pat,
                    });
                } else {
                    // Uncompilable end: treat the begin marker as plain content
                    // so we still make progress.
                    self.pending
                        .push_back(TokenizerOp::Content(&text[md.start..md.end]));
                    self.cursor = md.end;
                }
            }
            Pattern::BeginWhile {
                while_regex,
                content_scope,
                begin_captures,
                while_captures,
                patterns,
                ..
            } => {
                let while_str = substitute_backrefs(while_regex, md, text);
                let resolved = content_scope
                    .as_ref()
                    .and_then(|t| resolve_scope(t, md, text));
                if let Some(scope) = resolved {
                    self.pending.push_back(TokenizerOp::Push(scope));
                    self.active_scopes.push(scope.to_string());
                }
                let spans = capture_spans(begin_captures, md, text, syn);
                self.emit_spans(text, md.start, md.end, spans);
                self.cursor = md.end;
                self.anchor = md.end;
                self.stack.push(StackFrame {
                    end_regex: None,
                    end_captures: None,
                    while_regex: Some(while_str),
                    while_captures: Some(while_captures),
                    patterns: patterns.as_slice(),
                    syntax: syn,
                    scope: resolved,
                    enter_pos: md.start,
                    enter_rule: pat,
                });
            }
            _ => unreachable!("open_begin called with a non-begin pattern"),
        }
    }

    /// At the end-of-line position, tries to open a single zero-width `begin` block (one
    /// matching exactly at the line end with no width, e.g. an embedding `(?<=>)`). Only
    /// such begins can fire here: anything that consumes text would have matched during
    /// the in-line scan. Returns `true` if a block was opened.
    ///
    /// This is what lets lookbehind-anchored embeddings whose opening tag ends a line
    /// (`<script>\n…`) start their embedded grammar, since the lookbehind cannot see the
    /// previous line's final character from the start of the next line.
    fn try_open_eol_begin(&mut self, line: &'a str, line_start: usize, line_end: usize) -> bool {
        let allow_a = line_start == 0;
        let allow_g = self.cursor == self.anchor;
        let pos = self.cursor - line_start;
        let text = self.text;

        let candidates = self.gather_candidates();
        let mut best: Option<(MatchData, &'a Pattern, &'a SyntaxDefinition, i8)> = None;
        for (pat, syn, priority) in candidates {
            let begin = match pat {
                Pattern::BeginEnd { begin, .. } | Pattern::BeginWhile { begin, .. } => begin,
                _ => continue,
            };
            let Some(md) = self.run_regex(begin, line, pos, line_start, allow_a, allow_g) else {
                continue;
            };
            // Only a zero-width begin sitting exactly at the line end is eligible.
            if md.start != line_end || md.end != line_end {
                continue;
            }
            if self.would_reenter(pat, md.start) {
                continue;
            }
            // Skip a begin/end whose `end` would also match (zero-width) right here:
            // opening it produces an empty block that `close_ends_at_line_end` closes on
            // the next step, only to be reopened again — an infinite loop that never
            // emits the newline. A genuine embedding begin (`(?<=>)` with an end like
            // `(?=</script…)`) does not self-close here, so it is unaffected.
            if let Pattern::BeginEnd { end, .. } = pat {
                let end_str = substitute_backrefs(end, &md, text);
                if self.end_closes_at(&end_str, line_start, line_end) {
                    continue;
                }
            }
            // Highest-priority candidate wins; ties keep the first (earliest in the
            // flattened pattern order), matching the in-line scan's preference.
            if best.as_ref().is_none_or(|(_, _, _, bp)| priority > *bp) {
                best = Some((md, pat, syn, priority));
            }
        }

        match best {
            Some((md, pat, syn, _)) => {
                self.open_begin(text, pat, syn, &md);
                true
            }
            None => false,
        }
    }

    /// Whether `end_str` (an already back-reference-substituted end regex) matches
    /// zero-width at the current cursor, exactly at `line_end` — i.e. it would close a
    /// block opened here without consuming any content. Mirrors the close condition used
    /// by [`Self::close_ends_at_line_end`] (testing both the newline-excluding and
    /// newline-including views of the line).
    fn end_closes_at(&mut self, end_str: &str, line_start: usize, line_end: usize) -> bool {
        let allow_a = line_start == 0;
        let allow_g = self.cursor == self.anchor;
        let pos = self.cursor - line_start;
        let text = self.text;
        let no_nl = &text[line_start..line_end];
        let with_nl = if line_end < text.len() {
            &text[line_start..line_end + 1]
        } else {
            no_nl
        };
        let closes_here = |md: &MatchData| md.start == line_end && md.end <= line_end + 1;
        self.run_regex(end_str, no_nl, pos, line_start, allow_a, allow_g)
            .filter(closes_here)
            .or_else(|| {
                self.run_regex(end_str, with_nl, pos, line_start, allow_a, allow_g)
                    .filter(closes_here)
            })
            .is_some()
    }

    /// Emits the text of `[region_start, region_end)` as content, wrapping the (properly
    /// nested) `spans` in `Push`/`Pop` pairs. A span carrying `sub` patterns has its
    /// range recursively tokenized instead of emitted flat. Every byte of the region is
    /// emitted exactly once as [`TokenizerOp::Content`].
    fn emit_spans(
        &mut self,
        text: &'a str,
        region_start: usize,
        region_end: usize,
        mut spans: Vec<Span<'a>>,
    ) {
        // Outer spans first: by start ascending, then by end descending.
        spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));

        let mut pos = region_start;
        let mut open: Vec<usize> = Vec::new(); // stack of end offsets of open (scoped) spans
        let mut i = 0;

        loop {
            let next_start = spans.get(i).map(|s| s.start);
            let next_end = open.last().copied();

            // Decide the next event: open a span or close one. Closes win ties so that
            // adjacent sibling spans nest correctly.
            let (event, is_open) = match (next_start, next_end) {
                (None, None) => {
                    if region_end > pos {
                        self.pending
                            .push_back(TokenizerOp::Content(&text[pos..region_end]));
                    }
                    break;
                }
                (Some(s), None) => (s, true),
                (None, Some(e)) => (e, false),
                (Some(s), Some(e)) => {
                    if e <= s {
                        (e, false)
                    } else {
                        (s, true)
                    }
                }
            };

            if event > pos {
                self.pending
                    .push_back(TokenizerOp::Content(&text[pos..event]));
                pos = event;
            }

            if is_open {
                let start = spans[i].start;
                let end = spans[i].end;
                let scope = spans[i].scope;
                let sub = spans[i].sub;
                if let Some((patterns, syntax)) = sub {
                    // Capture with nested patterns: recursively tokenize its range,
                    // wrapped in its own scope, and skip any spans nested inside it.
                    if let Some(scope) = scope {
                        self.pending.push_back(TokenizerOp::Push(scope));
                    }
                    self.tokenize_sub(text, start, end, patterns, syntax);
                    if scope.is_some() {
                        self.pending.push_back(TokenizerOp::Pop);
                    }
                    pos = end;
                    i += 1;
                    while i < spans.len() && spans[i].start < end {
                        i += 1;
                    }
                } else {
                    self.pending.push_back(TokenizerOp::Push(
                        scope.expect("a span without sub-patterns always has a scope"),
                    ));
                    open.push(end);
                    i += 1;
                }
            } else {
                open.pop();
                self.pending.push_back(TokenizerOp::Pop);
            }
        }
    }

    /// Whether opening `pat` at `pos` would re-enter a block already open at the same
    /// position with the same rule — the signature of a zero-width `begin` infinite loop.
    fn would_reenter(&self, pat: &Pattern, pos: usize) -> bool {
        let p = pat as *const Pattern;
        self.stack
            .iter()
            .any(|f| f.enter_pos == pos && std::ptr::eq(f.enter_rule, p))
    }

    /// At the start of a line, evaluates the `while` condition of every open
    /// `begin`/`while` block (outermost first). Each matching block consumes its `while`
    /// marker; the first block whose condition fails is closed along with everything
    /// nested inside it.
    ///
    /// Returns `true` if it changed any state (so the caller yields this as an event);
    /// `false` means nothing happened and normal scanning should proceed.
    fn process_while_conditions(&mut self, line: &'a str, line_start: usize) -> bool {
        if !self.stack.iter().any(|f| f.while_regex.is_some()) {
            return false;
        }
        let text = self.text;
        let before = (self.pending.len(), self.stack.len(), self.cursor);
        let allow_a = line_start == 0;

        let mut i = 1;
        while i < self.stack.len() {
            let Some(while_re) = self.stack[i].while_regex.clone() else {
                i += 1;
                continue;
            };
            let pos = self.cursor - line_start;
            let md = self.run_regex(&while_re, line, pos, line_start, allow_a, false);
            match md {
                Some(md) if md.start == self.cursor => {
                    // Continue the block: consume the `while` marker and its captures.
                    let syntax = self.stack[i].syntax;
                    let spans = match self.stack[i].while_captures {
                        Some(c) => capture_spans(c, &md, text, syntax),
                        None => Vec::new(),
                    };
                    self.emit_spans(text, md.start, md.end, spans);
                    self.cursor = md.end;
                    self.anchor = md.end;
                    i += 1;
                }
                _ => {
                    // Condition failed: close this block and everything above it.
                    while self.stack.len() > i {
                        let frame = self.stack.pop().unwrap();
                        if frame.scope.is_some() {
                            self.active_scopes.pop();
                            self.pending.push_back(TokenizerOp::Pop);
                        }
                    }
                    break;
                }
            }
        }

        (self.pending.len(), self.stack.len(), self.cursor) != before
    }

    /// Recursively tokenizes `[start, end)` with a capture's nested `patterns`, appending
    /// the resulting ops to `pending`.
    fn tokenize_sub(
        &mut self,
        text: &'a str,
        start: usize,
        end: usize,
        patterns: &'a [Pattern],
        syntax: &'a SyntaxDefinition,
    ) {
        if start >= end {
            return;
        }
        let sub = Tokenizer::build_with_patterns(&text[start..end], patterns, syntax, self.set);
        for op in sub {
            self.pending.push_back(op);
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = TokenizerOp<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(op) = self.pending.pop_front() {
                return Some(op);
            }
            if self.finished {
                return None;
            }
            self.advance();
        }
    }
}

/// Extracts absolute-offset match data from a [`fancy_regex::Captures`].
fn match_data(caps: &fancy_regex::Captures<'_, str>, line_start: usize) -> MatchData {
    let whole = caps.get(0).expect("a match always has group 0");
    let groups = caps
        .iter()
        .map(|g| g.map(|m| (m.start() + line_start, m.end() + line_start)))
        .collect();
    MatchData {
        start: whole.start() + line_start,
        end: whole.end() + line_start,
        groups,
    }
}

/// Flattens a pattern list into the concrete `Match`/`BeginEnd` rules it resolves to,
/// expanding `include`s (including cross-grammar ones, when `set` is available) and bare
/// pattern groups.
///
/// Each concrete rule is paired with the grammar that owns it, so that rules pulled in
/// from an embedded grammar continue to resolve their own includes correctly.
///
/// `current` is the grammar owning `patterns`; `base` is the outermost grammar (the
/// target of `$base`). `visited` (keyed by `(grammar scope, include name)`) guards
/// against include cycles.
fn flatten_patterns<'p>(
    set: Option<&'p SyntaxSet>,
    base: &'p SyntaxDefinition,
    current: &'p SyntaxDefinition,
    patterns: &'p [Pattern],
    out: &mut Vec<(&'p Pattern, &'p SyntaxDefinition)>,
    visited: &mut HashSet<(Scope, &'p str)>,
) {
    for p in patterns {
        match p {
            Pattern::Include(name) => {
                if name == "$self" {
                    if visited.insert((current.scope, "$self")) {
                        flatten_patterns(set, base, current, &current.patterns, out, visited);
                    }
                } else if name == "$base" {
                    if visited.insert((base.scope, "$base")) {
                        flatten_patterns(set, base, base, &base.patterns, out, visited);
                    }
                } else if let Some(local) = name.strip_prefix('#') {
                    if visited.insert((current.scope, local))
                        && let Some(rule) = current.repository.get(local)
                    {
                        flatten_patterns(
                            set,
                            base,
                            current,
                            std::slice::from_ref(rule),
                            out,
                            visited,
                        );
                    }
                } else if let Some(set) = set {
                    // Cross-grammar: `source.x` or `source.x#rule`.
                    let (scope_str, rule) = match name.split_once('#') {
                        Some((s, r)) => (s, Some(r)),
                        None => (name.as_str(), None),
                    };
                    if let Ok(scope) = Scope::new(scope_str)
                        && let Some(other) = set.find_by_scope(scope)
                    {
                        match rule {
                            Some(rule) => {
                                if visited.insert((other.scope, rule))
                                    && let Some(r) = other.repository.get(rule)
                                {
                                    flatten_patterns(
                                        Some(set),
                                        base,
                                        other,
                                        std::slice::from_ref(r),
                                        out,
                                        visited,
                                    );
                                }
                            }
                            None => {
                                if visited.insert((other.scope, "")) {
                                    flatten_patterns(
                                        Some(set),
                                        base,
                                        other,
                                        &other.patterns,
                                        out,
                                        visited,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Pattern::Patterns(ps) => flatten_patterns(set, base, current, ps, out, visited),
            Pattern::Match { .. } | Pattern::BeginEnd { .. } | Pattern::BeginWhile { .. } => {
                out.push((p, current))
            }
        }
    }
}

/// Builds the capture spans for a match from a `group index -> Capture` map, resolving
/// `$n` scope-name interpolation and carrying through any nested capture `patterns`.
fn capture_spans<'a>(
    captures: &'a HashMap<usize, Capture>,
    md: &MatchData,
    text: &str,
    syntax: &'a SyntaxDefinition,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    // Iterate by ascending group index so the output order is deterministic (the source
    // is a `HashMap`, whose iteration order is not). Lower-indexed groups also enclose
    // higher-indexed ones in practice, so for spans sharing a range this yields the
    // outer-before-inner order that `emit_spans`' stable sort then preserves.
    let mut indices: Vec<usize> = captures.keys().copied().collect();
    indices.sort_unstable();
    for idx in indices {
        let cap = &captures[&idx];
        if let Some(Some((start, end))) = md.groups.get(idx) {
            // Clip to the overall match: capture groups inside a lookahead can extend
            // beyond the (possibly zero-width) match, and that text has not been
            // consumed by this rule — emitting it here would duplicate it.
            let start = (*start).max(md.start);
            let end = (*end).min(md.end);
            if end <= start {
                continue;
            }
            let scope = cap.name.as_ref().and_then(|t| resolve_scope(t, md, text));
            let sub = if cap.patterns.is_empty() {
                None
            } else {
                Some((cap.patterns.as_slice(), syntax))
            };
            if scope.is_some() || sub.is_some() {
                spans.push(Span {
                    start,
                    end,
                    scope,
                    sub,
                });
            }
        }
    }
    spans
}

/// Resolves a [`ScopeTemplate`] to a concrete [`Scope`], interpolating `$n` capture
/// references against `md` for dynamic templates. Returns `None` if the resulting scope
/// is invalid (e.g. too many atoms).
fn resolve_scope(template: &ScopeTemplate, md: &MatchData, text: &str) -> Option<Scope> {
    match template {
        ScopeTemplate::Static(scope) => Some(*scope),
        ScopeTemplate::Dynamic(tpl) => Scope::new(&interpolate(tpl, md, text)).ok(),
    }
}

/// Replaces `$0`..`$9` in a scope-name template with the text of the corresponding
/// capture group (empty if the group did not participate).
fn interpolate(tpl: &str, md: &MatchData, text: &str) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut chars = tpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$'
            && let Some(&d) = chars.peek()
            && let Some(idx) = d.to_digit(10)
        {
            chars.next();
            if let Some(Some((start, end))) = md.groups.get(idx as usize) {
                out.push_str(&text[*start..*end]);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Compiles a regex after translating Oniguruma-only constructs that the `regex`
/// crate (via `fancy_regex`) does not accept. Centralised so every compile site —
/// `begin`/`match`/`while` (through [`Self::compiled`]) and the `end` compile check —
/// benefits from the same rewrites.
fn compile_regex(pattern: &str) -> Result<Regex, fancy_regex::Error> {
    match build_regex(pattern) {
        Ok(re) => Ok(re),
        // Only pay the translation/recompile cost when the raw pattern fails: the vast
        // majority compile as-is, and an Oniguruma-ism is the common failure cause.
        Err(_) => build_regex(&translate_oniguruma(pattern)),
    }
}

/// Builds a single regex with the options the tokenizer relies on:
///
/// - `allow_input_assertion_overrides` so [`RegexInput::continue_from_previous_match_end`]
///   can suppress `\G` at runtime (this is what replaces the old `\G` string shim).
/// - `oniguruma_mode` to better match the Oniguruma engine the reference grammars target
///   (e.g. `\<`/`\>` as literal angle brackets, and empty repeats silently dropped).
fn build_regex(pattern: &str) -> Result<Regex, fancy_regex::Error> {
    RegexBuilder::new(pattern)
        .allow_input_assertion_overrides(true)
        .oniguruma_mode(true)
        .build()
}

/// Rewrites the handful of Oniguruma POSIX-property escapes that grammars in the wild
/// use but the `regex` crate spells differently. Notably `\p{word}` / `\P{word}`
/// (Oniguruma's "word character" property), which kills rules like Python's
/// `function-declaration` and is also used by gleam, ocaml, mojo, asciidoc and vyper.
fn translate_oniguruma(pattern: &str) -> std::borrow::Cow<'_, str> {
    if !pattern.contains("p{word}") {
        return std::borrow::Cow::Borrowed(pattern);
    }
    std::borrow::Cow::Owned(
        pattern
            .replace("\\p{word}", "\\w")
            .replace("\\P{word}", "\\W"),
    )
}

/// Rewrites a `\A` (start-of-document) anchor that is not permitted at the current
/// position into a never-matching assertion (`\b\B`), so a pattern relying on it fails
/// to match there. `\A` is only allowed on the first line.
///
/// Unlike `\G` (handled at runtime via [`RegexInput::continue_from_previous_match_end`]),
/// `\A` has no runtime override that does not also suppress `^` — and `^` must keep
/// matching at the start of every per-line slice — so it is neutered by rewriting.
fn neuter_doc_start(re: &str, allow_a: bool) -> std::borrow::Cow<'_, str> {
    if allow_a || !re.contains("\\A") {
        return std::borrow::Cow::Borrowed(re);
    }
    let mut out = String::with_capacity(re.len());
    let mut chars = re.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('A') => {
                    chars.next();
                    out.push_str("\\b\\B");
                }
                Some(&n) => {
                    chars.next();
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Substitutes numeric backreferences (`\1`..`\9`) in an `end` pattern with the
/// regex-escaped text matched by the corresponding group of the begin match. This
/// supports constructs like heredocs (`<<-(\w+) ... \1`).
fn substitute_backrefs(end: &str, md: &MatchData, text: &str) -> String {
    let mut result = String::with_capacity(end.len());
    let mut chars = end.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&d) = chars.peek()
                && let Some(idx) = d.to_digit(10)
            {
                chars.next();
                if let Some(Some((start, end))) = md.groups.get(idx as usize) {
                    result.push_str(&fancy_regex::escape(&text[*start..*end]));
                }
                continue;
            }
            // Not a backreference: keep the backslash and let the next char be
            // processed normally so escapes like `\.` / `\\` survive intact.
            result.push('\\');
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenates a tokenized op stream back into source text, asserting that every
    /// byte is accounted for exactly once.
    fn reconstruct(ops: &[TokenizerOp]) -> String {
        let mut s = String::new();
        for op in ops {
            match op {
                TokenizerOp::Content(c) => s.push_str(c),
                TokenizerOp::Newline => s.push('\n'),
                TokenizerOp::Push(_) | TokenizerOp::Pop => {}
            }
        }
        s
    }

    fn assert_balanced(ops: &[TokenizerOp]) {
        let mut depth = 0i32;
        for op in ops {
            match op {
                TokenizerOp::Push(_) => depth += 1,
                TokenizerOp::Pop => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "Pop without matching Push");
        }
        assert_eq!(depth, 0, "unbalanced Push/Pop");
    }

    const GRAMMAR: &str = r###"
    {
        "scopeName": "source.test",
        "patterns": [
            { "match": "\\d+", "name": "constant.numeric" },
            {
                "begin": "\"",
                "end": "\"",
                "name": "string.quoted.double",
                "patterns": [
                    { "match": "\\\\.", "name": "constant.character.escape" }
                ]
            }
        ]
    }
    "###;

    #[test]
    fn test_tokenize_exact() {
        let syntax = SyntaxDefinition::from_json_str(GRAMMAR).expect("load grammar");
        let input = "12 \"a\"";
        let ops: Vec<_> = Tokenizer::new(input, &syntax).collect();

        let numeric = Scope::new("constant.numeric").unwrap();
        let string = Scope::new("string.quoted.double").unwrap();
        let expected = vec![
            TokenizerOp::Push(numeric),
            TokenizerOp::Content("12"),
            TokenizerOp::Pop,
            TokenizerOp::Content(" "),
            TokenizerOp::Push(string),
            TokenizerOp::Content("\""),
            TokenizerOp::Content("a"),
            TokenizerOp::Content("\""),
            TokenizerOp::Pop,
        ];
        assert_eq!(ops, expected);
    }

    #[test]
    fn test_string_escape_capture() {
        let syntax = SyntaxDefinition::from_json_str(GRAMMAR).expect("load grammar");
        let input = "\"a\\nb\"";
        let ops: Vec<_> = Tokenizer::new(input, &syntax).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        let escape = Scope::new("constant.character.escape").unwrap();
        assert!(
            ops.contains(&TokenizerOp::Push(escape)),
            "escape sequence should be scoped: {ops:?}"
        );
    }

    #[test]
    fn test_multiline_block_spans_newline() {
        let syntax = SyntaxDefinition::from_json_str(GRAMMAR).expect("load grammar");
        let input = "\"line1\nline2\"";
        let ops: Vec<_> = Tokenizer::new(input, &syntax).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);
        assert!(ops.contains(&TokenizerOp::Newline));
    }

    const HOST_GRAMMAR: &str = r###"
    {
        "scopeName": "source.host",
        "patterns": [
            {
                "begin": "<<",
                "end": ">>",
                "name": "meta.embedded.guest",
                "patterns": [{ "include": "source.guest" }]
            }
        ]
    }
    "###;

    const GUEST_GRAMMAR: &str = r###"
    {
        "scopeName": "source.guest",
        "patterns": [
            { "match": "\\d+", "name": "constant.numeric.guest" }
        ]
    }
    "###;

    #[test]
    fn test_cross_grammar_include() {
        use crate::SyntaxSet;

        let mut set = SyntaxSet::new();
        set.add(SyntaxDefinition::from_json_str(HOST_GRAMMAR).unwrap());
        set.add(SyntaxDefinition::from_json_str(GUEST_GRAMMAR).unwrap());

        let host = set
            .find_by_scope(Scope::new("source.host").unwrap())
            .unwrap();
        let input = "a << 42 >> b";
        let ops: Vec<_> = Tokenizer::new_in_set(input, host, &set).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        // The number inside the embedded block must pick up the *guest* grammar's scope.
        let guest_num = Scope::new("constant.numeric.guest").unwrap();
        assert!(
            ops.contains(&TokenizerOp::Push(guest_num)),
            "embedded grammar scope missing: {ops:?}"
        );
    }

    #[test]
    fn test_standalone_skips_cross_grammar_include() {
        // Without a SyntaxSet, the `source.guest` include is simply skipped; the digits
        // are emitted as plain content rather than panicking.
        let host = SyntaxDefinition::from_json_str(HOST_GRAMMAR).unwrap();
        let input = "<< 42 >>";
        let ops: Vec<_> = Tokenizer::new(input, &host).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);
        let guest_num = Scope::new("constant.numeric.guest").unwrap();
        assert!(!ops.contains(&TokenizerOp::Push(guest_num)));
    }

    const PLAIN_HOST: &str = r###"
    {
        "scopeName": "source.plain",
        "patterns": [{ "match": "\\w+", "name": "text.word.plain" }]
    }
    "###;

    // An injection grammar that highlights `TODO` anywhere inside `source.plain`.
    const TODO_INJECTION: &str = r###"
    {
        "scopeName": "comment.todo.injection",
        "injectionSelector": "L:source.plain",
        "patterns": [{ "match": "TODO", "name": "keyword.todo" }]
    }
    "###;

    #[test]
    fn test_injection_applies() {
        use crate::SyntaxSet;

        let mut set = SyntaxSet::new();
        set.add(SyntaxDefinition::from_json_str(PLAIN_HOST).unwrap());
        set.add(SyntaxDefinition::from_json_str(TODO_INJECTION).unwrap());

        let host = set
            .find_by_scope(Scope::new("source.plain").unwrap())
            .unwrap();
        let input = "fix TODO later";
        let ops: Vec<_> = Tokenizer::new_in_set(input, host, &set).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        // The `L:` injection wins the tie against the base `\w+` rule for `TODO`.
        let todo = Scope::new("keyword.todo").unwrap();
        assert!(
            ops.contains(&TokenizerOp::Push(todo)),
            "injected scope missing: {ops:?}"
        );
    }

    // An auxiliary grammar that declares a broad cross-grammar injection via its
    // `injections` map (not `injectionSelector`). This must NOT pollute other grammars
    // when it is merely registered but not actually in play.
    const AUX_INJECTOR: &str = r###"
    {
        "scopeName": "inline.aux",
        "injections": {
            "L:source": {
                "patterns": [{ "match": "<", "name": "invalid.illegal.aux" }]
            }
        },
        "patterns": []
    }
    "###;

    #[test]
    fn test_injections_map_gated_to_owner() {
        use crate::SyntaxSet;

        let mut set = SyntaxSet::new();
        set.add(SyntaxDefinition::from_json_str(PLAIN_HOST).unwrap());
        set.add(SyntaxDefinition::from_json_str(AUX_INJECTOR).unwrap());

        let host = set
            .find_by_scope(Scope::new("source.plain").unwrap())
            .unwrap();
        let input = "a < b";
        let ops: Vec<_> = Tokenizer::new_in_set(input, host, &set).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        // `inline.aux` is not active while tokenizing `source.plain`, so its `injections`
        // map (selector `L:source`, which would otherwise match) must not apply.
        let aux = Scope::new("invalid.illegal.aux").unwrap();
        assert!(
            !ops.contains(&TokenizerOp::Push(aux)),
            "aux injections-map leaked into source.plain: {ops:?}"
        );
    }

    /// A `begin`/`end` block whose `end` is anchored to the end of the line (`$`) must
    /// close on that line rather than leaking into the next one.
    const LINE_BLOCK_GRAMMAR: &str = r###"
    {
        "scopeName": "source.lineblock",
        "patterns": [
            { "begin": "@", "end": "$", "name": "meta.directive" },
            { "begin": "#", "end": "\\n", "name": "comment.line" },
            { "match": "\\w+", "name": "keyword.word" }
        ]
    }
    "###;

    #[test]
    fn test_dollar_end_block_does_not_leak() {
        let syntax = SyntaxDefinition::from_json_str(LINE_BLOCK_GRAMMAR).expect("load grammar");
        let input = "@directive\nword\n";
        let ops: Vec<_> = Tokenizer::new(input, &syntax).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        // `word` on the second line must NOT be inside `meta.directive`: there must be a
        // Pop closing the directive before `word` is pushed.
        let directive = Scope::new("meta.directive").unwrap();
        let word = Scope::new("keyword.word").unwrap();
        let directive_idx = ops
            .iter()
            .position(|op| *op == TokenizerOp::Push(directive))
            .expect("directive opened");
        let word_idx = ops
            .iter()
            .position(|op| *op == TokenizerOp::Push(word))
            .expect("word tokenized");
        assert!(
            ops[directive_idx..word_idx].contains(&TokenizerOp::Pop),
            "meta.directive leaked into the next line: {ops:?}"
        );
    }

    /// A line comment whose `end` consumes the newline (`end: \n`) must also close on its
    /// own line, with the newline still emitted as a `Newline` op (not swallowed).
    #[test]
    fn test_newline_end_comment_does_not_leak() {
        let syntax = SyntaxDefinition::from_json_str(LINE_BLOCK_GRAMMAR).expect("load grammar");
        let input = "# note\nword\n";
        let ops: Vec<_> = Tokenizer::new(input, &syntax).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        let comment = Scope::new("comment.line").unwrap();
        let word = Scope::new("keyword.word").unwrap();
        let comment_idx = ops
            .iter()
            .position(|op| *op == TokenizerOp::Push(comment))
            .expect("comment opened");
        let word_idx = ops
            .iter()
            .position(|op| *op == TokenizerOp::Push(word))
            .expect("word tokenized");
        assert!(
            ops[comment_idx..word_idx].contains(&TokenizerOp::Pop),
            "comment.line leaked into the next line: {ops:?}"
        );
    }

    // A host grammar that embeds a guest via a lookbehind-anchored zero-width begin
    // (`(?<=>)`), exactly like HTML/Vue `<script>`/`<style>`/`<template>` bodies. The
    // embedding begin only matches at the end of the opening tag's line, so the guest
    // block must be opened there for the embed to span the following lines.
    const EMBED_HOST: &str = r###"
    {
        "scopeName": "source.host",
        "patterns": [
            {
                "begin": "<s>",
                "end": "</s>",
                "name": "meta.tag.host",
                "patterns": [
                    {
                        "begin": "(?<=>)",
                        "end": "(?=</s>)",
                        "name": "meta.embedded.host",
                        "patterns": [{ "include": "source.guest" }]
                    }
                ]
            }
        ]
    }
    "###;

    const EMBED_GUEST: &str = r###"
    {
        "scopeName": "source.guest",
        "patterns": [{ "match": "\\d+", "name": "constant.numeric.guest" }]
    }
    "###;

    /// A lookbehind-anchored embedding begin (`(?<=>)`) whose anchor sits at the end of
    /// the opening tag's line must still open the embedded block: the lookbehind can see
    /// the line's final `>` from the end-of-line position, even though it cannot from the
    /// start of the next line. The embedded block must then close cleanly at its end
    /// marker rather than leaking past it.
    #[test]
    fn test_embedding_begin_fires_across_line_boundary() {
        use crate::SyntaxSet;

        let mut set = SyntaxSet::new();
        set.add(SyntaxDefinition::from_json_str(EMBED_HOST).unwrap());
        set.add(SyntaxDefinition::from_json_str(EMBED_GUEST).unwrap());

        let host = set
            .find_by_scope(Scope::new("source.host").unwrap())
            .unwrap();
        // The opening tag ends line 1; the guest content (`42`) is on line 2; the closing
        // tag is on line 3.
        let input = "<s>\n42\n</s>";
        let ops: Vec<_> = Tokenizer::new_in_set(input, host, &set).collect();

        assert_eq!(reconstruct(&ops), input);
        assert_balanced(&ops);

        // The embedded block opened (across the line boundary) and the guest grammar
        // tokenized the number on the next line.
        let embedded = Scope::new("meta.embedded.host").unwrap();
        let guest_num = Scope::new("constant.numeric.guest").unwrap();
        let embed_idx = ops
            .iter()
            .position(|op| *op == TokenizerOp::Push(embedded))
            .expect("embedded block never opened across the line boundary");
        assert!(
            ops.contains(&TokenizerOp::Push(guest_num)),
            "guest grammar did not tokenize embedded content: {ops:?}"
        );

        // The embedded block must close (Pop) before the closing `</s>` tag is emitted —
        // i.e. it does not leak past its closing tag. Locate the `</s>` content and check
        // a Pop of the embedded scope precedes it.
        let close_idx = ops
            .iter()
            .position(|op| matches!(op, TokenizerOp::Content("</s>")))
            .expect("closing tag content not found");
        assert!(
            ops[embed_idx..close_idx].contains(&TokenizerOp::Pop),
            "embedded block leaked past its closing tag: {ops:?}"
        );
    }
}
