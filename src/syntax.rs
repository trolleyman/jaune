use crate::Scope;
use std::{
    collections::HashMap,
    io::{BufReader, Cursor, Read},
    path::Path,
};

#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("plist parse error")]
    PlistParse(#[from] plist::Error),
    #[error("JSON parse error")]
    JsonParse(#[from] serde_json::Error),
}

/// A complete language definition (e.g., "Rust").
#[derive(Debug)]
pub struct SyntaxDefinition {
    pub name: String,
    pub scope: Scope,
    pub file_extensions: Vec<String>,
    /// The "repository" of reusable patterns (defines 'include' targets).
    pub repository: HashMap<String, Pattern>,
    /// The top-level patterns to match.
    pub patterns: Vec<Pattern>,
}

/// The fundamental building block of a grammar.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// A simple regex match (e.g., a keyword).
    Match {
        regex: String, // Compiled later to save startup time? Or use Regex here.
        scope: Option<Scope>,
        captures: HashMap<usize, Scope>, // e.g. group 1 is 'variable', group 2 is 'type'
    },
    /// A block that opens a new context (e.g., strings, comments).
    BeginEnd {
        begin: String,
        end: String, // Note: This often contains backreferences like \1 !
        content_scope: Option<Scope>,
        begin_captures: HashMap<usize, Scope>,
        end_captures: HashMap<usize, Scope>,
        /// Patterns allowed inside this block
        patterns: Vec<Pattern>,
    },
    /// A reference to another rule (e.g., { include = "#function" })
    Include(String),
}

impl SyntaxDefinition {
    /// Load from a .json / .tm-language file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, LoadError> {
        let path = path.as_ref();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            Self::from_json_file(path)
        } else if path.extension().and_then(|s| s.to_str()) == Some("tmLanguage") {
            Self::from_plist_file(path)
        } else {
            Self::from_plist_file(path).or_else(|_| Self::from_json_file(path))
        }
    }

    /// Load from a TextMate .json file
    pub fn from_json_file<P: AsRef<Path>>(path: P) -> Result<Self, LoadError> {
        Self::from_json(serde_json::from_reader(BufReader::new(
            std::fs::File::open(path)?,
        ))?)
    }

    /// Load from a TextMate JSON slice
    pub fn from_json_slice(v: &[u8]) -> Result<Self, LoadError> {
        Self::from_json(serde_json::from_slice(v)?)
    }

    /// Load from a TextMate JSON string
    pub fn from_json_str(s: &str) -> Result<Self, LoadError> {
        Self::from_json(serde_json::from_str(s)?)
    }

    /// Load from a TextMate JSON reader
    pub fn from_json_reader<R: Read>(reader: R) -> Result<Self, LoadError> {
        Self::from_json(serde_json::from_reader(reader)?)
    }

    /// Load from a TextMate JSON value
    pub fn from_json(value: serde_json::Value) -> Result<Self, LoadError> {
        todo!()
    }

    /// Load from a .tmLanguage (plist) file
    pub fn from_plist_file<P: AsRef<Path>>(path: P) -> Result<Self, LoadError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    pub const SIMPLE_SYNTAX_JSON: &'static str = r###"
    {
        "scopeName": "source.abc",
        "patterns": [{ "include": "#expression" }],
        "repository": {
            "expression": {
            "patterns": [{ "include": "#letter" }, { "include": "#paren-expression" }]
            },
            "letter": {
            "match": "a|b|c",
            "name": "keyword.letter"
            },
            "paren-expression": {
            "begin": "\\(",
            "end": "\\)",
            "beginCaptures": {
                "0": { "name": "punctuation.paren.open" }
            },
            "endCaptures": {
                "0": { "name": "punctuation.paren.close" }
            },
            "name": "expression.group",
            "patterns": [{ "include": "#expression" }]
            }
        }
    }
    "###;
    pub const SIMPLE_SYNTAX_SAMPLE: &'static str = r###"
    a
    (
        b
    )
    x
    (
        (
            c
            xyz
        )
    )
    (
    a
    "###;

    #[test]
    pub fn load_simple_from_json() {
    }
}
