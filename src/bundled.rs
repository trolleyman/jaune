//! Compile-time bundled grammars.
//!
//! This module is only compiled when at least one grammar feature is enabled (each
//! `grammar-<name>` feature, as well as the `top`, `all`, and category bundles, turns on
//! the internal `_bundled` feature that gates this module).
//!
//! The grammar table itself lives in the generated [`crate::bundled_generated`] module
//! and is produced by `scripts/package-grammars.ts`.

use crate::{SyntaxDefinition, SyntaxSet};

/// A grammar embedded into the binary at compile time via `include_str!`.
pub struct BundledGrammar {
    /// The canonical language name (e.g. `rust`).
    pub name: &'static str,
    /// The root scope name (e.g. `source.rust`).
    pub scope: &'static str,
    /// Alternate names / aliases for the language (e.g. `rs`).
    pub aliases: &'static [&'static str],
    /// The raw grammar JSON.
    pub json: &'static str,
}

impl SyntaxSet {
    /// Builds a [`SyntaxSet`] containing every grammar enabled via Cargo features,
    /// indexed by scope, name, and aliases.
    ///
    /// Grammars that fail to parse are skipped rather than panicking, so a single
    /// malformed grammar can't take down the whole set. Use [`SyntaxSet::tokenizer`] on
    /// the result to tokenize with cross-grammar (embedded language) support.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "grammar-json")] {
    /// use jaune::{Scope, SyntaxSet};
    /// let set = SyntaxSet::bundled();
    /// let json = set.find_by_name("json").expect("json grammar enabled");
    /// assert_eq!(json.scope, Scope::new("source.json").unwrap());
    /// # }
    /// ```
    pub fn bundled() -> Self {
        let mut set = SyntaxSet::new();
        for g in crate::bundled_generated::grammars() {
            if let Ok(def) = SyntaxDefinition::from_json_str(g.json) {
                let scope = def.scope;
                set.add(def);
                for alias in g.aliases {
                    set.add_alias(alias, scope);
                }
            }
        }
        set
    }
}
