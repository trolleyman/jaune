# `jaune`

**A fast and lightweight TextMate grammar parser and tokenizer in Rust.**

`jaune` loads TextMate grammars (the JSON flavour used by VS Code / Shiki) and turns
source text into a stream of scope-annotated tokens. It is a *tokenizer and parser only* —
colour themes are deliberately out of scope and left to downstream libraries.

## Overview

The crate is built from a few small pieces:

| Module      | Responsibility                                                             |
| ----------- | ------------------------------------------------------------------------- |
| `atom`      | Interns scope segments (`function`, `rust`, …) into 16-bit [`Atom`]s.      |
| `scope`     | Bit-packs up to 16 atoms into a [`Scope`] (`meta.function.rust`).          |
| `syntax`    | Loads a grammar (`SyntaxDefinition`) and its `Pattern`s from JSON.         |
| `set`       | `SyntaxSet` — a registry that links grammars together for includes.        |
| `tokenizer` | The line-based engine, emitting `Push`/`Pop`/`Content`/`Newline` ops.      |

## Usage

### Tokenizing with a single grammar

```rust
use jaune::{SyntaxDefinition, Tokenizer, TokenizerOp};

let grammar = SyntaxDefinition::from_json_str(json_src)?;
for op in Tokenizer::new("let x = 1;", &grammar) {
    match op {
        TokenizerOp::Push(scope) => { /* enter scope */ }
        TokenizerOp::Pop         => { /* leave scope  */ }
        TokenizerOp::Content(t)  => { /* text `t` under the current scope stack */ }
        TokenizerOp::Newline     => { /* line break */ }
    }
}
```

The tokenizer does **not** track the scope stack for you — it emits `Push`/`Pop`
operations and you maintain a `Vec<Scope>` if you need the full context of each token.

### Cross-grammar / embedded languages

Languages embedded in other languages (CSS-in-HTML, fenced code blocks in Markdown, …)
are resolved through a [`SyntaxSet`]:

```rust
use jaune::SyntaxSet;

let mut set = SyntaxSet::new();
set.add(SyntaxDefinition::from_json_str(html)?);
set.add(SyntaxDefinition::from_json_str(css)?);

let scope = set.find_by_name("html").unwrap().scope;
for op in set.tokenizer(source, scope).unwrap() { /* ... */ }
```

## Bundled grammars

Over 240 grammars from
[`textmate-grammars-themes`](https://github.com/shikijs/textmate-grammars-themes) can be
embedded directly into your binary. **Bundling is opt-in via Cargo features** so you only
pay for the languages you use:

```toml
[dependencies]
# A few specific languages (embedded deps are pulled in automatically):
jaune = { version = "0.1", features = ["grammar-rust", "grammar-json"] }

# Or a curated set of popular languages:
jaune = { version = "0.1", features = ["top"] }

# Or a category bundle (web / markup / general / scripting / data / dsl / …):
jaune = { version = "0.1", features = ["web"] }

# Or absolutely everything:
jaune = { version = "0.1", features = ["all"] }
```

Then build the set at runtime:

```rust
let set = jaune::SyntaxSet::bundled();
let scope = set.find_by_name("rust").unwrap().scope; // or by alias, e.g. "rs"
```

Enabling `grammar-vue` automatically enables `grammar-html`, `grammar-css`,
`grammar-javascript`, … so embedded languages tokenize correctly.

## Tooling (Bun scripts)

| Command                        | Description                                                     |
| ------------------------------ | --------------------------------------------------------------- |
| `bun run update-submodules`    | Update the upstream grammars submodule.                         |
| `bun run package-grammars`     | Copy grammars into `assets/`, regenerate the bundle + features. |
| `bun run release [-- --dry-run]` | Package grammars, test, and `cargo publish`.                  |

The grammar JSON lives in the `textmate-grammars-themes` submodule. `package-grammars`
copies it into `assets/grammars/`, generates `src/bundled_generated.rs`, and rewrites the
`[features]` block of `Cargo.toml`. The upstream `LICENSE`/`NOTICE` are copied alongside
the assets for attribution.

## Testing

```sh
cargo test                              # unit, doc, snapshot, and smoke tests
cargo test --features grammar-json      # also exercise the bundled-grammar path
UPDATE_SNAPSHOTS=1 cargo test --test fixtures   # regenerate scope-label snapshots
```

- `tests/smoke.rs` tokenizes every upstream sample with its grammar and checks the output
  round-trips and stays balanced.
- `tests/fixtures.rs` renders scope labels for small snippets into reviewable `.snap`
  files under `tests/fixtures/` (regenerate + eyeball when the tokenizer changes).

## Status

Implemented:

- Grammar loading from **JSON** and **`.tmLanguage` property lists**
  ([`SyntaxDefinition::from_plist_file`]).
- `repository` / `$self` / `$base` / cross-grammar includes.
- `begin`/`end` **and** `begin`/`while` blocks.
- `\A` and `\G` anchor tracking (so `\G`-anchored embedding such as HTML
  `<style>`/`<script>` → CSS/JS works).
- Capture scopes with `$n` interpolation and nested capture `patterns`.
- Numeric back-references in `end`/`while` patterns.
- **Injection grammars** (`injectionSelector` and per-grammar `injections`), selected via
  a TextMate scope-selector engine.
- Bundled grammars (all 244 upstream grammars load).

Known rough edges: Markdown's paragraph/inline interplay can over-nest slightly inside
embedded code blocks (output stays correct-length and balanced), and `\A` is treated as
line-relative on the first line only. These don't affect round-tripping.

## Attribution

Bundled grammars come from
[`shikijs/textmate-grammars-themes`](https://github.com/shikijs/textmate-grammars-themes)
and retain their individual upstream licenses (see `assets/grammars/UPSTREAM-LICENSE` and
`UPSTREAM-NOTICE`).
