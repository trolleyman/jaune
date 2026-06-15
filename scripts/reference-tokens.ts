/**
 * Produces reference tokenizations of the sample corpus using `vscode-textmate` — the same
 * TextMate engine VS Code (and Shiki) use — so jaune's output can be diffed against the canonical
 * one.
 *
 * For every sample (the `textmate-grammars-themes/samples/*.sample` corpus plus our own
 * `tests/samples/extra-input/*.sample`), it tokenizes each line and writes the per-line token
 * scopes to `tests/samples/.reference-tokens/<file>.json` as a compact
 * `[[ [startCol, charLen, [scope, …]], … ], … ]` array, with the grammar root scope stripped.
 *
 * The Rust `tests/samples.rs` snapshot test reads those JSON files and renders them through the
 * *same* formatter it uses for jaune, writing `tests/samples/textmate-grammars-themes/<file>`.
 * Keeping a single formatter guarantees any diff between the two sample directories is a real
 * tokenization difference, never a formatting one.
 *
 * Columns are counted in Unicode scalar values (not UTF-16 code units) to match Rust's
 * `chars().count()`.
 *
 * Run with: `bun run scripts/reference-tokens.ts` (then `UPDATE_SNAPSHOTS=1 cargo test --test samples`).
 */
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import * as oniguruma from "vscode-oniguruma"
import * as vsctm from "vscode-textmate"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const grammarsDir = join(root, "assets/grammars")
const tgtSamples = join(root, "textmate-grammars-themes/samples")
const extraInput = join(root, "tests/samples/extra-input")
const outDir = join(root, "tests/samples/.reference-tokens")

// --- Oniguruma engine ----------------------------------------------------------------------------
const wasmPath = join(root, "node_modules/vscode-oniguruma/release/onig.wasm")
await oniguruma.loadWASM(readFileSync(wasmPath).buffer as ArrayBuffer)
const onigLib = Promise.resolve({
  createOnigScanner: (patterns: string[]) => new oniguruma.OnigScanner(patterns),
  createOnigString: (s: string) => new oniguruma.OnigString(s),
})

// --- Grammar registry ----------------------------------------------------------------------------
// Map every grammar's scopeName to its file, and collect the injection grammars so we can offer them
// to vscode-textmate, scoped the way VS Code itself scopes them (see `getInjections` below).
const scopeToPath = new Map<string, string>()
const nameToScope = new Map<string, string>()
type Injection = { scope: string; injectTo: string[]; positives: string[] }
const injections: Injection[] = []

/**
 * Pull the positive (non-negated) scope atoms out of a TextMate `injectionSelector` — e.g.
 * `"L:source.ts#meta.decorator.ts -comment, L:text.html"` → `["source.ts", "meta.decorator.ts",
 * "text.html"]`. We only need the positives: they say which scope stacks the injection *targets*,
 * whereas the negated `-foo` atoms only ever shrink that set.
 */
function selectorPositives(selector: string): string[] {
  const atoms: string[] = []
  // Drop the `L:`/`R:` "applies to left/right" prefixes and grouping parens, then split the
  // comma-separated alternatives into whitespace-separated terms.
  for (const term of selector.replace(/[LR]:/g, " ").replace(/[(),]/g, " ").split(/\s+/)) {
    if (!term || term.startsWith("-")) continue
    // `a#b` means both scopes must be present; treat each part as a targeted atom.
    for (const part of term.split("#")) {
      if (part && /^[\w.+-]+$/.test(part)) atoms.push(part)
    }
  }
  return atoms
}

for (const file of readdirSync(grammarsDir)) {
  if (!file.endsWith(".json")) continue
  const path = join(grammarsDir, file)
  let json: any
  try {
    json = JSON.parse(readFileSync(path, "utf8"))
  } catch {
    continue
  }
  if (!json.scopeName) continue
  scopeToPath.set(json.scopeName, path)
  nameToScope.set(file.replace(/\.json$/, ""), json.scopeName)
  if (json.injectionSelector)
    injections.push({
      scope: json.scopeName,
      injectTo: Array.isArray(json.injectTo) ? json.injectTo : [],
      positives: selectorPositives(json.injectionSelector),
    })
}

/** Whether scope names `a` and `b` are the same scope or one is an ancestor of the other. */
const dotCompatible = (a: string, b: string) =>
  a === b || a.startsWith(`${b}.`) || b.startsWith(`${a}.`)

// Injection grammars the recursive vscode-textmate tokenizer cannot evaluate without blowing the JS
// call stack, no matter how tightly they're scoped. `mdc` is a near-clone of the Markdown grammar
// that re-includes `text.html.markdown`; injected back into Markdown (which its `L:text.html.markdown`
// selector and injectTo both target) it recurses Markdown→mdc→Markdown→… and overflows on even a
// single heading line. jaune's *iterative* tokenizer is immune and emits no mdc scopes for these
// samples either, so dropping mdc here keeps the reference a faithful oracle rather than skipping the
// whole sample.
const recursionUnsafeInjections = new Set(["text.markdown.mdc.standalone"])

