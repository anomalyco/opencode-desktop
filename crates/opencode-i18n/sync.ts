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
import { mkdtemp, readdir, rm } from "node:fs/promises"
import { createRequire } from "node:module"
import { tmpdir } from "node:os"
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
const builtIn = ["browser", "details", "file", "review", "terminal", "updater"]
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
// Number and date patterns as Desktop's Intl calls show them. They come from Desktop's own
// Electron, in a hidden window, since Chromium's ICU data is cut down: a locale it lacks
// (Icelandic, Armenian) formats as en-US there, and some (Uzbek, Azerbaijani) with root patterns.
// The tags Windows has locales for, whose names intl.names.* carry (see languageNames).
const spellcheckTags = (await Bun.file(path.join(import.meta.dir, "spellcheck-tags.txt")).text()).split("\n").filter(Boolean)
const intl = path.join(out, "intl")
const generated = await inElectron(ids.map((id) => native.DESKTOP_NATIVE_LOCALE_TAGS[id]))
for (const [index, id] of ids.entries()) {
  await Bun.write(path.join(intl, `${id}.json`), JSON.stringify(generated[index], null, 2) + "\n")
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
  JSON.stringify({ layers: ["fill", "official", "intl", "rune"], locales }, null, 2) + "\n",
)
console.log(`${ids.length} locales, ${rtl.size} right to left`)

// `intl` for each tag, run in a hidden window of the Electron Desktop is built with.
async function inElectron(tags: string[]): Promise<Record<string, string>[]> {
  const electron: string = createRequire(path.join(root, "packages/desktop/package.json"))("electron")
  const dir = await mkdtemp(path.join(tmpdir(), "opencode-i18n-"))
  const result = path.join(dir, "intl.json")
  const main = `
    const { app, BrowserWindow } = require("electron")
    app.setPath("userData", ${JSON.stringify(path.join(dir, "user"))})
    // A failure in the page rejects; say what it was and end rather than wait forever.
    process.on("unhandledRejection", (error) => { console.error(String(error)); app.exit(1) })
    app.whenReady().then(async () => {
      const window = new BrowserWindow({ show: false })
      await window.loadURL("about:blank")
      const out = await window.webContents.executeJavaScript(${JSON.stringify(`(${JSON.stringify(tags)}).map((tag) => (${intlMessages})(tag, ${JSON.stringify(spellcheckTags)}))`)})
      require("fs").writeFileSync(${JSON.stringify(result)}, JSON.stringify(out))
      app.exit(0)
    })`
  await Bun.write(path.join(dir, "main.js"), main)
  const env = { ...process.env, ELECTRON_RUN_AS_NODE: undefined }
  const code = await Bun.spawn([electron, path.join(dir, "main.js")], { env, stdout: "inherit", stderr: "inherit" }).exited
  if (code !== 0) throw new Error(`electron exited with ${code}`)
  const messages = await Bun.file(result).json()
  await rm(dir, { recursive: true, force: true })
  return messages
}

