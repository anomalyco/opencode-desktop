// The web build's messages: each locale but the source one as one JSON object of key to message,
// its layers merged in `locales.json`'s order (a later layer winning, as `rune_i18n::codegen`
// merges them), written as `<out>/<id>.json` for the page to fetch when it shows that locale
// (`crates/opencode-i18n/src/web.rs`). The module carries the source locale alone.
// Usage: node (or bun) scripts/web-i18n.mjs <out dir>
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { fileURLToPath } from "node:url"

const out = process.argv[2]
if (!out) throw new Error("usage: node scripts/web-i18n.mjs <out dir>")
const dir = join(fileURLToPath(new URL("..", import.meta.url)), "crates", "opencode-i18n", "locales")
const manifest = JSON.parse(readFileSync(join(dir, "locales.json"), "utf8"))
mkdirSync(out, { recursive: true })
let bytes = 0
for (const locale of manifest.locales.slice(1)) {
  const merged = {}
  for (const layer of manifest.layers) {
    const file = join(dir, layer, `${locale.id}.json`)
    if (!existsSync(file)) continue
    for (const [key, value] of Object.entries(JSON.parse(readFileSync(file, "utf8")))) if (typeof value === "string") merged[key] = value
  }
  const text = JSON.stringify(merged)
  bytes += text.length
  writeFileSync(join(out, `${locale.id}.json`), text)
}
console.log(`web i18n: ${manifest.locales.length - 1} locales, ${bytes} bytes`)