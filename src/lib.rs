pub use atom::{ATOM_REPO, Atom, AtomParseError, AtomRepository};
pub use scope::{MAX_ATOMS, Scope, SimpleScopeStack, ScopeParseError, ScopeStack};
pub use set::{SyntaxSet, parse_reference};
pub use syntax::{LoadError, Pattern, ScopeTemplate, SyntaxDefinition};
pub use tokenizer::{Tokenizer, TokenizerOp};

#[cfg(feature = "_bundled")]
pub use bundled::BundledGrammar;

mod atom;
mod scope;
mod selector;
mod set;
mod syntax;
mod tokenizer;

#[cfg(feature = "_bundled")]
mod bundled;
#[cfg(feature = "_bundled")]
mod bundled_generated;