// A locale's number and date patterns as Desktop's Intl calls write them, for src/lib.rs and
// src/date.rs to fill in at runtime. `intl.percent` is the percent pattern with the digits left to
// the app's own formatter (`{{value}}%`, `{{value}} %`, `%{{value}}`). Each date
// format is a pattern of literals and {{month}}, {{d}}/{{dd}}, {{y}}, {{h}}/{{hh}} (1–12),
// {{K}}/{{KK}} (0–11), {{H}}/{{HH}} (0–23), {{k}}/{{kk}} (1–24), {{m}}/{{mm}}, {{dayPeriod}} and
// {{zone}}, with the month names (`|`-separated) and the day period of each hour that pattern
// uses. Numbers are written with ASCII digits and mapped to the locale's `intl.date.digits`.
function intlMessages(tag: string, spellcheckTags: string[]) {
  const formats: Record<string, Intl.DateTimeFormatOptions> = {
    time: { timeStyle: "short" },
    monthDay: { month: "short", day: "numeric" },
    monthDayYear: { day: "numeric", month: "short", year: "numeric" },
    dateTime: { dateStyle: "medium", timeStyle: "short" },
    monthDayTime: { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" },
    monthDayYearTime: { day: "numeric", month: "short", year: "numeric", hour: "numeric", minute: "2-digit" },
    monthDayTimeZone: { month: "short", day: "numeric", hour: "numeric", minute: "2-digit", timeZoneName: "shortOffset" },
  }
  const resolved = new Intl.DateTimeFormat(tag).resolvedOptions()
  if (!["gregory", "buddhist", "persian"].includes(resolved.calendar)) throw new Error(`${tag}: calendar ${resolved.calendar}`)
  const latn = `${tag}-u-nu-latn`
  const digits = Array.from({ length: 10 }, (_, d) =>
    new Intl.NumberFormat(tag, { numberingSystem: resolved.numberingSystem, useGrouping: false }).format(d),
  )
  if (digits.some((d) => [...d].length !== 1)) throw new Error(`${tag}: digits ${digits}`)
  const utc = (y: number, m: number, d: number, h = 13, min = 5) => Date.UTC(y, m, d, h, min)
  const part = (format: Intl.DateTimeFormat, at: number, type: string) =>
    format.formatToParts(at).find((p) => p.type === type)?.value ?? ""

  // A day in each month of the locale's calendar, and one early in a month (to tell `d` from `dd`).
  const calendarDate = new Intl.DateTimeFormat(latn, { month: "numeric", day: "numeric", timeZone: "UTC" })
  const inMonth: number[] = []
  let early = 0
  for (let day = 0; day < 400; day++) {
    const at = utc(2026, 0, 1 + day)
    const month = Number(part(calendarDate, at, "month"))
    const date = Number(part(calendarDate, at, "day"))
    if (date === 15) inMonth[month - 1] ??= at
    if (date === 3 && !early) early = utc(2026, 0, 1 + day, 1)
  }
  const names = (options: Intl.DateTimeFormatOptions, type: string, at: number[]) =>
    at.map((when) => part(new Intl.DateTimeFormat(tag, { ...options, timeZone: "UTC" }), when, type)).join("|")
  // A month or weekday on its own is the whole of what the format writes (`1月`, not `1`).
  const alone = (options: Intl.DateTimeFormatOptions, at: number[]) =>
    at.map((when) => new Intl.DateTimeFormat(tag, { ...options, timeZone: "UTC" }).format(when)).join("|")
  const sunday = utc(2026, 10, 1)
  const week = Array.from({ length: 7 }, (_, d) => sunday + d * 86_400_000)

  const percentParts = new Intl.NumberFormat(tag, { style: "percent" }).formatToParts(12.5)
  const numeric = new Set(["integer", "group", "decimal", "fraction"])
  const out: Record<string, string> = {
    "intl.percent": percentParts.reduce(
      (text, p, i) =>
        numeric.has(p.type) ? (i > 0 && numeric.has(percentParts[i - 1].type) ? text : text + "{{value}}") : text + p.value,
      "",
    ),
    "intl.date.calendar": resolved.calendar,
    "intl.date.digits": digits.join(""),
    "intl.date.months.short": alone({ month: "short" }, inMonth),
    "intl.date.months.long": alone({ month: "long" }, inMonth),
    "intl.date.weekdays.short": alone({ weekday: "short" }, week),
    "intl.date.weekdays.long": alone({ weekday: "long" }, week),
    ...numbers(),
    ...currency(),
    ...units(),
    ...lists(),
    ...languageNames(),
    // The locale Desktop's spellcheck menu names and orders its dictionaries in
    // (`nativeLanguageOptions`): `en-US` where Chromium's ICU lacks the locale.
    "intl.names.locale": new Intl.DisplayNames(tag, { type: "language" }).resolvedOptions().locale,
    "intl.plural": pluralTable(tag),
  }
  for (const [name, options] of Object.entries(formats)) {
    const local = new Intl.DateTimeFormat(tag, { ...options, timeZone: "UTC" })
    const ascii = new Intl.DateTimeFormat(latn, { ...options, timeZone: "UTC" })
    const cycle = { h11: "K", h12: "h", h23: "H", h24: "k" }[ascii.resolvedOptions().hourCycle ?? "h23"]
    // 01:05 on the 3rd: a padded day, hour or minute starts with 0.
    const padded = (type: string) => part(ascii, early, type).startsWith("0")
    const token = (type: string, value: string) => {
      if (type === "literal" || type === "era") return value
      if (type === "month") return "{{month}}"
      if (type === "day") return padded("day") ? "{{dd}}" : "{{d}}"
      if (type === "year") return "{{y}}"
      if (type === "hour") return padded("hour") ? `{{${cycle}${cycle}}}` : `{{${cycle}}}`
      if (type === "minute") return padded("minute") ? "{{mm}}" : "{{m}}"
      if (type === "dayPeriod") return "{{dayPeriod}}"
      if (type === "timeZoneName") return "{{zone}}"
      throw new Error(`${tag} ${name}: part ${type}`)
    }
    const parts = local.formatToParts(early)
    out[`intl.date.${name}`] = parts.map((p) => token(p.type, p.value)).join("")
    if (parts.some((p) => p.type === "month")) out[`intl.date.${name}.months`] = names(options, "month", inMonth)
    if ("hour" in options || "timeStyle" in options) {
      const hours = Array.from({ length: 24 }, (_, h) => part(local, utc(2026, 10, 3, h), "dayPeriod"))
      out[`intl.date.${name}.dayPeriods`] = hours.join("|")
    }
  }

  // The `shortOffset` zone: `GMT`, else `GMT{{offset}}` with its signs and separator.
  const zone = (timeZone: string) =>
    part(new Intl.DateTimeFormat(latn, { timeZoneName: "shortOffset", timeZone }), utc(2026, 0, 15), "timeZoneName")
  const [minus, plus] = [zone("Etc/GMT+7"), zone("Asia/Kolkata")]
  let prefix = 0
  while (prefix < minus.length && minus[prefix] === plus[prefix] && !/[\d+\-−]/.test(minus[prefix])) prefix++
  let suffix = 0
  while (suffix < minus.length - prefix && minus.at(-1 - suffix) === plus.at(-1 - suffix) && !/\d/.test(minus.at(-1 - suffix)!))
    suffix++
  const [m, p] = [minus.slice(prefix, minus.length - suffix), plus.slice(prefix, plus.length - suffix)]
  if (!m.endsWith("7") || !/5\D*30$/.test(p)) throw new Error(`${tag}: zones ${minus} ${plus}`)
  Object.assign(out, {
    "intl.date.zone": `${minus.slice(0, prefix)}{{offset}}${minus.slice(minus.length - suffix)}`,
    "intl.date.zone.utc": part(
      new Intl.DateTimeFormat(tag, { timeZoneName: "shortOffset", timeZone: "UTC" }),
      utc(2026, 0, 15),
      "timeZoneName",
    ),
    "intl.date.zone.minus": m.slice(0, -1),
    "intl.date.zone.plus": p.slice(0, p.indexOf("5")),
    "intl.date.zone.separator": p.slice(p.indexOf("5") + 1, -2),
  })

  // Relative days as Desktop's `RelativeTimeFormat({ numeric: "auto" })` names them, and each
  // unit's past and future by plural category as `{ numeric: "always" }` writes them.
  const auto = new Intl.RelativeTimeFormat(tag, { numeric: "auto" })
  out["intl.relative.day.yesterday"] = auto.format(-1, "day")
  out["intl.relative.day.today"] = auto.format(0, "day")
  out["intl.relative.day.tomorrow"] = auto.format(1, "day")
  // A locale Chromium has no relative times for writes en-US's, with en-US's plural rules.
  const always = new Intl.RelativeTimeFormat(tag, { numeric: "always" })
  const rules = new Intl.PluralRules(always.resolvedOptions().locale)
  out["intl.relative.plural"] = pluralTable(always.resolvedOptions().locale)
  const samples = rules.resolvedOptions().pluralCategories.flatMap((category) => {
    const n = [...Array.from({ length: 2000 }, (_, i) => i), 1_000_000].find((n) => rules.select(n) === category)
    return n === undefined ? [] : [[category, n] as const]
  })
  for (const unit of ["second", "minute", "hour", "day", "week", "month", "year"] as const) {
    for (const [direction, sign] of [["past", -1], ["future", 1]] as const) {
      for (const [category, n] of samples) {
        const parts = always.formatToParts(n === 0 ? sign * 0 : sign * n, unit)
        out[`intl.relative.${unit}.${direction}.${category}`] = parts.reduce(
          (text, p, i) =>
            numeric.has(p.type) ? (i > 0 && numeric.has(parts[i - 1].type) ? text : text + "{{count}}") : text + p.value,
          "",
        )
      }
    }
  }
  return out

  // `Intl.NumberFormat(tag)`'s symbols: its digits, separators, grouping (`3`, Indian `3,2`), the
  // fewest digits over a group before it groups (2 in Spanish: `1234`, `12.345`), and its minus.
  function numbers() {
    const format = new Intl.NumberFormat(tag, { maximumFractionDigits: 3 })
    const of = (n: number, type: string) => format.formatToParts(n).filter((p) => p.type === type)
    const groups = of(123_456_789, "integer").map((p) => p.value.length)
    const primary = groups.length > 1 ? groups.at(-1)! : 3
    const secondary = groups.length > 2 ? groups.at(-2)! : primary
    const minGrouping = [1, 2, 3].find((k) => of(Number("1".repeat(primary + k)), "group").length > 0) ?? 4
    const negative = format.formatToParts(-1)
    const sign = negative.findIndex((p) => p.type === "integer")
    const text = (parts: Intl.NumberFormatPart[]) => parts.map((p) => p.value).join("")
    return {
      "intl.number.digits": Array.from({ length: 10 }, (_, d) => format.format(d)).join(""),
      "intl.number.group": of(1_234_567, "group")[0]?.value ?? ",",
      "intl.number.decimal": of(1.5, "decimal")[0]?.value ?? ".",
      "intl.number.grouping": primary === secondary ? `${primary}` : `${primary},${secondary}`,
      "intl.number.minGrouping": `${minGrouping}`,
      "intl.number.negative": `${text(negative.slice(0, sign))}{{value}}${text(negative.slice(sign + 1))}`,
    }
  }

  // US dollars as the usage dialog's `format.money` writes them (`Intl.NumberFormat(tag, { style:
  // "currency", currency: "USD" })`): the pattern around the amount (`${{value}}`, `{{value}} $`,
  // `{{value}} US$`), with the amount itself left to the app's own decimal formatter, which this
  // checks writes the same digits.
  function currency() {
    const money = (fraction: number) =>
      new Intl.NumberFormat(tag, { style: "currency", currency: "USD", minimumFractionDigits: fraction, maximumFractionDigits: 2 })
    const pattern = (n: number, fraction: number) => {
      const parts = money(fraction).formatToParts(n)
      const amount = parts.filter((p) => numeric.has(p.type)).map((p) => p.value).join("")
      const plain = new Intl.NumberFormat(tag, { minimumFractionDigits: fraction, maximumFractionDigits: 2 }).format(n)
      if (amount !== plain) throw new Error(`${tag}: currency amount ${amount} is not ${plain}`)
      return parts.reduce(
        (text, p, i) => (numeric.has(p.type) ? (i > 0 && numeric.has(parts[i - 1].type) ? text : text + "{{value}}") : text + p.value),
        "",
      )
    }
    const cents = pattern(1234.5, 2)
    if (pattern(1234, 0) !== cents) throw new Error(`${tag}: currency patterns differ: ${cents} ${pattern(1234, 0)}`)
    return { "intl.currency.usd": cents }
  }

  // File sizes as the file viewer's toolbar writes them (`formatBytes`, artifact-view.tsx:
  // `Intl.NumberFormat(tag, { style: "unit", unit, unitDisplay })`, bytes long and the rest
  // short): each unit's pattern around the number for each plural category, a whole number's
  // where one reaches it and else one tenth's (Russian's `other`), with the number left to the
  // app's own decimal formatter, which this checks writes the same digits.
  function units() {
    const rules = new Intl.PluralRules(tag)
    const whole = [...Array.from({ length: 2000 }, (_, i) => i), 1_000_000]
    const tenths = Array.from({ length: 1000 }, (_, i) => i + 0.5)
    const samples = rules.resolvedOptions().pluralCategories.flatMap((category) => {
      const n = [...whole, ...tenths].find((n) => rules.select(n) === category)
      return n === undefined ? [] : [[category, n] as const]
    })
    const out: Record<string, string> = {}
    for (const [unit, unitDisplay] of [["byte", "long"], ["kilobyte", "short"], ["megabyte", "short"], ["gigabyte", "short"]] as const) {
      for (const [category, n] of samples) {
        const parts = new Intl.NumberFormat(tag, { style: "unit", unit, unitDisplay, maximumFractionDigits: 1 }).formatToParts(n)
        const amount = parts.filter((p) => numeric.has(p.type)).map((p) => p.value).join("")
        const plain = new Intl.NumberFormat(tag, { maximumFractionDigits: 1 }).format(n)
        if (amount !== plain) throw new Error(`${tag}: ${unit} amount ${amount} is not ${plain}`)
        out[`intl.unit.${unit}.${category}`] = parts.reduce(
          (text, p, i) => (numeric.has(p.type) ? (i > 0 && numeric.has(parts[i - 1].type) ? text : text + "{{value}}") : text + p.value),
          "",
        )
      }
    }
    return out
  }

  // `Intl.DisplayNames(tag, { type: "language" })`, which names the spellcheck dictionaries in
  // Desktop's context menu (`nativeLanguageOptions`), for every tag Windows has a locale for
  // (spellcheck-tags.txt, the dictionaries it can offer), taken apart as CLDR builds a name: the
  // names of the languages, scripts and regions those tags use (`code<tab>name` lines), the patterns
  // joining a name to one or two of them (`{{0}} ({{1}})`, `{{0}} ({{1}}, {{2}})`), and whole
  // names only where that doesn't rebuild Chromium's (`en-AU:Australian English`). A part
  // Chromium has no name for keeps its code (`English (029)`). src/names.rs builds them back.
  function languageNames() {
    const display = (type: "language" | "region" | "script") => new Intl.DisplayNames(tag, { type, fallback: "none" })
    const [languageOf, regionOf, scriptOf] = [display("language"), display("region"), display("script")]
    const tags = spellcheckTags.map((t) => t.split("-"))
    const isScript = (part: string) => /^[A-Z][a-z]{3}$/.test(part)
    const isRegion = (part: string) => /^([A-Z]{2}|\d{3})$/.test(part)
    const named = (codes: Iterable<string>, of: (code: string) => string | undefined) =>
      Object.fromEntries([...new Set(codes)].flatMap((code) => {
        const name = of(code)
        return name && name !== code ? [[code, name]] : []
      }))
    const languages: Record<string, string> = named(tags.map((t) => t[0]), (c) => languageOf.of(c))
    const scripts: Record<string, string> = named(tags.flatMap((t) => t.slice(1).filter(isScript)), (c) => scriptOf.of(c))
    const regions: Record<string, string> = named(tags.flatMap((t) => t.slice(1).filter(isRegion)), (c) => regionOf.of(c))
    // A name can change case inside a compound (Russian `латиница`); tags whose names this
    // doesn't rebuild exactly are kept whole below.
    const between = (whole: string | undefined, fallback: string, ...names: (string | undefined)[]) =>
      names.reduce<string>((pattern, name, i) => {
        const at = name ? pattern.toLowerCase().indexOf(name.toLowerCase()) : -1
        if (at < 0) return fallback
        return pattern.slice(0, at) + `{{${i}}}` + pattern.slice(at + name!.length)
      }, whole ?? fallback)
    const [de, latin, belgium] = [languageOf.of("de"), scriptOf.of("Latn"), regionOf.of("BE")]
    const pattern = between(languageOf.of("de-BE"), "{{0}} ({{1}})", de, belgium)
    const patternScript = between(languageOf.of("de-Latn-BE"), "{{0}} ({{1}}, {{2}})", de, latin, belgium)
    const fill = (template: string, parts: string[]) => parts.reduce((text, part, i) => text.replace(`{{${i}}}`, part), template)
    const dialects: Record<string, string> = {}
    for (const parts of tags) {
      const [language, ...rest] = parts
      const code = parts.join("-")
      const name = languageOf.of(code)
      if (!name) continue
      const base = languages[language] ?? language
      const subs = rest.map((p) => (isScript(p) ? scripts[p] : regions[p]) ?? p)
      const built = subs.length === 0 ? base : fill(subs.length === 1 ? pattern : patternScript, [base, ...subs])
      if (name !== built) dialects[code] = name
    }
    // `code<tab>name`, one a line: names have colons (`S:t Kitts och Nevis`) and bars.
    const list = (names: Record<string, string>) =>
      Object.entries(names)
        .map(([code, name]) => {
          if (/[\t\n]/.test(name)) throw new Error(`${tag}: ${code} is named ${name}`)
          return `${code}\t${name}`
        })
        .join("\n")
    return {
      "intl.names.languages": list(languages),
      "intl.names.scripts": list(scripts),
      "intl.names.regions": list(regions),
      "intl.names.dialects": list(dialects),
      "intl.names.pattern": pattern,
      "intl.names.patternScript": patternScript,
    }
  }

  // `Intl.ListFormat(tag, { style: "long", type: "conjunction" })`'s separators: between two items,
  // then after the first, in the middle and before the last of more. Before the last, Spanish
  // writes `e` for `y` ahead of an i sound and Hebrew drops its hyphen ahead of a Hebrew word.
  function lists() {
    const format = new Intl.ListFormat(tag, { style: "long", type: "conjunction" })
    const between = (items: string[]) => {
      const separators: string[] = []
      let pending: string | undefined
      for (const p of format.formatToParts(items)) {
        if (p.type !== "element") {
          if (pending !== undefined) pending += p.value
          continue
        }
        if (pending !== undefined) separators.push(pending)
        pending = ""
      }
      return separators
    }
    const [pair] = between(["A", "B"])
    const [start, middle, end] = between(["A", "B", "C", "D"])
    const last = (word: string) => [between(["A", word])[0], between(["A", "B", word])[1]]
    const [pairI, endI] = last("Iris")
    const [pairHebrew, endHebrew] = last("שלום")
    return {
      "intl.list.pair": pair,
      "intl.list.start": start,
      "intl.list.middle": middle,
      "intl.list.end": end,
      "intl.list.pair.i": pairI,
      "intl.list.end.i": endI,
      "intl.list.pair.hebrew": pairHebrew,
      "intl.list.end.hebrew": endHebrew,
    }
  }

  // `Intl.PluralRules(locale)`'s category of each whole number 0–199, then of 1000000, a letter
  // each (`z`ero, `o`ne, `t`wo, `f`ew, `m`any, othe`r`): CLDR's integer rules repeat every hundred
  // past that, but for the millions French, Spanish and others call many.
  function pluralTable(locale: string) {
    const rules = new Intl.PluralRules(locale)
    const letter: Record<string, string> = { zero: "z", one: "o", two: "t", few: "f", many: "m", other: "r" }
    return [...Array.from({ length: 200 }, (_, n) => n), 1_000_000].map((n) => letter[rules.select(n)]).join("")
  }
}
