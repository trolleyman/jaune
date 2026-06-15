/**
 * Packages the bundled grammars and publishes the `jaune` crate to crates.io.
 *
 * Steps:
 *   1. Ensure the grammar submodule is checked out.
 *   2. Regenerate the grammar assets + features (package-grammars.ts).
 *   3. Run the test suite.
 *   4. `cargo publish`.
 *
 * Usage:
 *   bun run scripts/publish.ts --dry-run   # verify packaging without publishing
 *   bun run scripts/publish.ts             # publish for real (requires a clean tree)
 */
import { $ } from "bun"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const scriptsDir = dirname(fileURLToPath(import.meta.url))
const root = join(scriptsDir, "..")
const dryRun = process.argv.includes("--dry-run")

process.chdir(root)

console.log("→ Ensuring grammar submodule is present")
await $`git submodule update --init --depth 1 textmate-grammars-themes`

console.log("→ Packaging grammars into assets/ + Cargo features")
await $`bun run ${join(scriptsDir, "package-grammars.ts")}`

console.log("→ Running tests (default features)")
await $`cargo test`

console.log("→ Verifying the full grammar set compiles")
await $`cargo build --features all`

if (dryRun) {
  console.log("→ cargo publish --dry-run (allowing dirty tree)")
  await $`cargo publish --dry-run --allow-dirty`
  console.log("\n✓ Dry run complete. Commit the regenerated assets, then run without --dry-run.")
} else {
  console.log("→ cargo publish")
  // No --allow-dirty: the regenerated assets must be committed first, so the published
  // crate matches the repository state.
  await $`cargo publish`
  console.log("\n✓ Published to crates.io.")
}