/**
 * Decide whether an injection should be offered to the grammar rooted at `scopeName`.
 *
 * VS Code registers an injection into a grammar via the extension's `injectTo` contribution — a list
 * of *grammar scopes* the injection attaches to — and only then uses `injectionSelector` for the
 * fine-grained scope-stack match while tokenizing. So we mirror that: scope by `injectTo`.
 *
 * The earlier "offer everything, let the selector sort it out" approach was wrong on two counts: it
 * recursed without converging on `markdown.embedded` (stack overflow), and — once that was fixed —
 * it still polluted every HTML-family document, because broad selectors like angular's `L:text.html`
 * match `text.html.markdown`/`text.html.basic` and tokenized ordinary prose as Angular/TS
 * expressions. `injectTo` is precisely the constraint VS Code uses to stop that.
 *
 * A few vendored grammars carry no `injectTo` (`mermaid`, `mdc`, `angular-expression`). For those we
 * fall back to the `injectionSelector`, but *conservatively*: only when a selector scope is the
 * target root itself or something more specific (so `mermaid`'s `text.html.markdown` still attaches
 * to Markdown, while angular-expression's bare `text.html` no longer blankets every HTML derivative).
 */
function targetsScope(inj: Injection, scopeName: string): boolean {
  if (recursionUnsafeInjections.has(inj.scope)) return false
  if (inj.injectTo.length > 0) return inj.injectTo.some((t) => dotCompatible(t, scopeName))
  return inj.positives.some((atom) => atom === scopeName || atom.startsWith(`${scopeName}.`))
}

const registry = new vsctm.Registry({
  onigLib,
  loadGrammar: async (scopeName: string) => {
    const path = scopeToPath.get(scopeName)
    if (!path) return null
    return vsctm.parseRawGrammar(readFileSync(path, "utf8"), path)
  },
  getInjections: (scopeName: string) =>
    injections.filter((inj) => targetsScope(inj, scopeName)).map((inj) => inj.scope),
})

// --- Tokenize ------------------------------------------------------------------------------------
type Token = [number, number, string[]]

/** Unicode scalar (codepoint) count of `s` — matches Rust's `chars().count()`. */
const cp = (s: string) => [...s].length

async function tokenizeSample(scopeName: string, source: string): Promise<Token[][] | null> {
  const grammar = await registry.loadGrammar(scopeName)
  if (!grammar) return null

  const lines = source.split("\n")
  if (source.endsWith("\n")) lines.pop()

  const out: Token[][] = []
  let ruleStack = vsctm.INITIAL
  for (const line of lines) {
    // Tokenize each line WITHOUT a trailing newline — the convention vscode-textmate/Shiki use, where
    // end-of-string already satisfies `$`. Appending `\n` instead feeds the newline to greedy
    // patterns: e.g. Markdown's fenced-code `while` clause `(^|\G)(?!\s*([`~]{3,})\s*$)` then fails to
    // pop at the closing fence, so a ```js block swallows the rest of the document as one JS template.
    const result = grammar.tokenizeLine(line, ruleStack)
    ruleStack = result.ruleStack
    const tokens: Token[] = []
    for (const t of result.tokens) {
      const start = Math.min(t.startIndex, line.length)
      const end = Math.min(t.endIndex, line.length)
      if (start >= end) continue // empty token
      const startCol = cp(line.slice(0, start))
      const len = cp(line.slice(start, end))
      // scopes[0] is the grammar root, assumed by the renderer — drop it.
      tokens.push([startCol, len, t.scopes.slice(1)])
    }
    out.push(tokens)
  }
  return out
}

function collectInputs(): { name: string; file: string; source: string }[] {
  const inputs: { name: string; file: string; source: string }[] = []
  for (const dir of [tgtSamples, extraInput]) {
    if (!existsSync(dir)) continue
    for (const file of readdirSync(dir).sort()) {
      if (!file.endsWith(".sample")) continue
      const name = file.split(".")[0]
      inputs.push({ name, file, source: readFileSync(join(dir, file), "utf8") })
    }
  }
  return inputs
}

mkdirSync(outDir, { recursive: true })
let written = 0
const skipped: string[] = []

for (const { name, file, source } of collectInputs()) {
  const scopeName = nameToScope.get(name)
  if (!scopeName) {
    skipped.push(`${file}: no grammar named '${name}'`)
    continue
  }
  try {
    const lines = await tokenizeSample(scopeName, source)
    if (!lines) {
      skipped.push(`${file}: grammar '${scopeName}' failed to load`)
      continue
    }
    writeFileSync(join(outDir, `${file}.json`), JSON.stringify(lines))
    written++
  } catch (e) {
    skipped.push(`${file}: ${(e as Error).message}`)
  }
}

console.log(`reference-tokens: wrote ${written} files to tests/samples/.reference-tokens/`)
if (skipped.length) console.log(`skipped ${skipped.length}:\n  ${skipped.join("\n  ")}`)
