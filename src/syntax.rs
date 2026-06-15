use crate::Scope;
use std::{
    collections::HashMap,
    io::{BufReader, Read},
    path::Path,
};

#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plist parse error: {0}")]
    PlistParse(#[from] plist::Error),
    #[error("JSON parse error: {0}")]
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
    /// For an injection grammar: the scope selector controlling where its top-level
    /// [`patterns`](Self::patterns) are injected.
    pub injection_selector: Option<String>,
    /// Per-grammar injections: `(scope selector, patterns)` pairs from the grammar's
    /// `injections` field.
    pub injections: Vec<(String, Vec<Pattern>)>,
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

    /// Load from a TextMate `.tmLanguage` (XML property-list) file.
    pub fn from_plist_file<P: AsRef<Path>>(path: P) -> Result<Self, LoadError> {
        Self::from_plist_value(plist::Value::from_file(path)?)
    }

    /// Load from a TextMate `.tmLanguage` (XML property-list) reader.
    pub fn from_plist_reader<R: Read + std::io::Seek>(reader: R) -> Result<Self, LoadError> {
        Self::from_plist_value(plist::Value::from_reader(reader)?)
    }

    /// Load from an already-parsed property-list [`plist::Value`].
    ///
    /// The value is converted to a [`serde_json::Value`] and then run through the same
    /// loader as JSON grammars, so both formats share one deserialization path.
    pub fn from_plist_value(value: plist::Value) -> Result<Self, LoadError> {
        Self::from_json(plist_to_json(&value))
    }
}

/// Converts a property-list value into the equivalent JSON value, so `.tmLanguage` and
/// `.json` grammars can share a single deserialization path.
fn plist_to_json(value: &plist::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        plist::Value::String(s) => J::String(s.clone()),
        plist::Value::Boolean(b) => J::Bool(*b),
        plist::Value::Integer(i) => i
            .as_signed()
            .map(|n| J::Number(n.into()))
            .or_else(|| i.as_unsigned().map(|n| J::Number(n.into())))
            .unwrap_or(J::Null),
        plist::Value::Real(r) => serde_json::Number::from_f64(*r)
            .map(J::Number)
            .unwrap_or(J::Null),
        plist::Value::Array(a) => J::Array(a.iter().map(plist_to_json).collect()),
        plist::Value::Dictionary(d) => {
            J::Object(d.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect())
        }
        // Data / Date / Uid don't appear in grammar files; represent as null.
        _ => J::Null,
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
            #[serde(rename = "injectionSelector")]
            injection_selector: Option<String>,
            // Each injection value is a rule object (`{ "patterns": [...] }`), which
            // deserializes as a `Pattern` (typically `Pattern::Patterns`).
            injections: Option<HashMap<String, Pattern>>,
        }

        let helper = SyntaxDefinitionHelper::deserialize(deserializer)?;

        let name = match helper.name {
            Some(n) => n,
            None => helper
            .scope_name
            .rsplit('.')
            .next()
            .unwrap_or(&helper.scope_name)
            .to_string(),
        };

        let injections = helper
            .injections
            .unwrap_or_default()
            .into_iter()
            .map(|(selector, rule)| {
                let patterns = match rule {
                    Pattern::Patterns(ps) => ps,
                    other => vec![other],
                };
                (selector, patterns)
            })
            .collect();

        Ok(SyntaxDefinition {
            name,
            scope: Scope::new_lossy(&helper.scope_name),
            file_extensions: helper.file_types.unwrap_or_default(),
            repository: helper.repository.unwrap_or_default(),
            patterns: helper.patterns,
            injection_selector: helper.injection_selector,
            injections,
        })
    }
}

/// A scope name from a grammar, which may contain capture references like `$1`.
///
/// TextMate lets scope names interpolate captures from the match that produced them
/// (e.g. `meta.tag.$2.html`). Names without a `$` are resolved to a [`Scope`] once, at
/// load time; names with a `$` are kept as a template and resolved per-match.
#[derive(Debug, Clone)]
pub enum ScopeTemplate {
    /// A fixed scope, resolved at load time.
    Static(Scope),
    /// A template containing `$n` capture references, resolved per-match.
    Dynamic(String),
}

impl ScopeTemplate {
    /// Parses a raw scope name, deciding whether it needs per-match interpolation.
    ///
    /// Static scopes are parsed leniently (see [`Scope::new_lossy`]) so an over-long
    /// scope never sinks the whole grammar.
    pub fn parse(s: &str) -> Self {
        if s.contains('$') {
            ScopeTemplate::Dynamic(s.to_string())
        } else {
            ScopeTemplate::Static(Scope::new_lossy(s))
        }
    }
}

/// A single capture-group rule (`beginCaptures`/`endCaptures`/`captures` entry).
///
/// A capture may assign a scope `name` to its range, further tokenize its range with
/// nested `patterns`, or both.
#[derive(Debug, Clone, Default)]
pub struct Capture {
    /// The scope to apply to the captured range, if any.
    pub name: Option<ScopeTemplate>,
    /// Sub-patterns used to tokenize the captured range, if any.
    pub patterns: Vec<Pattern>,
}

