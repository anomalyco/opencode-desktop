// Guardrail: one ENGINE_REVISION names one engine source.
//
// The release workflow builds and publishes the browser engine only when the live index lacks
// `<cef>-<ENGINE_REVISION>` (.github/release/opencode-rune-plan.sh), so a change to anything the engine
// is built from, merged without a bump, would leave `dev` at a revision whose published bytes came from
// other sources. This check compares the engine's inputs with scripts/guardrails/engine-inputs.txt:
//   - inputs changed, ENGINE_REVISION unchanged  -> fail
//   - ENGINE_REVISION changed, inputs unchanged  -> fail
//   - both changed, or neither                   -> pass (after a bump, refresh the record with --write)
// The inputs are DERIVED, so the list cannot go stale (see inputs() for the walk). Files are hashed as git
// blobs of the working tree (`git hash-object`), so line-ending settings do not matter and uncommitted edits count.
//
// Fails loudly on what it cannot read, with file and line: a `path` key in a table it does not understand or
// with a value it cannot parse, a `#[path]` / `include*!` target that is not a literal (an OUT_DIR include is
// allowed unless it contains `..`) or does not exist, and a `mod x;` without `#[path]` in a file followed from
// outside the crate folders. A literal target is FOLLOWED and becomes an input (that is how
// crates/browser-pane/src/device.rs, outside the crate folders, is derived from apps/browser-engine/src/wire.rs).
//
// Deliberately NOT inputs: rust-toolchain.toml, the root .cargo/config.toml (Windows crt-static flags) and the
// release workflow's engine jobs: they change how the engine is built, not what the app expects of it; the
// ROOT Cargo.lock (the engine is its own workspace: root Cargo.toml excludes it). The engine workspace's OWN
// apps/browser-engine/Cargo.lock IS an input: the published engine does not carry a dependency change until a
// new revision is built. Not caught: permission-mode changes and symlinks (hashes are of contents), and a
// hand edit of the recorded hashes (visible only in a diff of engine-inputs.txt).
//
// KNOWN LIMIT: a build.rs that reads files with std::fs (or a proc macro that does) at build time: those files
// are inputs only if they sit inside a crate folder. Today the engine's build.rs reads only environment
// variables and its own manifest inside apps/browser-engine. Also: a comment containing `path =` directly
// above a bare `mod x;` hides that line from the bare-mod check.
//
//   node scripts/guardrails/engine-revision.mjs            check
//   node scripts/guardrails/engine-revision.mjs --write    refresh the record (after an ENGINE_REVISION bump)
//   node scripts/guardrails/engine-revision.mjs --list     print the derived input paths
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, posix, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const RECORD = "scripts/guardrails/engine-inputs.txt";
const INSTALLER = "crates/browser-pane/src/installer.rs";
const ENGINE = "apps/browser-engine";

const read = (rel) => readFileSync(join(root, rel), "utf8");
const norm = (p) => posix.normalize(p.replaceAll("\\", "/")).replace(/\/$/, "");

function revision() {
  const m = read(INSTALLER).match(/^pub const ENGINE_REVISION: u32 = (\d+);$/m);
  if (!m) throw new Error(`engine-revision: cannot read ENGINE_REVISION from ${INSTALLER}`);
  return Number(m[1]);
}

