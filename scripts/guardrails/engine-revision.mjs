// Guardrail: one ENGINE_REVISION names one engine source.
//
// The release workflow builds and publishes the browser engine only when the live index lacks
// `<cef>-<ENGINE_REVISION>` (.github/release/opencode-rune-plan.sh), so a change to anything the engine
// is built from, merged without a bump, would leave `dev` at a revision whose published bytes came from
// other sources. This check compares the engine's inputs with scripts/guardrails/engine-inputs.txt:
//   - inputs changed, ENGINE_REVISION unchanged  -> fail
//   - ENGINE_REVISION changed, inputs unchanged  -> fail
//   - both changed, or neither                   -> pass (after a bump, refresh the record with --write)
// The inputs are DERIVED, so the list cannot go stale: apps/browser-engine itself, every crate it reaches
// through `path = "..."` dependencies (followed through their own Cargo.toml), and every file pulled in
// by a `#[path = "..."]` attribute under apps/browser-engine/src. Files are hashed as git blobs of the
// working tree (`git hash-object`), so line-ending settings do not matter and uncommitted edits count.
//
// Fails loudly on what it cannot read: a `path` key in a dependencies table it cannot parse, a `#[path]`
// or `include*!` whose target is not a literal (OUT_DIR excepted) or does not exist: each an error with file
// and line. A literal target is FOLLOWED and becomes an input (that is how crates/browser-pane/src/device.rs,
// outside the crate folders, is derived from apps/browser-engine/src/wire.rs).
//
// Deliberately NOT inputs: rust-toolchain.toml, the root .cargo/config.toml (Windows crt-static flags) and the
// release workflow's engine jobs: they change how the engine is built, not what the app expects of it; the
// ROOT Cargo.lock (the engine is its own workspace: root Cargo.toml excludes it). The engine workspace's OWN
// apps/browser-engine/Cargo.lock IS an input: the published engine does not carry a dependency change until a
// new revision is built. Not caught: permission-mode changes and symlinks (hashes are of contents), and a
// hand edit of the recorded hashes (visible only in a diff of engine-inputs.txt).
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

// Crate folders reached from apps/browser-engine through `path = "..."` dependencies (not dev- or build-dependencies).
function crateFolders() {
  const seen = new Set();
  const todo = [ENGINE];
  while (todo.length) {
    const dir = todo.pop();
    if (seen.has(dir)) continue;
    seen.add(dir);
    let section = "";
    for (const line of read(`${dir}/Cargo.toml`).split(/\r?\n/)) {
      const head = line.match(/^\s*\[([^\]]+)\]/);
      if (head) { section = head[1].trim(); continue; }
      if (!/(^|\.)dependencies(\.|$)/.test(section) || /dev-dependencies|build-dependencies/.test(section)) continue;
      // `name = { path = "..." }` in a [*dependencies] table, or `path = "..."` inside a [*dependencies.name] table.
      const inline = line.match(/^\s*[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"/);
      const table = /\.dependencies\.[^.]+$|^dependencies\.[^.]+$/.test(section) ? line.match(/^\s*path\s*=\s*"([^"]+)"/) : null;
      const hit = inline ?? table;
      if (hit) todo.push(norm(posix.join(dir, hit[1])));
      // Fail closed: a `path` key this reader does not understand must not be skipped silently.
      else if (/\bpath\s*=/.test(line)) throw new Error(`engine-revision: cannot read the path dependency in ${dir}/Cargo.toml [${section}]: ${line.trim()}`);
    }
  }
  return [...seen].sort();
}

// Files the crates' Rust sources pull in from elsewhere: `#[path = "..."]` and `include!` / `include_str!` /
// `include_bytes!` with a literal path (relative to the file). A non-literal include fails closed unless it
// reads OUT_DIR (a build script's output, whose inputs are already in the crate folder).
function pathFiles(folders) {
  const out = [];
  const walk = (dir) => {
    for (const e of readdirSync(join(root, dir), { withFileTypes: true })) {
      const rel = `${dir}/${e.name}`;
      if (e.isDirectory()) { if (e.name !== "target" && e.name !== "node_modules") walk(rel); continue; }
      if (!e.name.endsWith(".rs")) continue;
      const src = read(rel);
      const at = (i) => `${rel}:${src.slice(0, i).split("\n").length}`;
      const follow = (target, i, what) => {
        const f = norm(posix.join(dir, target));
        if (!existsSync(join(root, f))) throw new Error(`engine-revision: ${at(i)}: ${what} points at ${f}, which does not exist`);
        out.push(f);
      };
      for (const m of src.matchAll(/#\[path\s*=\s*([^\]]*)\]/g)) {
        const lit = m[1].trim().match(/^"([^"]+)"$/);
        if (!lit) throw new Error(`engine-revision: ${at(m.index)}: cannot read ${m[0]}`);
        follow(lit[1], m.index, "#[path]");
      }
      for (const m of src.matchAll(/\binclude(?:_str|_bytes)?!\s*\(\s*([^)]*)\)/g)) {
        const lit = m[1].trim().match(/^"([^"]+)"$/);
        if (lit) follow(lit[1], m.index, m[0].slice(0, m[0].indexOf("!") + 1));
        else if (!/env!\s*\(\s*"OUT_DIR"/.test(m[1])) throw new Error(`engine-revision: ${at(m.index)}: cannot follow a non-literal ${m[0]}`);
      }
    }
  };
  for (const d of folders) walk(d);
  return out;
}

function inputs() {
  const folders = crateFolders();
  const extra = [...new Set(pathFiles(folders))].filter((f) => !folders.some((d) => f === d || f.startsWith(`${d}/`)));
  const listed = execFileSync("git", ["ls-files", "-z", "--", ...folders, ...extra], { cwd: root, encoding: "utf8", maxBuffer: 1 << 26 })
    .split("\0").filter(Boolean).filter((f) => existsSync(join(root, f)));
  const files = [...new Set(listed)].sort();
  const blobs = files.length
    ? execFileSync("git", ["hash-object", "--stdin-paths"], { cwd: root, input: files.join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 26 }).trim().split(/\r?\n/)
    : [];
  return { sources: [...folders, ...extra], files: new Map(files.map((f, i) => [f, blobs[i]])) };
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
