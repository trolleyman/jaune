//! Shared helpers for the integration tests.
//!
//! Grammars are loaded directly from `assets/grammars/` at runtime (rather than via the
//! `bundled` Cargo features) so that these tests exercise the full grammar set without
//! needing every feature compiled in.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use jaune::{Scope, SyntaxDefinition, SyntaxSet, TokenizerOp};

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn assets_grammars_dir() -> PathBuf {
    manifest_dir().join("assets/grammars")
}

pub fn samples_dir() -> PathBuf {
    manifest_dir().join("textmate-grammars-themes/samples")
}

/// Loads every grammar in `assets/grammars/` into a [`SyntaxSet`]. Returns `None` if the
/// assets directory has not been generated yet (run `bun run package-grammars`).
pub fn load_all() -> Option<SyntaxSet> {
    let dir = assets_grammars_dir();
    if !dir.exists() {
        return None;
    }
    let mut set = SyntaxSet::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(def) = SyntaxDefinition::from_json_str(&text)
        {
            set.add(def);
        }
    }
    Some(set)
}

/// Concatenates an op stream back into source text.
pub fn reconstruct(ops: &[TokenizerOp]) -> String {
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

/// Returns `Err` with a description if the `Push`/`Pop` ops are not balanced.
pub fn check_balanced(ops: &[TokenizerOp]) -> Result<(), String> {
    let mut depth = 0i32;
    for op in ops {
        match op {
            TokenizerOp::Push(_) => depth += 1,
            TokenizerOp::Pop => {
                depth -= 1;
                if depth < 0 {
                    return Err("Pop without matching Push".to_string());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(format!("unbalanced Push/Pop (depth {depth} at end)"));
    }
    Ok(())
}

/// The comment delimiters for a grammar. Assertion lines are rendered as real comments in
/// the fixture's own language, so each fixture is a valid (if heavily annotated) source file.
#[derive(Clone, Copy)]
pub struct Comment {
    pub open: &'static str,
    pub close: &'static str,
}

impl Comment {
    /// A line comment (e.g. `//`, `#`, `--`): no closing delimiter.
    pub const fn line(open: &'static str) -> Self {
        Comment { open, close: "" }
    }

    /// A block comment (e.g. `<!-- -->`, `/* */`).
    pub const fn block(open: &'static str, close: &'static str) -> Self {
        Comment { open, close }
    }

    /// Wraps `body` in the comment delimiters: `<open> <body>[ <close>]`.
    fn wrap(&self, body: &str) -> String {
        if self.close.is_empty() {
            format!("{} {body}", self.open)
        } else {
            format!("{} {body} {}", self.open, self.close)
        }
    }
}

/// Renders one marker line for the inclusive column span `[start, end]`, labelled with `label`.
///
/// The span start is shown as a caret (`^`) and its extent as trailing dashes (`^----` is a
/// five-column span). When the start lies past the comment opener the caret is drawn directly
/// above the character; when it hides *underneath* the opener (where a caret can't go) the start
/// index is written instead and dashes run out to the end column (`//<0------`).
fn marker_line(comment: &Comment, start: usize, end: usize, label: &str) -> String {
    let open_width = comment.open.chars().count();
    let mut s = String::from(comment.open);
    if start >= open_width {
        s.extend(std::iter::repeat_n(' ', start - open_width));
        s.push('^');
        s.extend(std::iter::repeat_n('-', end - start));
    } else {
        s.push('<');
        s.push_str(&start.to_string());
        let rightmost = s.chars().count() - 1;
        if end > rightmost {
            s.extend(std::iter::repeat_n('-', end - rightmost));
        }
    }
    s.push(' ');
    s.push_str(label);
    if !comment.close.is_empty() {
        s.push(' ');
        s.push_str(comment.close);
    }
    s
}

/// Computes the marker lines for one source line's tokens.
///
/// Each token carries its scope stack with the (assumed) grammar root already stripped. Rather
/// than print the full stack per token — which repeats every shared parent — this reconstructs
/// the nested scope *regions*: at each depth, a maximal run of adjacent tokens sharing the same
/// scope prefix becomes one span, labelled with just the scope added at that depth. Spans are
/// emitted outermost-first, left-to-right, so the nesting reads top-down.
fn line_markers(tokens: &[(usize, usize, Vec<String>)], comment: &Comment) -> Vec<String> {
    let max_depth = tokens.iter().map(|(_, _, s)| s.len()).max().unwrap_or(0);
    // (start, end, depth, label)
    let mut spans: Vec<(usize, usize, usize, &str)> = Vec::new();
    for depth in 0..max_depth {
        let mut i = 0;
        while i < tokens.len() {
            let (start, len, scopes) = &tokens[i];
            if scopes.len() > depth {
                let prefix = &scopes[..=depth];
                let mut end = start + len - 1;
                let mut j = i + 1;
                while j < tokens.len()
                    && tokens[j].2.len() > depth
                    && &tokens[j].2[..=depth] == prefix
                {
                    end = tokens[j].0 + tokens[j].1 - 1;
                    j += 1;
                }
                spans.push((*start, end, depth, scopes[depth].as_str()));
                i = j;
            } else {
                i += 1;
            }
        }
    }
    spans.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    spans
        .iter()
        .map(|&(start, end, _, label)| marker_line(comment, start, end, label))
        .collect()
}

/// How to pick the comment syntax for each line of a fixture.
///
/// Most fixtures use one comment style throughout ([`Fixed`](CommentScheme::Fixed)). Embedded
/// fixtures, though, weave several languages together, and an assertion comment sits *inside*
/// whichever block it annotates — so a comment under a line of CSS-in-`<style>` must be a CSS
/// comment, not an HTML one. [`Markdown`](CommentScheme::Markdown) and [`Html`](CommentScheme::Html)
/// scan the source structurally (fenced code blocks; `<script>`/`<style>` regions) and switch the
/// comment style per line accordingly.
#[derive(Clone, Copy)]
pub enum CommentScheme {
    /// One comment style for the whole file.
    Fixed(Comment),
    /// Markdown: HTML comments in prose, switching to the fenced block's language inside ```` ``` ````.
    Markdown,
    /// HTML/Vue: HTML comments in markup, switching to CSS inside `<style>` and JS inside `<script>`.
    Html,
}

const OUTER_HTML: Comment = Comment::block("<!--", "-->");

impl CommentScheme {
    /// The comment style for the header line (and any line not otherwise classified).
    fn primary(&self) -> Comment {
        match self {
            CommentScheme::Fixed(c) => *c,
            CommentScheme::Markdown | CommentScheme::Html => OUTER_HTML,
        }
    }

    /// The comment style for each source line of `input`, indexed by line number.
    fn per_line(&self, input: &str) -> Vec<Comment> {
        match self {
            CommentScheme::Fixed(c) => input.split('\n').map(|_| *c).collect(),
            CommentScheme::Markdown => markdown_per_line(input),
            CommentScheme::Html => html_per_line(input),
        }
    }
}

/// The [`CommentScheme`] to annotate a sample of grammar `name` with.
///
/// The comment *delimiter* is chosen to be valid for the language so the generated file reads as
/// real (if heavily annotated) source — `rem` for batch, `--` for SQL, `;` for Lisps, and so on.
/// It is best-effort: unrecognised grammars fall back to `#`. Crucially, the choice is purely
/// cosmetic — both the jaune and reference renderers use this same function, so it never affects
/// the diff between them.
pub fn scheme_for(name: &str) -> CommentScheme {
    // Markup languages whose comments switch per embedded block.
    match name {
        "html" | "html-derivative" | "vue" | "vue-html" | "svelte" | "astro" | "angular-html"
        | "marko" | "edge" | "blade" | "liquid" | "twig" | "handlebars" => return CommentScheme::Html,
        "markdown" | "mdx" | "mdc" | "markdown-vue" => return CommentScheme::Markdown,
        _ => {}
    }
    CommentScheme::Fixed(comment_for(name))
}

/// The line/block comment delimiters conventionally used by grammar `name` (best-effort; see
/// [`scheme_for`]). Defaults to `#`.
fn comment_for(name: &str) -> Comment {
    let line = Comment::line;
    match name {
        // C-family / curly-brace `//`.
        "javascript" | "typescript" | "jsx" | "tsx" | "c" | "cpp" | "cpp-macro" | "csharp"
        | "java" | "go" | "rust" | "swift" | "kotlin" | "scala" | "dart" | "css" | "scss"
        | "less" | "postcss" | "stylus" | "json5" | "jsonc" | "glsl" | "hlsl" | "wgsl"
        | "shaderlab" | "gdshader" | "solidity" | "zig" | "v" | "d" | "haxe" | "groovy"
        | "proto" | "objective-c" | "objective-cpp" | "typespec" | "prisma" | "move" | "cairo"
        | "c3" | "jsonnet" | "vala" | "genie" | "gleam" | "hcl" | "terraform" | "jssm" | "apex"
        | "bicep" | "wit" | "odin" | "nextflow" | "nextflow-groovy" | "qml" | "qmldir" | "dax"
        | "kusto" | "rel" | "pkl" | "cue" | "luau" | "imba" | "rescript" | "reason" | "slint"
        | "templ" | "ballerina" | "ara" | "berry" | "moonbit" | "vyper" | "wgsl-bevy" | "json"
        | "jsonl" | "es-tag-css" | "es-tag-glsl" | "es-tag-html" | "es-tag-sql" | "es-tag-xml"
        | "ts-tags" | "typst" | "kdl" | "graphql" | "dream-maker" | "angular-ts" => line("//"),

        // Hash `#`.
        "python" | "ruby" | "perl" | "shellscript" | "shellsession" | "fish" | "nushell"
        | "yaml" | "toml" | "r" | "julia" | "nim" | "elixir" | "crystal" | "powershell" | "make"
        | "docker" | "gdscript" | "coffee" | "tcl" | "awk" | "gnuplot" | "nginx" | "apache"
        | "ini" | "dotenv" | "cmake" | "puppet" | "nix" | "raku" | "hjson" | "just" | "mojo"
        | "git-commit" | "git-rebase" | "po" | "codeowners" | "ssh-config" | "hurl" | "ron"
        | "elm-json" | "hy" | "fennel-hash" | "narrat" | "bsl" | "sdbl" | "stata" | "sas-hash"
        | "perl6" | "saturn" | "systemd" | "desktop" | "gn" | "starlark" | "bazel" | "jinja"
        | "jinja-html" | "csv" | "tsv" | "log" | "http" | "wikitext" | "fluent" | "hxml"
        | "qss" | "ssh" => line("#"),

        // Double-dash `--`.
        "sql" | "plsql" | "lua" | "haskell" | "elm" | "purescript" | "ada" | "vhdl"
        | "applescript" | "surrealql" | "cypher" | "sparql" | "elixir-comment" | "idris"
        | "agda" | "lean" | "pgsql" | "hoon" | "eiffel" => line("--"),

        // Semicolon `;` (Lisps and assembly).
        "clojure" | "scheme" | "racket" | "common-lisp" | "emacs-lisp" | "fennel" | "asm"
        | "mipsasm" | "riscv" | "llvm" | "wasm" | "ahk" | "ahk2" | "reg" | "scheme-srfi"
        | "newlisp" | "logo" => line(";"),

        // Percent `%`.
        "latex" | "tex" | "bibtex" | "erlang" | "prolog" | "matlab" | "postscript"
        | "mercury" => line("%"),

        // Other singletons.
        "bat" => line("rem"),
        "viml" => line("\""),
        "fortran-free-form" => line("!"),
        "fortran-fixed-form" => line("C"),
        "cobol" => line("*>"),
        "vb" => line("'"),
        "asciidoc" => line("//"),
        "org" => line("#"),
        "rst" => line(".."),

        // Block-comment-only languages.
        "ocaml" | "fsharp" | "coq" | "wolfram" | "pascal" | "sml" | "standard-ml" => {
            Comment::block("(*", "*)")
        }
        "xml" | "xsl" | "svg" => OUTER_HTML,

        _ => line("#"),
    }
}

/// The conventional line/block comment for a fenced-code or embedded language, by its info string
/// or tag name. Returns `None` for unknown languages, so the caller can fall back to the outer one.
fn lang_comment(lang: &str) -> Option<Comment> {
    Some(match lang {
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "json" | "c" | "cpp"
        | "java" | "go" | "php" | "csharp" | "cs" | "rust" | "rs" | "swift" | "scss" => {
            Comment::line("//")
        }
        "python" | "py" | "ruby" | "rb" | "sh" | "bash" | "shell" | "shellscript" | "yaml"
        | "toml" => Comment::line("#"),
        "sql" | "lua" => Comment::line("--"),
        "css" => Comment::block("/*", "*/"),
        "html" | "xml" | "markdown" | "md" | "vue" => OUTER_HTML,
        _ => return None,
    })
}

/// Per-line comments for a Markdown fixture: HTML comments everywhere except inside a fenced code
/// block, where the fence's info string (```` ```js ````, ```` ```python ````) selects the comment.
fn markdown_per_line(input: &str) -> Vec<Comment> {
    let mut fence: Option<Comment> = None;
    input
        .split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                match fence.take() {
                    // A closing fence: the lines below it are prose again.
                    Some(_) => OUTER_HTML,
                    // An opening fence: classify it and everything up to the close.
                    None => {
                        let lang = trimmed.trim_start_matches('`').trim();
                        let c = lang_comment(lang).unwrap_or(OUTER_HTML);
                        fence = Some(c);
                        c
                    }
                }
            } else {
                fence.unwrap_or(OUTER_HTML)
            }
        })
        .collect()
}

/// Per-line comments for an HTML/Vue fixture: HTML comments in markup, CSS comments inside a
/// `<style>` block, and JS comments inside a `<script>` block. A block opened and closed on the
/// same line (e.g. `<style>.x {}</style>`) stays HTML, since the line is not wholly embedded.
fn html_per_line(input: &str) -> Vec<Comment> {
    #[derive(Clone, Copy)]
    enum Mode {
        Html,
        Css,
        Js,
    }
    let mut mode = Mode::Html;
    input
        .split('\n')
        .map(|line| {
            // Update the mode from this line's tags first, so a comment sits in the context the
            // *next* line begins in: the assertions under `<script>` land inside the script.
            if line.contains("</style>") || line.contains("</script>") {
                mode = Mode::Html;
            } else if line.contains("<style") {
                mode = Mode::Css;
            } else if line.contains("<script") {
                mode = Mode::Js;
            }
            match mode {
                Mode::Html => OUTER_HTML,
                Mode::Css => Comment::block("/*", "*/"),
                Mode::Js => Comment::line("//"),
            }
        })
        .collect()
}

/// A token on a source line: `(start column, char length, scope stack with the grammar root
/// stripped)`. The shared currency between the jaune and reference renderers.
pub type LineToken = (usize, usize, Vec<String>);

/// Renders an annotated source file from per-line tokens: the original `input` with, beneath each
/// line, the nested scope regions marked as comments.
///
/// `lines[i]` holds the tokens of source line `i`. The grammar root (`scope`, named once in the
/// header) is *assumed* and must already be stripped from each token's scope list; shared parent
/// scopes are then factored out so each line shows only the scopes that actually change across it.
/// Comments use the grammar's own syntax — see [`CommentScheme`] — so the file stays valid (if
/// heavily annotated) source. Both jaune and the reference tokenizer render through this, so any
/// diff between their outputs is a genuine tokenization difference, never a formatting one.
pub fn render_annotated(
    scope: Scope,
    input: &str,
    lines: &[Vec<LineToken>],
    scheme: &CommentScheme,
) -> String {
    let line_comments = scheme.per_line(input);
    let header = scheme.primary();

    let mut out = header.wrap(&format!("grammar: {scope}"));
    out.push_str("\n\n");

    // `input` round-trips the token stream, so its `\n`-split lines line up 1:1 with `lines`. A
    // trailing newline yields a final empty entry with no tokens, which we drop.
    let mut source_lines: Vec<&str> = input.split('\n').collect();
    if input.ends_with('\n') {
        source_lines.pop();
    }
    for (i, text) in source_lines.iter().enumerate() {
        out.push_str(text);
        out.push('\n');
        if let Some(tokens) = lines.get(i) {
            let comment = line_comments.get(i).copied().unwrap_or(header);
            for marker in line_markers(tokens, &comment) {
                out.push_str(&marker);
                out.push('\n');
            }
        }
    }
    out
}

/// Renders a jaune op stream as an annotated source file. See [`render_annotated`].
pub fn render_sample(scope: Scope, ops: &[TokenizerOp], scheme: &CommentScheme, input: &str) -> String {
    let mut lines: Vec<Vec<LineToken>> = Vec::new();
    let mut current: Vec<LineToken> = Vec::new();
    let mut stack: Vec<Scope> = vec![scope];
    let mut col = 0usize;

    for op in ops {
        match op {
            TokenizerOp::Push(s) => stack.push(*s),
            TokenizerOp::Pop => {
                stack.pop();
            }
            TokenizerOp::Newline => {
                lines.push(std::mem::take(&mut current));
                col = 0;
            }
            TokenizerOp::Content(c) => {
                let len = c.chars().count();
                let scopes = stack[1..].iter().map(|s| s.to_string()).collect();
                current.push((col, len, scopes));
                col += len;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    render_annotated(scope, input, &lines, scheme)
}

/// Collects the distinct scopes pushed in an op stream, as strings.
pub fn pushed_scopes(ops: &[TokenizerOp]) -> Vec<String> {
    ops.iter()
        .filter_map(|op| match op {
            TokenizerOp::Push(s) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}