// Every input, walked to a fixed point from apps/browser-engine:
//   - a crate folder (whole) for every path in a manifest's [dependencies] / [build-dependencies] (also
//     target-specific, inline or [deps.name] table form), [workspace.dependencies], [patch.*] and [replace];
//     [dev-dependencies], [[test]], [[bench]] and [[example]] are skipped (they do not shape the engine);
//   - a file for every [lib] / [[bin]] `path`, [package] `build`, `#[path]`, `#[cfg_attr(.., path = ..)]`,
//     `include!`, `include_str!` and `include_bytes!` target; a followed Rust file is scanned in turn.
// A followed file outside the crate folders may not declare `mod x;` without its own `#[path]`: where that
// module lives is not derivable here, so it is an error.
function inputs() {
  const folders = new Set();
  const files = new Set();
  const mods = [];
  const todo = [{ crate: ENGINE }];
  while (todo.length) {
    const next = todo.pop();
    if (next.crate) {
      if (folders.has(next.crate)) continue;
      if (!existsSync(join(root, next.crate, "Cargo.toml"))) throw new Error(`engine-revision: ${next.from}: points at ${next.crate}, which has no Cargo.toml`);
      folders.add(next.crate);
      todo.push(...manifest(next.crate));
      for (const f of tracked([next.crate]).filter((f) => f.endsWith(".rs"))) {
        const r = rust(f);
        todo.push(...r.targets);
        mods.push(...r.mods);
      }
      continue;
    }
    if (files.has(next.file)) continue;
    files.add(next.file);
    if (!next.rust) continue;
    const r = rust(next.file);
    todo.push(...r.targets);
    mods.push(...r.mods);
  }
  const inFolder = (f) => [...folders].some((d) => f === d || f.startsWith(`${d}/`));
  const orphan = mods.find((m) => !inFolder(m.file));
  if (orphan) throw new Error(`engine-revision: ${orphan.at}: \`mod ${orphan.name};\` in a file followed from outside the crate folders: give it a #[path] so its file can be followed`);
  const sorted = [...folders].sort();
  const extra = [...files].filter((f) => !inFolder(f)).sort();
  const listed = [...new Set(tracked([...sorted, ...extra]).filter((f) => existsSync(join(root, f))))].sort();
  const blobs = listed.length
    ? execFileSync("git", ["hash-object", "--stdin-paths"], { cwd: root, input: listed.join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 26 }).trim().split(/\r?\n/)
    : [];
  return { sources: [...sorted, ...extra], files: new Map(listed.map((f, i) => [f, blobs[i]])) };
}

function tracked(paths) {
  return execFileSync("git", ["ls-files", "-z", "--", ...paths], { cwd: root, encoding: "utf8", maxBuffer: 1 << 26 }).split("\0").filter(Boolean);
}

