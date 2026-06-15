//! Snapshot fixtures for scope labelling.
//!
//! Each fixture tokenizes a small snippet with a real grammar and renders the resulting
//! scope stacks into a committed `.snap` file under `tests/fixtures/`. The snapshots are
//! generated *by the library* and then reviewed by hand; whenever the tokenizer logic
//! changes, regenerate them and eyeball the diff:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test --test fixtures
//! ```
//!
//! A run without `UPDATE_SNAPSHOTS` fails if the live output diverges from the committed
//! snapshot, so unreviewed labelling changes can't slip through.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{check_balanced, load_all, pushed_scopes, reconstruct, render};

/// `(id, grammar name, input)`. The `html_embedded` and `markdown_embedded` fixtures
/// deliberately mix languages to cover cross-grammar (embedded) tokenization.
const FIXTURES: &[(&str, &str, &str)] = &[
    ("json_basic", "json", r#"{
  "name": "jaune",
  "nums": [1, 2.5, true, null]
}
"#),
    ("css_basic", "css", r#"a.btn:hover {
  color: #fff;
  margin: 0 auto;
}
"#),
    ("scss_basic", "scss", r#"$accent: #336699;
.card {
  color: $accent;
  &:hover { opacity: 0.8; }
}
"#),
    ("html_embedded", "html", r#"<div id="x">
  <style>.x { color: red; }</style>
  <script>const n = 1;</script>
</div>
"#),
    ("xml_basic", "xml", r#"<?xml version="1.0"?>
<root attr="v">
  <!-- comment -->
  <item>text</item>
</root>
"#),
    ("markdown_embedded", "markdown", r#"# Title

Some **bold** and `code` text.

```js
const x = 42;
```

```python
def f(): return 1
```

- a list item
> a quote
"#),
    ("javascript_basic", "javascript", r#"import { x } from "m";
// a comment
const greet = (name) => `hi ${name}`;
class A extends B { run() { return 42; } }
"#),
    ("jsx_basic", "jsx", r#"const App = () => (
  <div className="app">
    <Button onClick={handle}>Go {count}</Button>
  </div>
);
"#),
    ("typescript_basic", "typescript", r#"interface User { id: number; name: string; }
const u: User = { id: 1, name: "a" };
function get<T>(x: T): T { return x; }
enum Color { Red, Green }
"#),
    ("tsx_basic", "tsx", r#"type Props = { count: number };
const C: React.FC<Props> = ({ count }) => {
  return <span title="c">{count}</span>;
};
"#),
    ("rust_basic", "rust", r#"use std::collections::HashMap;

/// doc comment
pub fn main() -> Result<(), Box<dyn Error>> {
    let xs: Vec<i32> = vec![1, 2, 3];
    let s = format!("n = {}", xs.len());
    Ok(())
}
"#),
    ("python_basic", "python", r#"import os
from typing import List

@decorator
def greet(name: str) -> str:
    """docstring"""
    return f"hello {name}"
"#),
    ("go_basic", "go", r#"package main

import "fmt"

func main() {
    xs := []int{1, 2, 3}
    fmt.Printf("len=%d\n", len(xs))
}
"#),
    ("java_basic", "java", r#"package app;

public class Main {
    public static void main(String[] args) {
        int n = 42;
        System.out.println("n=" + n);
    }
}
"#),
    ("c_basic", "c", r#"#include <stdio.h>

int main(void) {
    int n = 42;
    printf("n=%d\n", n);
    return 0;
}
"#),
    ("cpp_basic", "cpp", r#"#include <vector>

template <typename T>
T add(T a, T b) {
    auto v = std::vector<T>{a, b};
    return a + b;
}
"#),
    ("ruby_basic", "ruby", r#"require "set"

# a comment
class Greeter
  def greet(name)
    "hello #{name}"
  end
end
"#),
    ("swift_basic", "swift", r#"import Foundation

struct Greeter {
    let name: String
    func greet() -> String {
        return "hello \(name)"
    }
}
"#),
    ("yaml_basic", "yaml", r#"name: jaune
version: 0.1.0
deps:
  - serde
  - fancy-regex
nested: { a: 1, b: true }
"#),
    ("toml_basic", "toml", r#"[package]
name = "jaune"
version = "0.1.0"

[deps]
serde = { version = "1" }
"#),
    ("shell_basic", "shellscript", r#"#!/bin/bash
set -euo pipefail
NAME="world"
echo "hello ${NAME}"
for f in *.rs; do
  echo "$f"
done
"#),
    ("sql_basic", "sql", r#"-- a query
SELECT id, name
FROM users
WHERE age > 18
ORDER BY name DESC;
"#),
    ("lua_basic", "lua", r#"local function greet(name)
  -- a comment
  return "hello " .. name
end

print(greet("world"))
"#),
    ("csharp_basic", "csharp", r#"using System;

namespace App {
    public class Program {
        static void Main() {
            var n = 42;
            Console.WriteLine($"n={n}");
        }
    }
}
"#),
];

fn fixtures_dir() -> PathBuf {
    common::manifest_dir().join("tests/fixtures")
}

#[test]
fn scope_label_snapshots() {
    let Some(set) = load_all() else {
        eprintln!("assets/grammars missing — run `bun run package-grammars`; skipping");
        return;
    };
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let dir = fixtures_dir();
    fs::create_dir_all(&dir).unwrap();

    let mut mismatches: Vec<String> = Vec::new();

    for &(id, grammar, input) in FIXTURES {
        let def = set
            .find_by_name(grammar)
            .unwrap_or_else(|| panic!("fixture grammar `{grammar}` not found in assets"));
        let scope = def.scope;
        let ops: Vec<_> = set.tokenizer(input, scope).unwrap().collect();

        // Sanity: every fixture must round-trip and stay balanced regardless of snapshot.
        assert_eq!(reconstruct(&ops), input, "[{id}] reconstruction mismatch");
        check_balanced(&ops).unwrap_or_else(|e| panic!("[{id}] {e}"));

        let rendered = format!(
            "grammar: {scope}\n\n=== input ===\n{input}\n=== tokens ===\n{}",
            render(scope, &ops)
        );
        let snap_path = dir.join(format!("{id}.snap"));

        if update || !snap_path.exists() {
            fs::write(&snap_path, &rendered).unwrap();
            if !update {
                mismatches.push(format!("[{id}] snapshot created at {snap_path:?}; review & commit"));
            }
        } else {
            let existing = fs::read_to_string(&snap_path).unwrap();
            if existing != rendered {
                mismatches.push(format!(
                    "[{id}] snapshot mismatch. Re-run with UPDATE_SNAPSHOTS=1 and review the diff.\n--- live ---\n{rendered}"
                ));
            }
        }
    }

    assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
}

/// Focused assertions that don't depend on exact snapshot text: confirm the JSON grammar
/// applies the scopes we expect to a few representative tokens.
#[test]
fn json_scope_labels() {
    let Some(set) = load_all() else {
        eprintln!("assets/grammars missing; skipping");
        return;
    };
    let def = set.find_by_name("json").expect("json grammar");
    let scope = def.scope;
    let ops: Vec<_> = set
        .tokenizer("{\"k\": 12.5}", scope)
        .unwrap()
        .collect();

    let scopes = pushed_scopes(&ops);
    assert!(
        scopes.iter().any(|s| s.contains("constant.numeric")),
        "number should be labelled constant.numeric.*, got {scopes:?}"
    );
    assert!(
        scopes.iter().any(|s| s.contains("string")),
        "key should be labelled as a string, got {scopes:?}"
    );
    assert!(
        scopes
            .iter()
            .any(|s| s.contains("punctuation.definition.dictionary")
                || s.contains("meta.structure.dictionary")),
        "object braces should be labelled, got {scopes:?}"
    );
}
