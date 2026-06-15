//! Broad smoke test: tokenize every upstream sample with its grammar and assert the
//! tokenizer doesn't panic, that the output round-trips back to the input, and that
//! `Push`/`Pop` ops stay balanced.
//!
//! This exercises the whole bundled grammar set (including embedded/cross-grammar
//! includes) against real source files. It is skipped gracefully when the generated
//! assets or the submodule samples are unavailable.

mod common;

use std::fs;

use common::{check_balanced, load_all, reconstruct, samples_dir};

#[test]
fn smoke_all_samples() {
    let Some(set) = load_all() else {
        eprintln!("assets/grammars missing — run `bun run package-grammars`; skipping");
        return;
    };
    let samples = samples_dir();
    if !samples.exists() {
        eprintln!("submodule samples missing — run `bun run update-submodules`; skipping");
        return;
    }

    let mut tested = 0usize;
    let mut failures: Vec<String> = Vec::new();

    let mut paths: Vec<_> = fs::read_dir(&samples)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sample"))
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let Some(def) = set.find_by_name(&name) else {
            continue; // sample with no matching grammar (or an injection-only language)
        };
        let scope = def.scope;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue, // non-UTF-8 sample
        };

        let ops: Vec<_> = set.tokenizer(&text, scope).unwrap().collect();

        if reconstruct(&ops) != text {
            failures.push(format!("{name}: tokenized output does not reconstruct the input"));
        }
        if let Err(e) = check_balanced(&ops) {
            failures.push(format!("{name}: {e}"));
        }
        tested += 1;
    }

    eprintln!("smoke: tokenized {tested} samples across the grammar set");
    assert!(
        failures.is_empty(),
        "{} sample(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(tested > 100, "expected to test many samples, only got {tested}");
}
