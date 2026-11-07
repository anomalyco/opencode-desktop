#!/usr/bin/env bun
// Regenerates assets/themes.json from Desktop's theme files, resolved exactly as Desktop resolves
// them (`resolveThemeVariant` for v1 tokens, `resolveThemeVariantV2` for v2), keeping only the
// tokens this app reads.
//
// usage: bun tools/themes.ts <opencode>/packages/ui/src/theme > assets/themes.json
//
// Values are `#rrggbbaa`, or the name of a static alpha ramp entry (`alpha-dark-10`) that
// `assets/theme.json`'s palette carries. Desktop's default (`oc-2`) carries only its v1 tokens
// (code colours): the app keeps `assets/theme.json` for its v2 ones.
import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"

const dir = process.argv[2]
if (!dir) throw new Error("usage: bun tools/themes.ts <opencode>/packages/ui/src/theme")
const { resolveThemeVariant } = await import(join(dir, "resolve.ts"))
const { resolveThemeVariantV2 } = await import(join(dir, "v2/resolve.ts"))

const V2 = [
  "background-bg-base",
  "background-bg-deep",
  "background-bg-layer-01",
  "background-bg-layer-02",
  "background-bg-layer-03",
  "background-bg-layer-04",
  "background-bg-contrast",
  "background-bg-accent",
  "background-bg-button-neutral",
  "background-bg-icon-button-contrast",
  "background-bg-inverse",
  "background-bg-code-path",
  "text-text-base",
  "text-text-muted",
  "text-text-faint",
  "text-text-accent",
  "text-text-accent-hover",
  "text-text-inverse",
  "text-text-contrast",
  "text-text-code-accent",
  "text-text-code-path",
  "icon-icon-base",
  "icon-icon-muted",
  "icon-icon-faint",
  "icon-icon-contrast",
  "icon-icon-inverse",
  "icon-icon-accent",
  "icon-icon-accent-hover",
  "border-border-base",
  "border-border-muted",
  "border-border-strong",
  "border-border-inverse",
  "border-border-focus",
  "overlay-simple-overlay-hover",
  "overlay-simple-overlay-pressed",
  "overlay-simple-overlay-contrast-hover",
  "overlay-simple-overlay-contrast-pressed",
  "overlay-simple-overlay-scrim",
  "state-bg-info",
  "state-fg-info",
  "state-border-info",
  "state-bg-success",
  "state-fg-success",
  "state-border-success",
  "state-bg-danger",
  "state-fg-danger",
  "state-border-danger",
  "state-bg-warning",
  "state-fg-warning",
  "state-border-warning",
  "agent-plan-solid",
  "agent-build-solid",
  "agent-explore-solid",
  "agent-review-solid",
  "agent-writer-solid",
  "illustration-illustration-layer-01",
  "illustration-illustration-layer-02",
  "illustration-illustration-layer-03",
  "blue-500",
  "grey-300",
  "grey-800",
]
const V1 = [
  "text-stronger",
  "text-strong",
  "text-base",
  "text-weak",
  "background-stronger",
  "surface-raised-stronger-non-alpha",
  "surface-raised-base-hover",
  "icon-base",
  "syntax-comment",
  "syntax-string",
  "syntax-keyword",
  "syntax-primitive",
  "syntax-property",
  "syntax-type",
  "syntax-constant",
  "syntax-punctuation",
  "syntax-operator",
  "syntax-variable",
  "syntax-diff-add",
  "syntax-diff-delete",
]

function hex(v: string): string {
  const s = v.trim().toLowerCase()
  if (/^#[0-9a-f]{8}$/.test(s)) return s
  if (/^#[0-9a-f]{6}$/.test(s)) return `${s}ff`
  if (/^#[0-9a-f]{3}$/.test(s)) return `#${[...s.slice(1)].map((c) => c + c).join("")}ff`
  const m = s.match(/^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+%?))?\s*\)$/)
  if (m) {
    const a = m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4])
    const b = (n: number) => Math.round(Math.max(0, Math.min(255, n))).toString(16).padStart(2, "0")
    return `#${b(+m[1])}${b(+m[2])}${b(+m[3])}${b(a * 255)}`
  }
  throw new Error(`not a colour: ${v}`)
}

function resolve(tokens: Record<string, string>, prefix: string, key: string, seen = new Set<string>()): string {
  const v = tokens[prefix + key]
  if (v === undefined) {
    if (key.startsWith("alpha-")) return key
    throw new Error(`missing token ${prefix}${key}`)
  }
  const ref = v.match(/^var\(--([a-z0-9-]+)\)$/)
  if (!ref) return hex(v)
  const name = ref[1].startsWith(prefix) ? ref[1].slice(prefix.length) : ref[1]
  if (seen.has(name)) throw new Error(`cycle at ${name}`)
  seen.add(name)
  return resolve(tokens, prefix, name, seen)
}

const names = Object.fromEntries(
  [...readFileSync(join(dir, "context.tsx"), "utf8").matchAll(/^\s+"?([a-z0-9-]+)"?: "([^"]+)",$/gm)].map((m) => [m[1], m[2]]),
)

const themes = readdirSync(join(dir, "themes"))
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.slice(0, -5))
  .sort((a, b) => (a === "oc-2" ? -1 : b === "oc-2" ? 1 : a.localeCompare(b)))
  .map((id) => {
    const theme = JSON.parse(readFileSync(join(dir, "themes", `${id}.json`), "utf8"))
    const mode = (dark: boolean) => {
      const variant = dark ? theme.dark : theme.light
      const v1 = resolveThemeVariant(variant, dark)
      const v2 = resolveThemeVariantV2(variant, dark)
      const out: Record<string, string> = {}
      if (id !== "oc-2") for (const k of V2) out[k] = resolve(v2, "v2-", k)
      // v1 tokens may name v2 ones (`syntax-keyword: var(--v2-pink-800)`).
      for (const k of V1) out[k] = resolve({ ...v1, ...Object.fromEntries(Object.entries(v2).map(([n, v]) => [n.startsWith("v2-") ? n : `v2-${n}`, v])) }, "", k)
      return out
    }
    return { id, name: names[id] ?? theme.name, light: mode(false), dark: mode(true) }
  })

console.log(JSON.stringify(themes, null, 1))
