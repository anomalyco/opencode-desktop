// The web build names no std clock: `std::time::Instant` and `SystemTime::now` have no clock in a
// browser and panic there; `rune::time`'s are the page's clocks there and std's own natively.
// Clippy can't say this: natively `rune::time::Instant` *is* `std::time::Instant`, so a
// `disallowed-types` ban fails on correct code. This walks every crate of this workspace that the
// web build compiles, from its root through its `mod` declarations, leaves out modules and items
// compiled out for wasm32, and refuses std's `Instant` and `SystemTime::now` in what's left.
// `scripts/web.ps1 -Check` runs it. Usage: bun scripts/web-clock.ts [package] (default opencode-web)
import { existsSync, readFileSync } from "node:fs"
import { dirname, join, relative, basename } from "node:path"

const root = join(import.meta.dir, "..")
const entry = process.argv[2] ?? "opencode-web"
const meta = JSON.parse(
  Bun.spawnSync(["cargo", "metadata", "--format-version", "1", "--filter-platform", "wasm32-unknown-unknown", "--manifest-path", join(root, "Cargo.toml")], { stderr: "inherit" }).stdout.toString(),
)
const members = new Set<string>(meta.workspace_members)
const packages = new Map<string, any>(meta.packages.map((p: any) => [p.id, p]))
const nodes = new Map<string, any>(meta.resolve.nodes.map((n: any) => [n.id, n]))

// The workspace crates the web build compiles: its entry and their normal dependencies.
const start = meta.packages.find((p: any) => p.name === entry && members.has(p.id))
if (!start) throw new Error(`no workspace package ${entry}`)
const crates = new Set<string>()
const queue = [start.id]
while (queue.length) {
  const id = queue.pop()!
  if (crates.has(id) || !members.has(id)) continue
  crates.add(id)
  for (const dep of nodes.get(id)?.deps ?? []) if (dep.dep_kinds.some((k: any) => k.kind === null)) queue.push(dep.pkg)
}

const bad: string[] = []
let files = 0
for (const id of crates) {
  const pkg = packages.get(id)
  for (const target of pkg.targets.filter((t: any) => t.kind.some((k: string) => ["lib", "cdylib", "rlib", "staticlib"].includes(k))))
    walk(target.src_path, true, new Set())
}
if (bad.length) {
  console.error(`std's clock in the web build (Instant and SystemTime::now panic in a browser; use rune::time's, or web_time's where rune isn't a dependency):\n${bad.join("\n")}`)
  process.exit(1)
}
console.log(`web clock: ${files} files in ${crates.size} crates, no std::time::Instant or SystemTime::now`)

