//! Annotated-sample snapshots.
//!
//! Each upstream sample (`textmate-grammars-themes/samples/*.sample`) plus any extra inputs we
//! add (`tests/samples/extra-input/*.sample`) is tokenized with its real grammar and rendered into
//! a committed `tests/samples/jaune/<file>` file: the original source with the nested scope
//! regions woven in beneath each line as comments. See `tests/samples/README.md` for the format.
//!
//! When a reference tokenization is available (`tests/samples/.reference-tokens/<file>.json`,
//! produced by `bun run scripts/reference-tokens.ts` using vscode-textmate), the *same* renderer
//! also writes `tests/samples/textmate-grammars-themes/<file>` — so `git diff` between the two
//! directories shows exactly where jaune diverges from the canonical VS Code tokenizer.
//!
//! Regenerate and eyeball the diff whenever the tokenizer changes:
//!
//! ```sh
//! bun run scripts/reference-tokens.ts          # optional: refresh the reference side
//! UPDATE_SNAPSHOTS=1 cargo test --test samples
//! ```

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{
    LineToken, check_balanced, load_all, reconstruct, render_annotated, render_sample, scheme_for,
};

fn samples_root() -> PathBuf {
    common::manifest_dir().join("tests/samples")
}

/// Collects `(grammar name, output file name, source)` for every input, from the upstream samples
/// and our extra inputs. The grammar name is the first `.`-separated segment of the file name
/// (`bat.sample` → `bat`, `html.embedded.sample` → `html`); the output keeps the full name.
fn collect_inputs() -> Vec<(String, String, String)> {
    let mut inputs = Vec::new();
    let dirs = [
        common::manifest_dir().join("textmate-grammars-themes/samples"),
        samples_root().join("extra-input"),
    ];
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sample"))
            .collect();
        paths.sort();
        for path in paths {
            let file = path.file_name().unwrap().to_str().unwrap().to_string();
            let name = file.split('.').next().unwrap().to_string();
            let Ok(source) = fs::read_to_string(&path) else {
                continue; // non-UTF-8 sample
            };
            inputs.push((name, file, source));
        }
    }
    inputs
}

/// Reads `tests/samples/.reference-tokens/<file>.json` if present: a `[[ [start, len, [scopes…] ] …] …]`
/// array of lines of tokens, with the grammar root already stripped from every scope list.
fn load_reference(path: &Path) -> Option<Vec<Vec<LineToken>>> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let lines = value
        .as_array()?
        .iter()
        .map(|line| {
            line.as_array()
                .unwrap()
                .iter()
                .map(|tok| {
                    let t = tok.as_array().unwrap();
                    let start = t[0].as_u64().unwrap() as usize;
                    let len = t[1].as_u64().unwrap() as usize;
                    let scopes = t[2]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_string())
                        .collect();
                    (start, len, scopes)
                })
                .collect()
        })
        .collect();
    Some(lines)
}

/// Writes `rendered` to `path` when updating, otherwise records a mismatch if it diverges.
fn check_or_write(path: &Path, rendered: &str, update: bool, mismatches: &mut Vec<String>) {
    if update || !path.exists() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, rendered).unwrap();
        if !update {
            mismatches.push(format!("{path:?} created; review & commit"));
        }
    } else if fs::read_to_string(path).unwrap() != rendered {
        mismatches.push(format!(
            "{path:?} diverged. Re-run with UPDATE_SNAPSHOTS=1 and review the diff."
        ));
    }
}

#[test]
fn annotated_sample_snapshots() {
    let Some(set) = load_all() else {
        eprintln!("assets/grammars missing — run `bun run package-grammars`; skipping");
        return;
    };
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let root = samples_root();
    let jaune_dir = root.join("jaune");
    let reference_tokens = root.join(".reference-tokens");
    let reference_dir = root.join("textmate-grammars-themes");

    let mut mismatches: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut generated = 0usize;
    let mut referenced = 0usize;

    for (name, file, source) in collect_inputs() {
        let Some(def) = set.find_by_name(&name) else {
            skipped.push(format!("{file}: no grammar named `{name}`"));
            continue;
        };
        let scope = def.scope;
        let ops: Vec<_> = set.tokenizer(&source, scope).unwrap().collect();

        // Skip (don't fail) on upstream tokenization quirks — `smoke.rs` is the hard guarantee.
        if reconstruct(&ops) != source {
            skipped.push(format!("{file}: does not reconstruct the input"));
            continue;
        }
        if let Err(e) = check_balanced(&ops) {
            skipped.push(format!("{file}: {e}"));
            continue;
        }

        let scheme = scheme_for(&name);
        let rendered = render_sample(scope, &ops, &scheme, &source);
        check_or_write(&jaune_dir.join(&file), &rendered, update, &mut mismatches);
        generated += 1;

        // Render the reference side through the same formatter, when its tokens are available.
        let tokens_path = reference_tokens.join(format!("{file}.json"));
        if let Some(lines) = load_reference(&tokens_path) {
            let rendered_ref = render_annotated(scope, &source, &lines, &scheme);
            check_or_write(&reference_dir.join(&file), &rendered_ref, update, &mut mismatches);
            referenced += 1;
        }
    }

    eprintln!(
        "samples: generated {generated} jaune snapshots ({referenced} with a reference), \
         skipped {}",
        skipped.len()
    );
    if !skipped.is_empty() {
        eprintln!("skipped:\n  {}", skipped.join("\n  "));
    }
    assert!(
        mismatches.is_empty(),
        "{} snapshot(s) diverged:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
