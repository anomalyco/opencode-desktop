// Copies OpenCode Desktop's catalogs into locales/official, one JSON file per locale, and its
// locale list into locales/locales.json. Run it from this folder with a checkout of the opencode
// repository (with its dependencies installed):
//
//   bun sync.ts ../../../opencode
//
// Each file merges a locale's app messages (packages/app/src/runtime/i18n/<id>.ts) with its UI
// messages (packages/ui/src/i18n/<id>.ts), the UI's winning, as Desktop's language provider does.
// The locales, their order, labels and tags come from desktop-native.ts and the right-to-left ones
// from language.tsx, so a language Desktop adds arrives with the next sync and build.
import { readdir, rm } from "node:fs/promises"
import path from "node:path"

const root = path.resolve(process.argv[2] ?? "")
const app = path.join(root, "packages/app/src/runtime/i18n")
const ui = path.join(root, "packages/ui/src/i18n")
if (!(await Bun.file(path.join(app, "desktop-native.ts")).exists())) {
  console.error("usage: bun sync.ts <opencode checkout>")
  process.exit(1)
}

const native = await import(path.join(app, "desktop-native.ts"))
const ids: string[] = [...native.DESKTOP_NATIVE_LOCALES]
const rtl = new Set(
  [...((await Bun.file(path.join(app, "language.tsx")).text()).match(/RTL_LOCALES[^=]*=\s*new Set\(\[([^\]]*)\]/)?.[1] ?? "")
    .matchAll(/"([^"]+)"/g)].map((match) => match[1]),
)

const out = path.join(import.meta.dir, "locales")
const official = path.join(out, "official")
const sorted = (dict: Record<string, string>) =>
  Object.fromEntries(Object.entries(dict).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))

// Desktop's GUI plugins this app builds in rather than loading as plugins; each keeps its own
// catalog there (packages/gui-plugins/src/<id>/i18n), which joins here under `plugin.<id>.`.
const builtIn = ["browser", "file", "review", "terminal", "updater"]
const plugins = path.join(root, "packages/gui-plugins/src")
const plugin = async (name: string, id: string) => {
  const file = path.join(plugins, name, "i18n", `${id}.ts`)
  if (!(await Bun.file(file).exists())) return {}
  const dict: Record<string, string> = (await import(file)).default
  return Object.fromEntries(Object.entries(dict).map(([key, value]) => [`plugin.${name}.${key}`, value]))
}

for (const id of ids) {
  const messages = {
    ...(await import(path.join(app, `${id}.ts`))).dict,
    ...(await import(path.join(ui, `${id}.ts`))).dict,
    ...Object.assign({}, ...(await Promise.all(builtIn.map((name) => plugin(name, id))))),
  }
  await Bun.write(path.join(official, `${id}.json`), JSON.stringify(sorted(messages), null, 2) + "\n")
}
// Number patterns ICU gives each locale's tag, as Desktop's Intl.NumberFormat calls show them,
// with the digits left to the app's own formatter: `{{value}}%`, `{{value}} %`, `%{{value}}`.
const intl = path.join(out, "intl")
for (const id of ids) {
  const parts = new Intl.NumberFormat(native.DESKTOP_NATIVE_LOCALE_TAGS[id], { style: "percent" }).formatToParts(12.5)
  const digits = new Set(["integer", "group", "decimal", "fraction"])
  const percent = parts.reduce(
    (text, part, index) =>
      digits.has(part.type) ? (index > 0 && digits.has(parts[index - 1].type) ? text : text + "{{value}}") : text + part.value,
    "",
  )
  await Bun.write(path.join(intl, `${id}.json`), JSON.stringify({ "intl.percent": percent }, null, 2) + "\n")
}

for (const file of await readdir(official)) {
  if (!ids.includes(path.basename(file, ".json"))) await rm(path.join(official, file))
}

const locales = ids.map((id) => ({
  id,
  tag: native.DESKTOP_NATIVE_LOCALE_TAGS[id],
  label: native.DESKTOP_NATIVE_LABELS[id],
  ...(rtl.has(id) ? { rtl: true } : {}),
}))
await Bun.write(
  path.join(out, "locales.json"),
  JSON.stringify({ layers: ["official", "intl", "rune"], locales }, null, 2) + "\n",
)
console.log(`${ids.length} locales, ${rtl.size} right to left`)
