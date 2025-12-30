
import { $ } from "bun"

await $`git submodule update --init --recursive --remote --merge`
