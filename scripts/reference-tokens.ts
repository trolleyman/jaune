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
const injectionScopes: string[] = []

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
  if (json.injectionSelector) injectionScopes.push(json.scopeName)
}

const registry = new vsctm.Registry({
  onigLib,
  loadGrammar: async (scopeName: string) => {
    const path = scopeToPath.get(scopeName)
    if (!path) return null
    return vsctm.parseRawGrammar(readFileSync(path, "utf8"), path)
  },
  // Offer all injection grammars for every scope; vscode-textmate keeps only those whose
  // injectionSelector actually matches the live scope stack, so over-offering is harmless.
  getInjections: () => injectionScopes,
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
