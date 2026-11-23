// Moves a profile-guided optimisation profile's counts onto another build's symbol names.
//
// A `.profdata` names each function by its symbol, and every Rust (v0) symbol carries the hash of
// each crate it mentions (`Cs<base 62>_<len><crate>`; a crate-local function's profile name also
// starts with its codegen unit, `<crate>.<the same hash in hex>-cgu.<n>;`). cargo derives that hash
// from the crate's source revision and its dependencies' hashes, so a Rune pin bump changes the hash
// of every crate from the Rune repository and of every crate that depends on one, and a profile
// trained before the bump names almost nothing after it, although almost no function changed
// (apps/desktop/pgo/README.md).
//
// Given the counts (trained on the old build) and the new build's list of functions (read from the
// new instrumented exe itself, no run needed; or the profile any run of it wrote, merged without
// --sparse: both list every function with its control-flow hash), this writes a profile with the
// old counts under the new names. A function keeps its counts when its re-keyed name exists in the
// new build with the same control-flow hash, as LLVM itself requires; the rest is dropped and
// counted.
//
//   node scripts/pgo-rekey.mjs --counts <old.profdata> --names <new instrumented exe | new.profdata> --out <out.profdata>
//        [--old-names <old instrumented exe | old.profdata>] [--stamp <target>.txt --commit <sha> --rune <rev>] [--min-kept 90] [--report <file.json>]
//
// --old-names  the old build's own full list, so that which crate became which is read from two
//              complete lists; without it, from the functions that have counts.
// --stamp      rewrites the stamp's `commit` and `rune` lines and appends what was kept.
// --min-kept   fails (exit 1, nothing written) when fewer than this share of the functions, in
//              percent, keep their counts: the code has moved too far, train again.
//
// The instrumented exe is a Windows one (PE): LLVM keeps every instrumented function's profile name
// in `.lprfn` and a record per function in `.lprfd`, which the Windows runtime merges into `.data`.
import { spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { closeSync, openSync, readFileSync, readSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const opt = (n) => (process.argv.includes(`--${n}`) ? process.argv[process.argv.indexOf(`--${n}`) + 1] : undefined)

function profdata() {
  const run = (args) => spawnSync("rustc", args, { encoding: "utf8" }).stdout ?? ""
  const host = (run(["-vV"]).match(/^host: (.*)$/m)?.[1] ?? "").trim()
  return join(run(["--print", "sysroot"]).trim(), "lib", "rustlib", host, "bin", process.platform === "win32" ? "llvm-profdata.exe" : "llvm-profdata")
}

// ---- Rust v0 symbols: where the crate hashes and the back references are ----

const B62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
/** `<base-62-number>` without its `_`: "" is 0, otherwise the digits' value + 1. */
const b62 = (s) => (s === "" ? 0n : [...s].reduce((v, c) => v * 62n + BigInt(B62.indexOf(c)), 0n) + 1n)
const toB62 = (v) => {
  if (v === 0n) return ""
  let s = ""
  for (let x = v - 1n; ; x /= 62n) {
    s = B62[Number(x % 62n)] + s
    if (x < 62n) break
  }
  return s
}
/** A crate's hash as its codegen unit names it (hex, no leading zeros) from its disambiguator's digits, and back. */
const hexOf = (digits) => (b62(digits) + 1n).toString(16)
const digitsOf = (hex) => toB62(BigInt(`0x${hex}`) - 1n)

/** Parses the symbol after `_R`. Offsets are from there, as back references count them. Throws on what it does not know. */
export function parse(s) {
  let i = 0
  const crates = []
  const backrefs = []
  const fail = (what) => {
    throw new Error(`${what} at ${i}`)
  }
  const digits62 = () => {
    const from = i
    while (i < s.length && s[i] !== "_") {
      if (B62.indexOf(s[i]) < 0) fail("base 62")
      i++
    }
    if (s[i] !== "_") fail("base 62 end")
    return s.slice(from, i++)
  }
  const backref = () => {
    const at = i++
    const target = Number(b62(digits62()))
    if (target >= at) fail("back reference forward")
    backrefs.push({ at, end: i, target })
  }
  const disambiguator = () => {
    if (s[i] !== "s") return undefined
    const at = i++
    return { at, digits: digits62(), end: i }
  }
  const plain = () => {
    if (s[i] === "u") i++
    const from = i
    // "0" alone is an empty name (a closure's); no other length starts with 0.
    if (s[i] === "0") i++
    else while (s[i] >= "0" && s[i] <= "9") i++
    if (from === i) fail("identifier length")
    const n = Number(s.slice(from, i))
    if (s[i] === "_") i++
    if (i + n > s.length) fail("identifier bytes")
    const name = s.slice(i, i + n)
    i += n
    return name
  }
  const identifier = () => {
    const d = disambiguator()
    return { d, name: plain() }
  }
  const lifetime = () => {
    i++
    digits62()
  }
  const binder = () => {
    if (s[i] === "G") {
      i++
      digits62()
    }
  }
  const list = (what, item) => {
    while (s[i] !== "E") {
      if (i >= s.length) fail(what)
      item()
    }
    i++
  }
  const BASIC = "abcdefhijlmnopstuvxyz"
  function path() {
    const c = s[i]
    if (c === "C") {
      i++
      const id = identifier()
      if (id.d) crates.push({ at: id.d.at, end: id.d.end, digits: id.d.digits, name: id.name })
    } else if (c === "M") {
      i++
      disambiguator()
      path()
      type()
    } else if (c === "X") {
      i++
      disambiguator()
      path()
      type()
      path()
    } else if (c === "Y") {
      i++
      type()
      path()
    } else if (c === "N") {
      i += 2
      path()
      identifier()
    } else if (c === "I") {
      i++
      path()
      list("generic arguments", () => {
        if (s[i] === "L") lifetime()
        else if (s[i] === "K") {
          i++
          constant()
        } else type()
      })
    } else if (c === "B") backref()
    else fail(`path '${c}'`)
  }
  function type() {
    const c = s[i]
    if (c === "B") backref()
    else if (c === "A") {
      i++
      type()
      constant()
    } else if (c === "S" || c === "P" || c === "O") {
      i++
      type()
    } else if (c === "T") {
      i++
      list("tuple", type)
    } else if (c === "R" || c === "Q") {
      i++
      if (s[i] === "L") lifetime()
      type()
    } else if (c === "F") {
      i++
      binder()
      if (s[i] === "U") i++
      if (s[i] === "K") {
        i++
        if (s[i] === "C") i++
        else plain()
      }
      list("fn arguments", type)
      type()
    } else if (c === "D") {
      i++
      binder()
      list("dyn bounds", () => {
        path()
        while (s[i] === "p") {
          i++
          plain()
          type()
        }
      })
      if (s[i] !== "L") fail("dyn lifetime")
      lifetime()
    } else if (c !== undefined && "CMXYNI".includes(c)) path()
    else if (c !== undefined && BASIC.includes(c)) i++
    else fail(`type '${c}'`)
  }
  function constant() {
    const c = s[i]
    if (c === "B") backref()
    else if (c === "p") i++
    else if (c === "R" || c === "Q") {
      i++
      constant()
    } else if (c === "A" || c === "T") {
      i++
      list("const list", constant)
    } else if (c === "V") {
      i++
      path()
      if (s[i] === "U") i++
      else if (s[i] === "T") {
        i++
        list("const fields", constant)
      } else if (s[i] === "S") {
        i++
        list("const fields", () => {
          identifier()
          constant()
        })
      } else fail("const adt")
    } else if (c !== undefined && BASIC.includes(c)) {
      i++
      if (s[i] === "n") i++
      while (i < s.length && s[i] !== "_") {
        if (!/[0-9a-f]/.test(s[i])) fail("const digits")
        i++
      }
      if (s[i] !== "_") fail("const end")
      i++
    } else fail(`const '${c}'`)
  }
  path()
  // The crate that instantiated a generic function; then at most a suffix LLVM or the linker added.
  if (i < s.length && s[i] !== "." && s[i] !== "$") path()
  if (i < s.length && s[i] !== "." && s[i] !== "$") fail("trailing text")
  return { crates, backrefs }
}

/** `map` gives a crate's new disambiguator digits from its name and old digits (undefined: unchanged). */
export function rewriteSymbol(symbol, map) {
  if (!symbol.startsWith("_R")) return symbol
  const s = symbol.slice(2)
  const p = parse(s)
  const edits = [
    ...p.crates.flatMap((c) => {
      const to = map(c.name, c.digits)
      return to === undefined || to === c.digits ? [] : [{ at: c.at, end: c.end, text: `s${to}_` }]
    }),
    ...p.backrefs.map((b) => ({ at: b.at, end: b.end, target: b.target })),
  ].sort((a, b) => a.at - b.at)
  if (!edits.some((e) => e.text !== undefined)) return symbol
  let out = ""
  let from = 0
  const moved = [] // [old offset, shift]: from this old offset on, new = old + shift
  const shift = (old) => {
    let d = 0
    for (const [at, by] of moved) {
      if (at > old) break
      d = by
    }
    return d
  }
  for (const e of edits) {
    out += s.slice(from, e.at)
    // A back reference names the offset of what it repeats, which moved by what changed before it.
    out += e.text ?? `B${toB62(BigInt(e.target + shift(e.target)))}_`
    from = e.end
    moved.push([e.end, out.length - e.end])
  }
  return `_R${out}${s.slice(from)}`
}

const UNIT = /^(.*)\.([0-9a-f]{1,16})(-cgu\.\d+)$/

/** A profile name: a symbol, or `<codegen unit>;<symbol>` for a function local to its crate. */
export function rewriteName(name, map) {
  const cut = name.indexOf(";")
  if (cut < 0) return rewriteSymbol(name, map)
  const unit = name.slice(0, cut).replace(UNIT, (all, crate, hex, rest) => {
    const to = map(crate, digitsOf(hex))
    return to === undefined ? all : `${crate}.${hexOf(to)}${rest}`
  })
  return `${unit};${rewriteSymbol(name.slice(cut + 1), map)}`
}

/** Every crate hash a profile name mentions, as [crate, digits]. */
export function cratesOf(name) {
  const cut = name.indexOf(";")
  const out = []
  const unit = cut < 0 ? undefined : name.slice(0, cut).match(UNIT)
  if (unit) out.push([unit[1], digitsOf(unit[2])])
  const symbol = name.slice(cut + 1)
  if (symbol.startsWith("_R")) for (const c of parse(symbol.slice(2)).crates) out.push([c.name, c.digits])
  return out
}

// ---- a build's functions: from a profile's text, or from the instrumented exe ----

/** A profile as records { name, hash, lines } (the lines after the name), through `llvm-profdata merge --text`. */
export function read(file, tool = profdata()) {
  const text = `${file}.rekey.proftext`
  if (spawnSync(tool, ["merge", "--text", "-o", text, file], { stdio: "inherit" }).status !== 0) throw new Error(`llvm-profdata could not read ${file}`)
  const lines = readFileSync(text, "utf8").split(/\r?\n/)
  rmSync(text, { force: true })
  const head = []
  const records = []
  let at = 0
  while (at < lines.length && (lines[at].startsWith("#") || lines[at].startsWith(":") || lines[at] === "")) head.push(lines[at++])
  while (at < lines.length) {
    if (lines[at] === "") {
      at++
      continue
    }
    const name = lines[at++]
    const body = []
    while (at < lines.length && lines[at] !== "") body.push(lines[at++])
    const h = body.indexOf("# Func Hash:")
    records.push({ name, hash: h >= 0 ? body[h + 1] : "", lines: body })
  }
  return { head: head.filter((l) => l !== ""), records }
}

/** The functions of an instrumented Windows exe, read from the file itself: the names from `.lprfn`, and each
 *  function's record (the name's MD5, then its control-flow hash) found in `.data` by that MD5. */
export function readExe(file) {
  const fd = openSync(file, "r")
  const at = (offset, size) => {
    const b = Buffer.alloc(size)
    readSync(fd, b, 0, size, offset)
    return b
  }
  const head = at(0, 4096)
  const pe = head.readUInt32LE(0x3c)
  const sections = new Map()
  for (let i = 0, t = pe + 24 + head.readUInt16LE(pe + 20); i < head.readUInt16LE(pe + 6); i++, t += 40) {
    const name = head.subarray(t, t + 8).toString("latin1").replace(/\0+$/, "")
    if (name === ".lprfn" || name === ".data") sections.set(name, at(head.readUInt32LE(t + 20), Math.min(head.readUInt32LE(t + 8), head.readUInt32LE(t + 16))))
  }
  closeSync(fd)
  const names = sections.get(".lprfn")
  const data = sections.get(".data")
  if (!names || !data) throw new Error(`${file} has no .lprfn section: it is not an instrumented build`)
  const all = []
  for (let i = 0; i < names.length; ) {
    if (names[i] === 0) {
      i++
      continue
    }
    const leb = () => {
      let v = 0
      for (let by = 0; ; by += 7) {
        const b = names[i++]
        v += (b & 0x7f) * 2 ** by
        if (!(b & 0x80)) return v
      }
    }
    const size = leb()
    if (leb()) throw new Error(`${file}: its names are compressed (an LLVM built with zlib): give a run's profile, merged without --sparse, instead`)
    all.push(...names.subarray(i, i + size).toString("utf8").split("\x01"))
    i += size
  }
  const byRef = new Map(all.map((n) => [createHash("md5").update(n).digest().readBigUInt64LE(0), n]))
  const records = []
  const seen = new Set()
  for (let i = 0; i + 16 <= data.length; i += 8) {
    const n = byRef.get(data.readBigUInt64LE(i))
    if (n === undefined || seen.has(n)) continue
    seen.add(n)
    records.push({ name: n, hash: data.readBigUInt64LE(i + 8).toString(), lines: [] })
  }
  return { head: [], records }
}

function main() {
  const countsFile = opt("counts")
  const namesFile = opt("names")
  const outFile = opt("out")
  if (!countsFile || !namesFile || !outFile) {
    console.error("usage: node scripts/pgo-rekey.mjs --counts <old.profdata> --names <new exe | new.profdata> --out <out.profdata> [--old-names <old exe | old.profdata>] [--stamp <file> --commit <sha> --rune <rev>] [--min-kept 90] [--report <file.json>]")
    process.exit(2)
  }
  const tool = profdata()
  const readAny = (file) => (/\.exe$/i.test(file) ? readExe(file) : read(file, tool))
  const old = read(countsFile, tool)
  const now = readAny(namesFile)
  const oldList = opt("old-names") ? readAny(opt("old-names")).records : old.records
  const unparsed = { old: 0, new: 0 }
  const ids = (records, side) => {
    const m = new Map()
    for (const r of records) {
      let found
      try {
        found = cratesOf(r.name)
      } catch {
        unparsed[side]++
        continue
      }
      for (const [crate, digits] of found) (m.get(crate) ?? m.set(crate, new Set()).get(crate)).add(digits)
    }
    return m
  }
  const before = ids(oldList, "old")
  const after = ids(now.records, "new")
  // A hash on both sides is the same crate built the same way. What is left on each side of a crate's name is the
  // crate before and after: one of each maps at once.
  const map = new Map()
  const open = []
  const ambiguous = []
  for (const [crate, was] of before) {
    const is = after.get(crate) ?? new Set()
    const gone = [...was].filter((d) => !is.has(d))
    const came = [...is].filter((d) => !was.has(d))
    if (gone.length === 1 && came.length === 1) map.set(`${crate} ${gone[0]}`, came[0])
    else if (gone.length && came.length) open.push({ crate, gone, came })
  }
  const lookup = (crate, digits) => map.get(`${crate} ${digits}`)
  const target = new Map(now.records.map((r) => [r.name, r.hash]))
  // One name, several crates that all changed (a package's library and its program share a name): each old hash goes
  // to the new one under which the most of its functions exist in the new build.
  for (const o of open) {
    const mine = oldList.filter((r) => r.name.includes(o.crate))
    const left = new Set(o.came)
    for (const g of o.gone) {
      const users = mine.filter((r) => {
        try {
          return cratesOf(r.name).some(([c, d]) => c === o.crate && d === g)
        } catch {
          return false
        }
      })
      const score = (c) => users.filter((r) => target.has(rewriteName(r.name, (crate, d) => (crate === o.crate && d === g ? c : crate === o.crate ? undefined : lookup(crate, d))))).length
      const best = [...left].map((c) => ({ c, n: score(c) })).sort((a, b) => b.n - a.n)[0]
      if (best && best.n > users.length / 2) {
        map.set(`${o.crate} ${g}`, best.c)
        left.delete(best.c)
      } else ambiguous.push(`${o.crate} ${g}: ${users.length} functions, best match ${best?.n ?? 0}`)
    }
  }
  const stats = { functions: old.records.length, kept: 0, renamed: 0, no_such_function: 0, code_changed: 0, unparsed: 0 }
  const out = [...old.head, ""]
  for (const r of old.records) {
    let name
    try {
      name = rewriteName(r.name, lookup)
    } catch {
      stats.unparsed++
      continue
    }
    const hash = target.get(name)
    if (hash === undefined) {
      stats.no_such_function++
      continue
    }
    if (hash !== r.hash) {
      stats.code_changed++
      continue
    }
    stats.kept++
    if (name !== r.name) stats.renamed++
    // An indirect call's targets are names too (`<name>:<count>`).
    let indirect = false
    const body = r.lines.map((l) => {
      if (l.startsWith("# ValueKind = ")) indirect = l.includes("IPVK_IndirectCallTarget")
      if (!indirect || l.startsWith("#") || !l.includes(":")) return l
      const cut = l.lastIndexOf(":")
      try {
        return `${rewriteName(l.slice(0, cut), lookup)}${l.slice(cut)}`
      } catch {
        return l
      }
    })
    out.push(name, ...body, "")
  }
  const report = { counts: countsFile, names: namesFile, out: outFile, crates_rekeyed: map.size, crates_ambiguous: ambiguous, new_build_functions: now.records.length, unparsed_names: unparsed, ...stats }
  if (opt("report")) writeFileSync(opt("report"), JSON.stringify(report, null, 1))
  console.log(JSON.stringify(report, null, 1))
  const share = stats.functions ? (stats.kept / stats.functions) * 100 : 0
  if (share < Number(opt("min-kept") ?? 0)) {
    console.error(`pgo-rekey: only ${share.toFixed(1)} % of the functions keep their counts (--min-kept ${opt("min-kept")}): the code moved too far, train again`)
    process.exit(1)
  }
  const text = `${outFile}.proftext`
  writeFileSync(text, out.join("\n"))
  const merged = spawnSync(tool, ["merge", "--sparse", "-o", outFile, text], { stdio: "inherit" }).status
  rmSync(text, { force: true })
  if (merged !== 0) throw new Error("llvm-profdata could not write the re-keyed profile")
  const stamp = opt("stamp")
  if (stamp) {
    const kept = readFileSync(stamp, "utf8").split(/\r?\n/).filter((l) => l !== "" && !/^(commit|rune|Total functions:|re-keyed:) /.test(l))
    const lines = [
      ...(opt("commit") ? [`commit ${opt("commit")}`] : []),
      ...(opt("rune") ? [`rune ${opt("rune")}`] : []),
      ...kept,
      `Total functions: ${stats.kept}`,
      `re-keyed: ${stats.kept} of ${stats.functions} functions kept, ${stats.code_changed} dropped (their code changed), ${stats.no_such_function} gone`,
    ]
    writeFileSync(stamp, `${lines.join("\n")}\n`)
  }
}

if (process.argv[1] && import.meta.url.endsWith(process.argv[1].replace(/\\/g, "/").split("/").pop())) main()
