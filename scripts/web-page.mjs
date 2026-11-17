// Writes the page (index.html) and its service worker (sw.js) into a built web folder:
// - the page gets the build's id and the English messages it shows before the module is in
//   (the `rune.web.*` keys and the shared ones it uses), with the catalog's locales, so it picks
//   the user's language as the app does and fetches that locale's `i18n/<id>.json` (the module
//   reads the same file a moment later);
// - the worker gets the build's id and the files the first frame needs.
// Usage: node (or bun) scripts/web-page.mjs <built folder>
import { createHash } from "node:crypto"
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import { join, relative, sep } from "node:path"
import { fileURLToPath } from "node:url"

const out = process.argv[2]
if (!out) throw new Error("usage: node scripts/web-page.mjs <built folder>")
const root = fileURLToPath(new URL("..", import.meta.url))
const locales = join(root, "crates", "opencode-i18n", "locales")
const SHARED = ["common.connect", "common.ok"]

const wasm = readFileSync(join(out, "app_bg.wasm"))
const build = createHash("sha256").update(wasm).digest("hex").slice(0, 12)
const english = {}
for (const layer of JSON.parse(readFileSync(join(locales, "locales.json"), "utf8")).layers) {
  const file = join(locales, layer, "en.json")
  if (!existsSync(file)) continue
  const all = JSON.parse(readFileSync(file, "utf8"))
  for (const [key, value] of Object.entries(all)) if (key.startsWith("rune.web.") || SHARED.includes(key)) english[key] = value
}
const manifest = JSON.parse(readFileSync(join(locales, "locales.json"), "utf8"))
const page = {
  build,
  shared: SHARED,
  locales: manifest.locales.map((l) => (l.rtl ? [l.id, l.tag, 1] : [l.id, l.tag])),
  en: english,
}
// `</` can't end the script element early: JSON never needs it raw.
const json = JSON.stringify(page).replaceAll("</", "<\\/")
const html = readFileSync(join(root, "apps", "web", "index.html"), "utf8")
  .replace('/* bounded-response-reader */', () => readFileSync(join(root, 'crates', 'ureq-web', 'src', 'body.js'), 'utf8'))
  .replace('<script id="page" type="application/json">{}</script>', `<script id="page" type="application/json">${json}</script>`)
writeFileSync(join(out, "index.html"), html)

const files = (dir) => readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? files(join(dir, e.name)) : [join(dir, e.name)]))
const rel = (file) => relative(out, file).split(sep).join("/")
const shell = [
  "",
  "app.js",
  "app_bg.wasm",
  "manifest.webmanifest",
  ...files(join(out, "snippets")).map(rel).filter((f) => f.endsWith(".js")),
  ...files(join(out, "icons")).map(rel),
  ...files(join(out, "fonts")).map(rel).filter((f) => /CommitMono-.*\.otf$/.test(f)),
]
const worker = readFileSync(join(root, "apps", "web", "sw.js"), "utf8").replace("'__BUILD__'", JSON.stringify(build)).replace("__SHELL__", JSON.stringify(shell))
writeFileSync(join(out, "sw.js"), worker)
console.log(`web page: build ${build}, ${Object.keys(english).length} messages, ${shell.length} files in the shell`)
