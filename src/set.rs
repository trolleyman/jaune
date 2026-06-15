use crate::selector::Selector;
use crate::syntax::{Pattern, SyntaxDefinition};
use crate::{Scope, ScopeParseError, Tokenizer};
use std::collections::HashMap;

/// Where an injection's patterns live within their owning grammar.
#[derive(Debug)]
enum InjectionSource {
    /// The grammar's own top-level patterns (an `injectionSelector` grammar).
    Whole,
    /// The `index`-th entry of the grammar's `injections` map.
    Internal(usize),
}

/// A registered injection: a selector plus a pointer to the patterns it injects.
#[derive(Debug)]
struct Injection {
    selector: Selector,
    owner: Scope,
    source: InjectionSource,
}

/// A registry of loaded syntax definitions.
///
/// This acts as the "Linker" context. When a syntax includes another syntax
/// (e.g., `include: source.json`), the tokenizer looks it up here.
#[derive(Default, Debug)]
pub struct SyntaxSet {
    /// Map of scope (e.g., `source.rust`) to the definition.
    definitions: HashMap<Scope, SyntaxDefinition>,
    /// Map of file extension (e.g., `rs`) to the main scope.
    extensions: HashMap<String, Scope>,
    /// Map of language name / alias (e.g., `rust`, `rs`) to the main scope.
    names: HashMap<String, Scope>,
    /// Registered injections, evaluated against the scope stack during tokenization.
    injections: Vec<Injection>,
}

impl SyntaxSet {
    /// Creates a new, empty syntax set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a syntax definition to the set, indexing it by scope, file extensions, and
    /// its `name`, and registering any injections it declares.
    pub fn add(&mut self, syntax: SyntaxDefinition) {
        for ext in &syntax.file_extensions {
            self.extensions.insert(ext.clone(), syntax.scope);
        }
        self.names
            .insert(syntax.name.to_ascii_lowercase(), syntax.scope);

        if let Some(sel) = &syntax.injection_selector {
            self.injections.push(Injection {
                selector: Selector::parse(sel),
                owner: syntax.scope,
                source: InjectionSource::Whole,
            });
        }
        for (i, (sel, _)) in syntax.injections.iter().enumerate() {
            self.injections.push(Injection {
                selector: Selector::parse(sel),
                owner: syntax.scope,
                source: InjectionSource::Internal(i),
            });
        }

        self.definitions.insert(syntax.scope, syntax);
    }

    /// Whether any injections are registered (a cheap gate for the tokenizer's per-scan
    /// injection check).
    pub(crate) fn has_injections(&self) -> bool {
        !self.injections.is_empty()
    }

    /// Returns the injected patterns whose selector matches `scopes` (the current scope
    /// stack), each paired with its priority and owning grammar.
    ///
    /// `active_grammars` is the set of grammars currently participating in the
    /// tokenization (the base grammar plus any embedded ones on the stack). A grammar's
    /// own `injections` map is only consulted when that grammar is active — mirroring
    /// vscode-textmate, where the `injections` map is part of a grammar's own rules and
    /// the cross-grammar mechanism is `injectionSelector`. Without this gate, an
    /// auxiliary grammar with a broad selector (e.g. es-tag-html's `L:source` →
    /// `invalid.illegal.bad-angle-bracket`) would pollute every `source.*` language.
    pub(crate) fn matching_injections(
        &self,
        scopes: &[String],
        active_grammars: &[Scope],
    ) -> Vec<(i8, &[Pattern], &SyntaxDefinition)> {
        let mut out = Vec::new();
        for inj in &self.injections {
            // `injectionSelector` (whole-grammar) injections apply across grammars; an
            // `injections`-map entry applies only when its owner is in play.
            if matches!(inj.source, InjectionSource::Internal(_))
                && !active_grammars.contains(&inj.owner)
            {
                continue;
            }
            if let Some(priority) = inj.selector.matches(scopes)
                && let Some(owner) = self.definitions.get(&inj.owner)
            {
                let patterns = match inj.source {
                    InjectionSource::Whole => owner.patterns.as_slice(),
                    InjectionSource::Internal(i) => match owner.injections.get(i) {
                        Some((_, p)) => p.as_slice(),
                        None => continue,
                    },
                };
                out.push((priority, patterns, owner));
            }
        }
        out
    }

    /// Registers an additional name/alias (e.g. `rs`, `js`) pointing at an existing
    /// scope. Used by the bundled grammars, whose aliases live in metadata rather than
    /// the grammar files themselves.
    pub fn add_alias(&mut self, alias: &str, scope: Scope) {
        self.names.insert(alias.to_ascii_lowercase(), scope);
    }

    /// Finds a syntax by its file extension (e.g., "rs").
    pub fn find_by_extension(&self, ext: &str) -> Option<&SyntaxDefinition> {
        self.extensions
            .get(ext)
            .and_then(|scope| self.definitions.get(scope))
    }

    /// Finds a syntax by its root scope (e.g., `source.rust`).
    pub fn find_by_scope(&self, scope: Scope) -> Option<&SyntaxDefinition> {
        self.definitions.get(&scope)
    }

    /// Finds a syntax by language name or alias (case-insensitive, e.g. `rust` or `rs`).
    pub fn find_by_name(&self, name: &str) -> Option<&SyntaxDefinition> {
        self.names
            .get(&name.to_ascii_lowercase())
            .and_then(|scope| self.definitions.get(scope))
    }

    /// Returns the number of syntax definitions in the set.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Returns `true` if the set contains no syntax definitions.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Iterates over the syntax definitions in the set.
    pub fn iter(&self) -> impl Iterator<Item = &SyntaxDefinition> {
        self.definitions.values()
    }

    /// Creates a [`Tokenizer`] for `text` starting in the grammar identified by `scope`,
    /// resolving cross-grammar includes against this set. Returns `None` if `scope` is
    /// not in the set.
    pub fn tokenizer<'a>(&'a self, text: &'a str, scope: Scope) -> Option<Tokenizer<'a>> {
        let syntax = self.find_by_scope(scope)?;
        Some(Tokenizer::new_in_set(text, syntax, self))
    }
}

/// Helper to parse a TextMate reference string.
///
/// Examples:
/// - `source.rust` -> `(Some(source.rust), None)`
/// - `#function` -> `(None, Some("function"))`
/// - `source.rust#function` -> `(Some(source.rust), Some("function"))`
/// - `$self` -> `(None, None)` (Special case, usually handled by caller)
pub fn parse_reference(s: &str) -> Result<(Option<Scope>, Option<&str>), ScopeParseError> {
    if s.starts_with('#') {
        // Local reference: "#rule_name"
        return Ok((None, Some(&s[1..])));
    }

    match s.split_once('#') {
        Some((scope_str, rule_name)) => {
            // External reference with rule: "source.rust#rule_name"
            let scope = Scope::new(scope_str)?;
            Ok((Some(scope), Some(rule_name)))
        }
        None => {
            // External reference root: "source.rust"
            if s == "$self" || s == "$base" {
                return Ok((None, None));
            }
            let scope = Scope::new(s)?;
            Ok((Some(scope), None))
        }
    }
}