// TODO: Use Regex here & precompile? Cache regexes somewhere? Save & load using
#[derive(Debug, Clone)]
pub enum Pattern {
    /// A simple regex match (e.g., a keyword).
    Match {
        regex: String,
        scope: Option<ScopeTemplate>,
        captures: HashMap<usize, Capture>,
    },
    /// A block that opens a new context (e.g., strings, comments).
    BeginEnd {
        /// The regex that begins this block.
        begin: String,
        /// The regex that ends this block.
        end: String,
        /// The scope to apply to the content inside this block.
        content_scope: Option<ScopeTemplate>,
        /// Captures for the `begin` regex
        begin_captures: HashMap<usize, Capture>,
        /// Captures for the `end` regex
        end_captures: HashMap<usize, Capture>,
        /// Patterns allowed inside this block
        patterns: Vec<Pattern>,
    },
    /// A block that opens with `begin` and stays open as long as a `while` regex matches
    /// at the start of each subsequent line (e.g. Markdown block quotes / list items).
    BeginWhile {
        /// The regex that begins this block.
        begin: String,
        /// The regex tested at the start of each following line to continue the block.
        while_regex: String,
        /// The scope to apply to the content inside this block.
        content_scope: Option<ScopeTemplate>,
        /// Captures for the `begin` regex.
        begin_captures: HashMap<usize, Capture>,
        /// Captures for the `while` regex.
        while_captures: HashMap<usize, Capture>,
        /// Patterns allowed inside this block.
        patterns: Vec<Pattern>,
    },
    /// A reference to another rule (e.g., { include = "#function" })
    Include(String),
    /// A bare group of sub-patterns with no `match`/`begin`/`end`/`include` of its
    /// own (e.g. a repository entry that just lists `patterns`). When matched against,
    /// it behaves as if its sub-patterns were spliced into the surrounding list.
    Patterns(Vec<Pattern>),
}

