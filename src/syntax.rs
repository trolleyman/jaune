use crate::Scope;
use std::{
    collections::HashMap,
    io::{BufReader, Read},
    path::Path,
};

#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    #[error("IO error")]
    Io(#[from] std::io::Error),
    // #[error("plist parse error")]
    // PlistParse(#[from] plist::Error),
    #[error("JSON parse error")]
    JsonParse(#[from] serde_json::Error),
}

/// A complete language definition (e.g., "Rust").
///
/// ```json
/// {
///     "scopeName": "source.abc",
///     "patterns": [{ "include": "#expression" }],
///     "repository": {
///         "expression": {
///             "patterns": [{ "include": "#letter" }, { "include": "#paren-expression" }]
///         },
///         "letter": {
///             "match": "a|b|c",
///             "name": "keyword.letter"
///         },
///         "paren-expression": {
///             "begin": "\\(",
///             "end": "\\)",
///             "beginCaptures": {
///                 "0": { "name": "punctuation.paren.open" }
///             },
///             "endCaptures": {
///                 "0": { "name": "punctuation.paren.close" }
///             },
///             "name": "expression.group",
///             "patterns": [{ "include": "#expression" }]
///         }
///     }
/// }
/// ```
// TODO: Implement loading from .messagePack files?
#[derive(Debug)]
pub struct SyntaxDefinition {
    /// The name of this syntax (e.g., "Rust"). If this is not provided in the source file,
    /// the last atom of the scope name is used as a fallback.
    pub name: String,
    /// The root scope of this syntax (e.g., `source.rust`).
    pub scope: Scope,
    /// File extensions associated with this syntax (e.g., `rs`, `toml`).
    pub file_extensions: Vec<String>,
    /// The "repository" of reusable patterns (defines 'include' targets).
    pub repository: HashMap<String, Pattern>,
    /// The top-level patterns to match.
    pub patterns: Vec<Pattern>,
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
        Ok(serde_json::from_value(value)?)
    }

    /// Load from a TextMate .tmLanguage (property-list) file
    pub fn from_plist_file<P: AsRef<Path>>(_path: P) -> Result<Self, LoadError> {
        todo!()
    }
}

impl<'d> serde::Deserialize<'d> for SyntaxDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'d>,
    {
        #[derive(serde::Deserialize, Debug)]
        struct SyntaxDefinitionHelper {
            #[serde(rename = "scopeName")]
            scope_name: String,
            name: Option<String>,
            #[serde(rename = "fileTypes")]
            file_types: Option<Vec<String>>,
            repository: Option<HashMap<String, Pattern>>,
            patterns: Vec<Pattern>,
        }

        let helper = SyntaxDefinitionHelper::deserialize(deserializer)?;
        dbg!(&helper);

        let name = match helper.name {
            Some(n) => n,
            None => helper
            .scope_name
            .rsplit('.')
            .next()
            .unwrap_or(&helper.scope_name)
            .to_string(),
        };

        Ok(SyntaxDefinition {
            name,
            scope: Scope::new(&helper.scope_name).map_err(serde::de::Error::custom)?,
            file_extensions: helper.file_types.unwrap_or_default(),
            repository: helper.repository.unwrap_or_default(),
            patterns: helper.patterns,
        })
    }
}

/// The fundamental building block of a grammar.
// TODO: Use Regex here & precompile? Cache regexes somewhere? Save & load using
#[derive(Debug, Clone)]
pub enum Pattern {
    /// A simple regex match (e.g., a keyword).
    Match {
        regex: String,
        scope: Option<Scope>,
        captures: HashMap<usize, Scope>,
    },
    /// A block that opens a new context (e.g., strings, comments).
    BeginEnd {
        /// The regex that begins this block.
        begin: String,
        /// The regex that ends this block.
        end: String,
        /// The scope to apply to the content inside this block.
        content_scope: Option<Scope>,
        /// Captures for the `begin` regex
        begin_captures: HashMap<usize, Scope>,
        /// Captures for the `end` regex
        end_captures: HashMap<usize, Scope>,
        /// Patterns allowed inside this block
        patterns: Vec<Pattern>,
    },
    /// A reference to another rule (e.g., { include = "#function" })
    Include(String),
}

impl<'d> serde::Deserialize<'d> for Pattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'d>,
    {
        #[derive(serde::Deserialize, Debug)]
        struct PatternHelper {
            #[serde(rename = "match")]
            match_regex: Option<String>,
            begin: Option<String>,
            end: Option<String>,
            name: Option<String>,
            captures: Option<HashMap<usize, String>>,
            #[serde(rename = "beginCaptures")]
            begin_captures: Option<HashMap<usize, String>>,
            #[serde(rename = "endCaptures")]
            end_captures: Option<HashMap<usize, String>>,
            patterns: Option<Vec<Pattern>>,
            include: Option<String>,
        }

        let helper = PatternHelper::deserialize(deserializer)?;
        dbg!(&helper);

        if let Some(include) = helper.include {
            Ok(Pattern::Include(include))
        } else if let Some(match_regex) = helper.match_regex {
            let captures: Result<HashMap<_, _>, _> = helper
                .captures
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| Ok((k, Scope::new(&v).map_err(serde::de::Error::custom)?)))
                .collect();
            Ok(Pattern::Match {
                regex: match_regex,
                scope: match helper.name {
                    Some(name) => Some(Scope::new(&name).map_err(serde::de::Error::custom)?),
                    None => None,
                },
                captures: captures?,
            })
        } else if let (Some(begin), Some(end)) = (helper.begin, helper.end) {
            let begin_captures: Result<HashMap<_, _>, _> = helper
                .begin_captures
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| Ok((k, Scope::new(&v).map_err(serde::de::Error::custom)?)))
                .collect();
            let begin_captures = begin_captures?;
            let end_captures: Result<HashMap<_, _>, _> = helper
                .end_captures
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| Ok((k, Scope::new(&v).map_err(serde::de::Error::custom)?)))
                .collect();
            let end_captures = end_captures?;
            Ok(Pattern::BeginEnd {
                begin,
                end,
                content_scope: match helper.name {
                    Some(name) => Some(Scope::new(&name).map_err(serde::de::Error::custom)?),
                    None => None,
                },
                begin_captures,
                end_captures,
                patterns: helper.patterns.unwrap_or_default(),
            })
        } else {
            Err(serde::de::Error::custom(
                "Invalid pattern: patterns must have one of: 'match', 'begin'/'end' pair, or 'include'",
            ))
        }
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
    pub fn test_load_simple_from_json() {
        let syntax = super::SyntaxDefinition::from_json_str(SIMPLE_SYNTAX_JSON)
            .expect("Failed to load syntax");
        assert_eq!(syntax.name, "abc");
        assert_eq!(syntax.scope.to_string(), "source.abc");
        assert_eq!(syntax.patterns.len(), 1);
        assert!(syntax.repository.contains_key("expression"));
        assert!(syntax.repository.contains_key("letter"));
        assert!(syntax.repository.contains_key("paren-expression"));
    }

    #[test]
    #[ignore]
    pub fn test_tokenize_simple_sample() {
        let syntax = super::SyntaxDefinition::from_json_str(SIMPLE_SYNTAX_JSON)
            .expect("Failed to load syntax");
        todo!("{:?}\n{}", syntax, SIMPLE_SYNTAX_SAMPLE);
    }
}