const STR = String.raw`(?:"([^"]+)"|'([^']+)')`;
const KEY = String.raw`("[^"]*"|'[^']*'|[A-Za-z0-9_-]+)`;
const DEPS = /^(dependencies|build-dependencies|build_dependencies)$/;
const unquote = (s) => s.replace(/^["']|["']$/g, "");
const manifestOf = (dir) => (dir === "." ? "Cargo.toml" : `${dir}/Cargo.toml`);

// The path-bearing keys of one Cargo.toml, as crate or file targets. A `path` key in a table this reader does
// not understand, or a value it cannot read, is an error with file and line, never skipped. A dependency
// inherited with `workspace = true` is resolved through the crate's workspace root (see inherited()).
function manifest(dir) {
  const rows = toml(manifestOf(dir));
  const out = [];
  for (const r of rows) {
    const s = r.s[0] === "target" ? r.s.slice(2) : r.s;
    const line = r.line;
    if (/^dev[-_]dependencies$/.test(s[0]) || (r.array && ["test", "bench", "example"].includes(s[0]))) continue;
    const table = (k) => (DEPS.test(s[0]) && s.length === k) || (s[0] === "workspace" && s[1] === "dependencies" && s.length === k + 1) || (s[0] === "patch" && s.length === k + 1) || (s[0] === "replace" && s.length === k);
    const kind = table(1) ? "inline"
      : table(2) ? "table"
      : s.length === 1 && (s[0] === "lib" || (r.array && s[0] === "bin")) ? "target"
      : s.length === 1 && s[0] === "package" ? "package"
      : "other";
    if (DEPS.test(s[0])) {
      const name = s.length === 2
        ? (/^\s*workspace\s*=\s*true\s*(#.*)?$/.test(line) ? s[1] : null)
        : (line.match(new RegExp(String.raw`^\s*${KEY}\s*=\s*\{[^}]*\bworkspace\s*=\s*true\b`)) ?? line.match(new RegExp(String.raw`^\s*${KEY}\.workspace\s*=\s*true\s*(#.*)?$`)))?.[1];
      if (name) {
        const crate = inherited(dir, rows, unquote(name), r.at);
        if (crate) out.push({ crate, from: r.at });
        continue;
      }
      if (/\bworkspace\s*=/.test(line)) throw new Error(`engine-revision: ${r.at}: cannot read the workspace inheritance in ${line.trim()}`);
    }
    const val = (re) => { const m = line.match(re); return m ? norm(posix.join(dir, m[1] ?? m[2])) : null; };
    const crate = kind === "inline" ? val(new RegExp(String.raw`^\s*${KEY.replace("(", "(?:")}\s*=\s*\{[^}]*\bpath\s*=\s*${STR}`)) ?? val(new RegExp(String.raw`^\s*${KEY.replace("(", "(?:")}\.path\s*=\s*${STR}`))
      : kind === "table" ? val(new RegExp(String.raw`^\s*path\s*=\s*${STR}`))
      : null;
    if (crate) { out.push({ crate, from: r.at }); continue; }
    const target = kind === "target" ? val(new RegExp(String.raw`^\s*path\s*=\s*${STR}`))
      : kind === "package" ? val(new RegExp(String.raw`^\s*build\s*=\s*${STR}`))
      : null;
    if (target) { out.push({ file: must(target, r.at, "a manifest path"), rust: true }); continue; }
    if (kind === "package" && /^\s*build\s*=/.test(line) && !/^\s*build\s*=\s*(true|false)\s*(#.*)?$/.test(line)) throw new Error(`engine-revision: ${r.at}: cannot read ${line.trim()}`);
    if (/\bpath\s*=/.test(line)) throw new Error(`engine-revision: ${r.at}: cannot read the path in ${line.trim()}`);
  }
  return out;
}

// `x.workspace = true` in a member crate takes x from its WORKSPACE ROOT's [workspace.dependencies], even when
// the engine (its own workspace) is what is being built. Returns x's crate folder there, or null when x is not a
// path dependency; a root or an entry that cannot be found is an error.
function inherited(dir, rows, name, at) {
  const ws = workspaceRoot(dir, rows, at);
  const deps = new Map();
  for (const r of toml(manifestOf(ws)).filter((r) => r.s[0] === "workspace" && r.s[1] === "dependencies")) {
    const key = r.s.length === 3 ? r.s[2] : unquote(r.line.match(new RegExp(String.raw`^\s*${KEY}\s*[=.]`))?.[1] ?? "");
    if (!key) continue;
    const p = r.line.match(new RegExp(String.raw`(?:^\s*|\{[^}]*\b|^\s*${KEY.replace("(", "(?:")}\.)path\s*=\s*${STR}`));
    if (!p && /\bpath\s*=/.test(r.line)) throw new Error(`engine-revision: ${r.at}: cannot read the path in ${r.line.trim()}`);
    if (p) deps.set(key, norm(posix.join(ws, p[1] ?? p[2])));
    else if (!deps.has(key)) deps.set(key, null);
  }
  if (!deps.has(name)) throw new Error(`engine-revision: ${at}: ${name} is inherited from ${manifestOf(ws)}, which has no [workspace.dependencies] entry for it`);
  return deps.get(name);
}

// The crate's workspace root: [package] workspace = "..." if set, else the nearest folder at or above it whose
// Cargo.toml has a [workspace] table.
function workspaceRoot(dir, rows, at) {
  const set = rows.find((r) => r.s.length === 1 && r.s[0] === "package" && /^\s*workspace\s*=/.test(r.line));
  if (set) {
    const m = set.line.match(new RegExp(String.raw`^\s*workspace\s*=\s*${STR}`));
    if (!m) throw new Error(`engine-revision: ${set.at}: cannot read ${set.line.trim()}`);
    return norm(posix.join(dir, m[1] ?? m[2])) || ".";
  }
  for (let d = dir; ; d = posix.dirname(d)) {
    if (existsSync(join(root, manifestOf(d))) && /^\s*\[\s*workspace\s*\]/m.test(read(manifestOf(d)))) return d;
    if (d === ".") throw new Error(`engine-revision: ${at}: inherits a workspace dependency, but no workspace root was found above ${dir}`);
  }
}

// A Cargo.toml as rows: each non-blank, non-comment line with its table path (segments, unquoted) and file:line.
function toml(file) {
  let s = [];
  let array = false;
  return read(file).split(/\r?\n/).flatMap((line, n) => {
    const head = line.match(/^\s*(\[\[?)\s*([^\]]+?)\s*\]\]?\s*(#.*)?$/);
    if (head) {
      s = (head[2].match(/"[^"]*"|'[^']*'|[^.\s]+/g) ?? []).map(unquote);
      array = head[1] === "[[";
      return [];
    }
    if (/^\s*(#|$)/.test(line)) return [];
    return [{ at: `${file}:${n + 1}`, line, s, array }];
  });
}

// What one Rust file pulls in: `#[path]`, `#[cfg_attr(.., path = ..)]` and `include*!` targets (literal paths,
// relative to the file; an `include*!(concat!(env!("OUT_DIR"), ..))` is a build output and allowed unless it
// climbs out with `..`), and its `mod x;` lines without a path attribute.
function rust(file) {
  const src = read(file);
  const dir = posix.dirname(file);
  const at = (i) => `${file}:${src.slice(0, i).split("\n").length}`;
  const targets = [];
  const lit = (text, i, what) => {
    const m = text.trim().match(/^"([^"]+)"$/);
    if (!m) throw new Error(`engine-revision: ${at(i)}: cannot follow a non-literal ${what}`);
    return must(norm(posix.join(dir, m[1])), at(i), what);
  };
  for (const m of src.matchAll(/#\[\s*path\s*=\s*([^\]]*)\]/g)) targets.push({ file: lit(m[1], m.index, m[0]), rust: true });
  for (const m of src.matchAll(/#\[\s*cfg_attr\s*\(/g)) {
    const body = args(src, m.index + m[0].length - 1, at(m.index));
    for (const p of body.matchAll(/\bpath\s*=\s*([^,]*)/g)) targets.push({ file: lit(p[1], m.index, `#[cfg_attr(.., ${p[0].trim()})]`), rust: true });
  }
  for (const m of src.matchAll(/\binclude(_str|_bytes)?!\s*([(\[{])/g)) {
    const body = args(src, m.index + m[0].length - 1, at(m.index));
    const what = `${m[0].replace(/\s*[(\[{]$/, "")}(${body.trim()})`;
    if (/env!\s*\(\s*"OUT_DIR"\s*\)/.test(body)) {
      if (body.includes("..")) throw new Error(`engine-revision: ${at(m.index)}: ${what} climbs out of OUT_DIR`);
      continue;
    }
    targets.push({ file: lit(body, m.index, what), rust: !m[1] });
  }
  const lines = src.split("\n");
  const mods = lines.flatMap((line, n) => {
    const m = line.match(/^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;/);
    if (!m) return [];
    const attrs = [];
    for (let k = n - 1; k >= 0 && /^\s*(#\[|\/\/)/.test(lines[k]); k--) attrs.push(lines[k]);
    return attrs.some((a) => /\bpath\s*=/.test(a)) ? [] : [{ file, name: m[1], at: `${file}:${n + 1}` }];
  });
  return { targets, mods };
}

function must(f, at, what) {
  if (!existsSync(join(root, f))) throw new Error(`engine-revision: ${at}: ${what} points at ${f}, which does not exist`);
  return f;
}

// The text between a macro's or attribute's opening bracket at `open` and its matching close.
function args(src, open, at) {
  const close = { "(": ")", "[": "]", "{": "}" };
  const stack = [];
  for (let i = open; i < src.length; i++) {
    const c = src[i];
    if (c === '"') {
      for (i++; i < src.length && src[i] !== '"'; i++) if (src[i] === "\\") i++;
      continue;
    }
    if (Object.hasOwn(close, c)) stack.push(close[c]);
    else if (c === stack.at(-1)) {
      stack.pop();
      if (!stack.length) return src.slice(open + 1, i);
    }
  }
  throw new Error(`engine-revision: ${at}: cannot find the end of this macro's arguments`);
}

function readRecord() {
  if (!existsSync(join(root, RECORD))) return null;
  const rec = { revision: NaN, files: new Map() };
  for (const line of read(RECORD).split(/\r?\n/)) {
    if (!line.trim() || line.startsWith("#")) continue;
    const rev = line.match(/^revision (\d+)$/);
    if (rev) { rec.revision = Number(rev[1]); continue; }
    const f = line.match(/^([0-9a-f]{40}) (.+)$/);
    if (f) rec.files.set(f[2], f[1]);
  }
  return rec;
}

function writeRecord(rev, cur, sameRevisionReason) {
  const lines = [
    "# Written by scripts/guardrails/engine-revision.mjs --write: the engine's inputs at this ENGINE_REVISION.",
    `# Inputs: ${cur.sources.join(", ")}`,
    ...(sameRevisionReason ? [`# same-revision: ${sameRevisionReason}`] : []),
    `revision ${rev}`,
    ...[...cur.files].map(([f, b]) => `${b} ${f}`),
  ];
  writeFileSync(join(root, RECORD), lines.join("\n") + "\n");
}

let rev, cur;
try {
  rev = revision();
  cur = inputs();
} catch (e) {
  console.error(e instanceof Error ? e.message : String(e));
  process.exit(1);
}
if (process.argv.includes("--list")) {
  for (const s of cur.sources) console.log(s);
  process.exit(0);
}
if (process.argv.includes("--write")) {
  // The escape hatch is a line in the committed record, never a label: refreshing the record at the SAME
  // revision with changed inputs needs `--same-revision "<reason>"`, and the reason lands in the PR diff.
  const old = readRecord();
  const i = process.argv.indexOf("--same-revision");
  const reason = i >= 0 ? (process.argv[i + 1] ?? "").trim() : "";
  const differs = old && [...new Set([...old.files.keys(), ...cur.files.keys()])].some((f) => old.files.get(f) !== cur.files.get(f));
  if (old && old.revision !== rev && !differs) {
    console.error(`engine-revision: refusing --write: ENGINE_REVISION moved ${old.revision} -> ${rev} but none of the engine's inputs changed; undo the bump, or make the engine change it is for`);
    process.exit(1);
  }
  if (old && old.revision === rev && differs && !reason) {
    console.error(`engine-revision: refusing --write: the inputs changed but ENGINE_REVISION is still ${rev}; bump it first, or pass --same-revision "<why the engine's bytes may stay the same>"`);
    process.exit(1);
  }
  writeRecord(rev, cur, old && old.revision === rev && differs ? reason : "");
  console.log(`engine-revision: recorded ${cur.files.size} input files at ENGINE_REVISION ${rev} in ${RECORD}`);
  process.exit(0);
}

const rec = readRecord();
if (!rec) {
  console.error(`engine-revision: ${RECORD} is missing: run \`node scripts/guardrails/engine-revision.mjs --write\` and commit it`);
  process.exit(1);
}
const changed = [...new Set([...rec.files.keys(), ...cur.files.keys()])].filter((f) => rec.files.get(f) !== cur.files.get(f)).sort();
const bumped = rev !== rec.revision;
const shown = changed.slice(0, 5).join(", ") + (changed.length > 5 ? ` and ${changed.length - 5} more` : "");

if (changed.length && !bumped) {
  console.error(`engine-revision: FAIL: the browser engine's inputs changed (${shown}) but ENGINE_REVISION is still ${rev} (${INSTALLER}); bump it, then run \`node scripts/guardrails/engine-revision.mjs --write\``);
  process.exit(1);
}
if (bumped && !changed.length) {
  console.error(`engine-revision: FAIL: ENGINE_REVISION changed ${rec.revision} -> ${rev} but none of the engine's inputs did (${cur.sources.join(", ")}); undo the bump, or make the engine change it is for`);
  process.exit(1);
}
if (bumped) {
  console.error(`engine-revision: FAIL: ENGINE_REVISION ${rec.revision} -> ${rev} with ${changed.length} input file(s) changed: refresh the record with \`node scripts/guardrails/engine-revision.mjs --write\` and commit it`);
  process.exit(1);
}
console.log(`engine-revision: ok (ENGINE_REVISION ${rev}; ${cur.files.size} input files under ${cur.sources.join(", ")} match ${RECORD})`);
