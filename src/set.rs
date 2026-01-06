use crate::{Scope, ScopeParseError, syntax::SyntaxDefinition};
use std::collections::HashMap;

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
}

impl SyntaxSet {
    /// Creates a new, empty syntax set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a syntax definition to the set.
    pub fn add(&mut self, syntax: SyntaxDefinition) {
        for ext in &syntax.file_extensions {
            self.extensions.insert(ext.clone(), syntax.scope);
        }
        self.definitions.insert(syntax.scope, syntax);
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
