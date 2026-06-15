# jaune — plan & notes

`jaune` is a TextMate-grammar tokenizer in Rust. It loads the VS Code / Shiki JSON grammars
(vendored from the `textmate-grammars-themes` submodule) and turns source text into scope-annotated
tokens. Regex matching is done with [`fancy-regex`](https://crates.io/crates/fancy-regex) — *not*
Oniguruma — with a `translate_oniguruma()` shim in `src/tokenizer.rs` that rewrites the
Oniguruma-isms TextMate patterns rely on (including a hand-rolled `\G` workaround).

## Correctness harness

`tests/samples/` renders every sample into an annotated source file (source + nested scope regions
as comments — see `tests/samples/README.md`) two ways:

- `tests/samples/jaune/` — produced by jaune.
- `tests/samples/textmate-grammars-themes/` — produced from a **reference** tokenization by
  `vscode-textmate` (the canonical VS Code / Shiki engine, which uses Oniguruma), rendered through
  the *same* Rust formatter via `scripts/reference-tokens.ts`.

Because both sides share one formatter, a `diff` between the two directories is a genuine
tokenization difference. This is the oracle we use to measure how close jaune's `fancy-regex`-based
matching gets to canonical Oniguruma behaviour. (Today: 83/237 samples match exactly; the rest are
real jaune gaps to chase down.)

Regenerate:

```sh
bun run reference-tokens                        # refresh the reference token JSON (needs bun install)
UPDATE_SNAPSHOTS=1 cargo test --test samples    # rewrite both sample directories
```

# Issues

1. [x] Fix the missing `tests/samples/textmate-grammars-themes/markdown.embedded.sample` reference
   render. `scripts/reference-tokens.ts` currently offers *every* injection grammar to
   `vscode-textmate` for *every* scope (`getInjections: () => injectionScopes`). On the deeply
   nested Markdown-with-fenced-code sample this over-injection sends the JS engine into
   "Maximum call stack size exceeded", so that one sample is skipped on the reference side (jaune
   renders it fine). Narrow `getInjections(scopeName)` to only return injection grammars whose
   `injectionSelector` actually targets `scopeName`, then regenerate and confirm the sample appears.

   Done. Three things were wrong; fixing the crash exposed the next two:

   - **Injection scoping.** `getInjections(scopeName)` now scopes injections by the grammar's
     `injectTo` list — the mechanism VS Code itself uses to register an injection into a grammar,
     with `injectionSelector` left as the fine scope-stack match it does on top. The old
     offer-everything approach both recursed forever *and* (once that was patched) polluted every
     HTML-family document, because broad selectors like angular's `L:text.html` match
     `text.html.markdown`/`text.html.basic` and tokenized ordinary prose as Angular/TS expressions.
     A few vendored grammars carry no `injectTo` (`mermaid`, `mdc`, `angular-expression`); for those
     we fall back to the selector but only when a selector scope is the target root or something more
     specific (so `mermaid` still attaches to Markdown while `angular-expression`'s bare `text.html`
     no longer blankets every derivative).
   - **`mdc` recursion.** The vendored `mdc` grammar is a near-clone of Markdown that re-includes
     `text.html.markdown`, so injected into Markdown it recurses Markdown→mdc→Markdown→… and overflows
     `vscode-textmate`'s recursive tokenizer on even a single heading. It's excluded from the
     reference injection set (`recursionUnsafeInjections`); jaune's iterative tokenizer is immune and
     emits no mdc scopes for the sample anyway.
   - **Line newline.** The script appended `\n` to every line before `tokenizeLine`. That feeds the
     newline to greedy patterns — Markdown's fenced-code `while` clause then never pops at the closing
     fence, so a ```js block swallowed the rest of the document as one JS template string. Tokenizing
     each line *without* the trailing `\n` (the vscode-textmate/Shiki convention, where end-of-string
     already satisfies `$`) fixes it: the `js`/`python` fenced blocks now tokenize as real JS/Python.

   The upstream `textmate-grammars-themes` submodule isn't checked out in every environment; when it's
   absent only the `tests/samples/extra-input/*.sample` corpus is regenerated (which covers
   `markdown.embedded`). The other committed reference renders should be regenerated where the
   submodule is present so they pick up the injectTo/newline changes.

2. [ ] Upgrade `fancy-regex` to pick up its `RegexSet` work, tracking
   [fancy-regex#162](https://github.com/fancy-regex/fancy-regex/issues/162) (whether fancy-regex
   becomes a full Oniguruma drop-in for TextMate use) and pointing at
   [fancy-regex#255](https://github.com/fancy-regex/fancy-regex/pull/255) directly. `RegexSet` is a
   multi-pattern DFA scanner — the Rust analog of Oniguruma's `OnigScanner` — that would replace
   jaune's one-pattern-at-a-time `captures_from_pos` loop with a single leftmost-match pass, and its
   `RegexInput` models `\G`/anchoring so we could drop the manual `\G` shim. The PR is unreleased,
   so until it lands in a published version, Cargo can pin the git branch directly (a PR *number*
   can't be referenced, but its source branch/rev can):

   ```toml
   # PR #255 lives on a branch of the upstream repo, so no fork needed:
   fancy-regex = { git = "https://github.com/fancy-regex/fancy-regex", branch = "regexset_find_input" }
   # …or pin an exact commit for reproducibility:
   # fancy-regex = { git = "https://github.com/fancy-regex/fancy-regex", rev = "34363aad51315500f4253482df3257c32ffb067d" }
   ```

   After switching the matcher over, regenerate the harness and confirm the `jaune/` ↔
   `textmate-grammars-themes/` diff shrinks rather than regresses.