function walk(file: string, owner: boolean, seen: Set<string>) {
  if (seen.has(file) || !existsSync(file)) return
  seen.add(file)
  files++
  const text = stripCompiledOut(readFileSync(file, "utf8").replace(/\/\/[^\n]*/g, ""))
  const head = text.match(/^\s*((?:#!\[[^\]]*\]\s*)+)/)?.[1] ?? ""
  if ([...head.matchAll(/#!\[cfg\(([\s\S]*?)\)\]/g)].some((m) => !cfg(m[1]!))) return
  // Child modules live beside a crate root or mod.rs, else in a folder named after the file.
  modules(text, file, owner || basename(file) === "mod.rs" ? dirname(file) : join(dirname(file), basename(file, ".rs")), seen)
  // `std::time::Instant`, or `Instant` in a `use std::time::{...}`, outside items compiled out for wasm32.
  for (const m of text.matchAll(/((?:#\[[^\]]*\]\s*)*)[^\n;{}]*std::time::(?:Instant\b|\{[^}]*\bInstant\b[^}]*\})/g)) {
    if ([...(m[1] ?? "").matchAll(/#\[cfg\(([\s\S]*?)\)\]/g)].some((c) => !cfg(c[1]!))) continue
    const line = text.slice(0, m.index! + (m[1]?.length ?? 0)).split("\n").length
    bad.push(`  ${relative(root, file)}:${line}`)
  }
  // `SystemTime` itself is a plain value (a file's mtime, `UNIX_EPOCH`); only asking std for now panics.
  const std = /std::time::SystemTime::now|use\s+std::time::(?:SystemTime\b|\{[^}]*\bSystemTime\b[^}]*\})/.test(text)
  for (const m of text.matchAll(/((?:#\[[^\]]*\]\s*)*)[^\n;{}]*(std::time::)?SystemTime::now\b/g)) {
    if (!m[2] && !std) continue
    if ([...(m[1] ?? "").matchAll(/#\[cfg\(([\s\S]*?)\)\]/g)].some((c) => !cfg(c[1]!))) continue
    const line = text.slice(0, m.index! + (m[1]?.length ?? 0)).split("\n").length
    bad.push(`  ${relative(root, file)}:${line}`)
  }
}

// Walks the `mod name;` files `text` declares, children living in `dir`; an inline
// `mod name { .. }`'s own declarations live in `dir/name`.
function modules(text: string, file: string, dir: string, seen: Set<string>) {
  for (let m; (m = /(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r#)?(\w+)\s*\{/.exec(text)); ) {
    const open = m.index + m[0].length - 1
    let depth = 0, end = open
    for (; end < text.length; end++) {
      if (text[end] === "{") depth++
      else if (text[end] === "}" && --depth === 0) break
    }
    modules(text.slice(open + 1, end), file, join(dir, m[1]!), seen)
    text = text.slice(0, m.index) + text.slice(end + 1)
  }
  for (const m of text.matchAll(/((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(r#)?(\w+)\s*;/g)) {
    const attrs = m[1] ?? ""
    if ([...attrs.matchAll(/#\[cfg\(([\s\S]*?)\)\]\s*/g)].some((c) => !cfg(c[1]!))) continue
    const path = attrs.match(/#\[path\s*=\s*"([^"]+)"\]/)?.[1]
    if (path) { walk(join(dirname(file), path), basename(path) === "mod.rs", seen); continue }
    const name = m[3]!
    const flat = join(dir, `${name}.rs`)
    if (existsSync(flat)) walk(flat, false, seen)
    else walk(join(dir, name, "mod.rs"), true, seen)
  }
}

// The text without the braced items (a function, an impl, a submodule) whose
// `#[cfg(...)]` leaves them out of the web build.
function stripCompiledOut(text: string) {
  for (;;) {
    const m = [...text.matchAll(/#\[cfg\(([\s\S]*?)\)\]/g)].find((c) => !cfg(c[1]!) && /^[^;{]*\{/.test(text.slice(c.index! + c[0].length)))
    if (!m) return text
    const open = text.indexOf("{", m.index! + m[0].length)
    let depth = 0, end = open
    for (; end < text.length; end++) {
      if (text[end] === "{") depth++
      else if (text[end] === "}" && --depth === 0) break
    }
    text = text.slice(0, m.index!) + text.slice(end + 1)
  }
}

// Whether a `cfg(...)` predicate holds for the web build (`wasm32-unknown-unknown`).
// A feature counts as on; a bare name other than `debug_assertions` as off.
function cfg(src: string): boolean {
  let at = 0
  const ws = () => { while (/\s/.test(src[at] ?? "")) at++ }
  const ident = () => { ws(); const m = /^[\w:]+/.exec(src.slice(at)); at += m?.[0].length ?? 0; return m?.[0] ?? "" }
  const pred = (): boolean => {
    const name = ident()
    ws()
    if (src[at] === "(") {
      at++
      const args: boolean[] = []
      for (ws(); src[at] !== ")" && at < src.length; ws()) { args.push(pred()); ws(); if (src[at] === ",") at++ }
      at++
      if (name === "not") return !args[0]
      if (name === "all") return args.every(Boolean)
      if (name === "any") return args.some(Boolean)
      return true
    }
    if (src[at] === "=") {
      at++; ws()
      const m = /^"([^"]*)"/.exec(src.slice(at)); at += m?.[0].length ?? 0
      const value = m?.[1] ?? ""
      if (name === "target_arch") return value === "wasm32"
      if (name === "target_family") return value === "wasm"
      if (name === "target_os") return value === "unknown" || value === "none"
      if (name === "target_vendor") return value === "unknown"
      return true
    }
    // A bare name: debug builds' own, or a custom build-script cfg (off in web builds).
    return name === "debug_assertions"
  }
  return pred()
}
