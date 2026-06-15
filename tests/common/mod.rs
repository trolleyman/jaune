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

/// Renders an op stream as a human-readable, reviewable scope listing: one line per
/// content token showing the escaped text and the full scope stack (rooted at `scope`).
pub fn render(scope: Scope, ops: &[TokenizerOp]) -> String {
    let mut out = String::new();
    let mut stack: Vec<Scope> = vec![scope];
    for op in ops {
        match op {
            TokenizerOp::Push(s) => stack.push(*s),
            TokenizerOp::Pop => {
                stack.pop();
            }
            TokenizerOp::Newline => out.push_str("⏎\n"),
            TokenizerOp::Content(c) => {
                let scopes = stack
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&format!("{c:<24?} {scopes}\n"));
            }
        }
    }
    out
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
