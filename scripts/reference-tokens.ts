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
// Map every grammar's scopeName to its file, and remember which grammars are injection-only so we
// can offer them to vscode-textmate as candidate injections (it then matches their selectors).
const scopeToPath = new Map<string, string>()
const nameToScope = new Map<string, string>()
type Injection = { scope: string; positives: string[] }
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
    injections.push({ scope: json.scopeName, positives: selectorPositives(json.injectionSelector) })
}

/** Whether scope names `a` and `b` are the same scope or one is an ancestor of the other. */
const dotCompatible = (a: string, b: string) =>
  a === b || a.startsWith(`${b}.`) || b.startsWith(`${a}.`)

// A selector atom is "language-scoped" when it names a loaded grammar's root scope (or a
// parent/child of one), e.g. `source.js`, `text.html` or the bare `source`/`text` roots. Those are
// the atoms that pin an injection to a specific language family; generic atoms like `meta.tag` or
// `comment` can show up under many grammars.
const rootScopes = [...scopeToPath.keys()]
const isLanguageScoped = (atom: string) => rootScopes.some((r) => dotCompatible(atom, r))

// Injection grammars the recursive vscode-textmate tokenizer cannot evaluate without blowing the JS
// call stack, regardless of how tightly getInjections is scoped. `mdc` is a near-clone of the
// Markdown grammar that re-includes `text.html.markdown`; injected back into Markdown (which its
// `L:text.html.markdown` selector legitimately targets) it recurses Markdown→mdc→Markdown→… and
// overflows on even a single heading line. jaune's *iterative* tokenizer is immune and emits no mdc
// scopes for these samples either, so dropping mdc here keeps the reference a faithful oracle rather
// than skipping the whole sample. (Selector narrowing alone can't help: mdc's selector is correct.)
const recursionUnsafeInjections = new Set(["text.markdown.mdc.standalone"])

/**
 * Decide whether an injection should be offered to the grammar rooted at `scopeName`.
 *
 * Previously every injection was offered to every grammar ("vscode-textmate filters by selector at
 * match time, so over-offering is harmless"). It isn't harmless: with language-embedding injections
 * (Markdown fenced code, `es6-css` tagged templates, …) cross-offered to every grammar, collecting
 * injections recurses grammar→injection→grammar→… without converging, blowing the JS call stack on
 * the deeply nested `markdown.embedded` sample.
 *
 * So we narrow: an injection whose selector pins it to a language family (a language-scoped atom)
 * is only offered to grammars in that family. Injections selected purely by generic scopes (e.g.
 * `meta.tag`) keep the old broad behaviour — they don't form language cycles. vscode-textmate still
 * does the precise selector-vs-scope-stack match at tokenization time on top of this.
 */
function targetsScope(inj: Injection, scopeName: string): boolean {
  if (recursionUnsafeInjections.has(inj.scope)) return false
  const languageAtoms = inj.positives.filter(isLanguageScoped)
  if (languageAtoms.length === 0) return true
  return languageAtoms.some((atom) => dotCompatible(atom, scopeName))
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
    // Tokenize with the trailing newline so end-of-line anchors behave, then clip to the line.
    const result = grammar.tokenizeLine(line + "\n", ruleStack)
    ruleStack = result.ruleStack
    const tokens: Token[] = []
    for (const t of result.tokens) {
      const start = Math.min(t.startIndex, line.length)
      const end = Math.min(t.endIndex, line.length)
      if (start >= end) continue // pure-newline (or empty) token
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