impl<'d> serde::Deserialize<'d> for Pattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'d>,
    {
        /// A single capture entry. Usually `{ "name": "..." }` or `{ "patterns": [...] }`,
        /// but some grammars use the string shorthand `"scope.name"` or wrap the entry in
        /// a one-element array.
        #[derive(serde::Deserialize, Debug)]
        #[serde(untagged)]
        enum CaptureHelper {
            Name(String),
            List(Vec<CaptureHelper>),
            Full {
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                patterns: Option<Vec<Pattern>>,
            },
        }

        /// A capture set, keyed by group number — written either as a map (`{"0": ...}`)
        /// or, in some grammars, as an array (index = group number).
        #[derive(serde::Deserialize, Debug)]
        #[serde(untagged)]
        enum Captures {
            Map(HashMap<String, CaptureHelper>),
            Seq(Vec<CaptureHelper>),
        }

        #[derive(serde::Deserialize, Debug)]
        struct PatternHelper {
            #[serde(rename = "match")]
            match_regex: Option<String>,
            begin: Option<String>,
            end: Option<String>,
            #[serde(rename = "while")]
            while_regex: Option<String>,
            name: Option<String>,
            captures: Option<Captures>,
            #[serde(rename = "beginCaptures")]
            begin_captures: Option<Captures>,
            #[serde(rename = "endCaptures")]
            end_captures: Option<Captures>,
            #[serde(rename = "whileCaptures")]
            while_captures: Option<Captures>,
            patterns: Option<Vec<Pattern>>,
            include: Option<String>,
        }

        /// A pattern may be written as a rule object or, in some grammars, as a bare array
        /// of sub-patterns (e.g. a repository entry that is a list).
        #[derive(serde::Deserialize, Debug)]
        #[serde(untagged)]
        enum PatternRepr {
            List(Vec<Pattern>),
            Rule(PatternHelper),
        }

        /// Flattens a (possibly array-wrapped) capture entry into a `(name, patterns)` pair.
        fn capture_parts(helper: CaptureHelper) -> (Option<ScopeTemplate>, Vec<Pattern>) {
            match helper {
                CaptureHelper::Name(s) => (Some(ScopeTemplate::parse(&s)), Vec::new()),
                CaptureHelper::Full { name, patterns } => (
                    name.as_deref().map(ScopeTemplate::parse),
                    patterns.unwrap_or_default(),
                ),
                CaptureHelper::List(list) => list
                    .into_iter()
                    .next()
                    .map(capture_parts)
                    .unwrap_or((None, Vec::new())),
            }
        }

        /// Converts a raw capture set (map or array) into a `group index -> Capture` map,
        /// skipping entries with non-numeric keys or with neither a `name` nor `patterns`.
        fn convert_captures(raw: Option<Captures>) -> HashMap<usize, Capture> {
            let entries: Vec<(usize, CaptureHelper)> = match raw {
                None => Vec::new(),
                Some(Captures::Map(m)) => m
                    .into_iter()
                    .filter_map(|(k, v)| k.parse::<usize>().ok().map(|i| (i, v)))
                    .collect(),
                Some(Captures::Seq(s)) => s.into_iter().enumerate().collect(),
            };
            let mut out = HashMap::new();
            for (idx, helper) in entries {
                let (name, patterns) = capture_parts(helper);
                if name.is_some() || !patterns.is_empty() {
                    out.insert(idx, Capture { name, patterns });
                }
            }
            out
        }

        let helper = match PatternRepr::deserialize(deserializer)? {
            PatternRepr::List(patterns) => return Ok(Pattern::Patterns(patterns)),
            PatternRepr::Rule(helper) => helper,
        };
        let content_scope = helper.name.as_deref().map(ScopeTemplate::parse);

        if let Some(include) = helper.include {
            Ok(Pattern::Include(include))
        } else if let Some(match_regex) = helper.match_regex {
            Ok(Pattern::Match {
                regex: match_regex,
                scope: content_scope,
                captures: convert_captures(helper.captures),
            })
        } else if let Some(begin) = helper.begin {
            if let Some(end) = helper.end {
                Ok(Pattern::BeginEnd {
                    begin,
                    end,
                    content_scope,
                    begin_captures: convert_captures(helper.begin_captures),
                    end_captures: convert_captures(helper.end_captures),
                    patterns: helper.patterns.unwrap_or_default(),
                })
            } else if let Some(while_regex) = helper.while_regex {
                Ok(Pattern::BeginWhile {
                    begin,
                    while_regex,
                    content_scope,
                    begin_captures: convert_captures(helper.begin_captures),
                    while_captures: convert_captures(helper.while_captures),
                    patterns: helper.patterns.unwrap_or_default(),
                })
            } else {
                // `begin` with neither `end` nor `while` is malformed; treat it as a plain
                // match on the begin regex so the grammar still loads.
                Ok(Pattern::Match {
                    regex: begin,
                    scope: content_scope,
                    captures: convert_captures(helper.begin_captures),
                })
            }
        } else if let Some(patterns) = helper.patterns {
            Ok(Pattern::Patterns(patterns))
        } else {
            // Unrecognized pattern shape (e.g. `{}` or comment-only): treat as a no-op so
            // one odd entry doesn't sink the whole grammar.
            Ok(Pattern::Patterns(Vec::new()))
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

    const SIMPLE_SYNTAX_PLIST: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>scopeName</key>
    <string>source.abc</string>
    <key>name</key>
    <string>ABC</string>
    <key>fileTypes</key>
    <array><string>abc</string></array>
    <key>patterns</key>
    <array>
        <dict>
            <key>match</key>
            <string>a|b|c</string>
            <key>name</key>
            <string>keyword.letter</string>
            <key>captures</key>
            <dict>
                <key>0</key>
                <dict><key>name</key><string>meta.letter</string></dict>
            </dict>
        </dict>
    </array>
</dict>
</plist>"##;

    #[test]
    pub fn test_load_from_plist() {
        use std::io::Cursor;
        let syntax = super::SyntaxDefinition::from_plist_reader(Cursor::new(SIMPLE_SYNTAX_PLIST))
            .expect("Failed to load plist syntax");
        assert_eq!(syntax.name, "ABC");
        assert_eq!(syntax.scope.to_string(), "source.abc");
        assert_eq!(syntax.file_extensions, vec!["abc".to_string()]);
        assert_eq!(syntax.patterns.len(), 1);
        // The capture (with a string-keyed dict) must survive the plist->json conversion.
        match &syntax.patterns[0] {
            super::Pattern::Match { captures, .. } => assert!(captures.contains_key(&0)),
            other => panic!("expected a match pattern, got {other:?}"),
        }
    }

    #[test]
    pub fn test_tokenize_simple_sample() {
        use crate::{Scope, Tokenizer, TokenizerOp};

        let syntax = super::SyntaxDefinition::from_json_str(SIMPLE_SYNTAX_JSON)
            .expect("Failed to load syntax");
        let ops: Vec<_> = Tokenizer::new(SIMPLE_SYNTAX_SAMPLE, &syntax).collect();

        // The op stream must reproduce the input exactly, with balanced Push/Pop.
        let mut reconstructed = String::new();
        let mut depth = 0i32;
        for op in &ops {
            match op {
                TokenizerOp::Content(c) => reconstructed.push_str(c),
                TokenizerOp::Newline => reconstructed.push('\n'),
                TokenizerOp::Push(_) => depth += 1,
                TokenizerOp::Pop => {
                    depth -= 1;
                    assert!(depth >= 0, "Pop without matching Push");
                }
            }
        }
        assert_eq!(reconstructed, SIMPLE_SYNTAX_SAMPLE);
        assert_eq!(depth, 0, "unbalanced Push/Pop");

        // The letters `a`, `b`, `c` should be scoped as `keyword.letter`, and the
        // parentheses should open `expression.group` blocks.
        let letter = Scope::new("keyword.letter").unwrap();
        let group = Scope::new("expression.group").unwrap();
        assert!(ops.contains(&TokenizerOp::Push(letter)));
        assert!(ops.contains(&TokenizerOp::Push(group)));
    }
}
