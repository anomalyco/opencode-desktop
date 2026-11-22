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
      if (head) section = head[1];
      const dep = line.match(/^\s*[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"/);
      if (dep && !/dev-dependencies|build-dependencies/.test(section)) todo.push(norm(posix.join(dir, dep[1])));
    }
  }
  return [...seen].sort();
}

// Files outside the crate folders that apps/browser-engine/src pulls in with `#[path = "..."]`.
function pathFiles() {
  const out = [];
  const walk = (dir) => {
    for (const e of readdirSync(join(root, dir), { withFileTypes: true })) {
      const rel = `${dir}/${e.name}`;
      if (e.isDirectory()) walk(rel);
      else if (e.name.endsWith(".rs")) {
        for (const m of read(rel).matchAll(/#\[path\s*=\s*"([^"]+)"\]/g)) out.push(norm(posix.join(dir, m[1])));
      }
    }
  };
  walk(`${ENGINE}/src`);
  return out;
}

function inputs() {
  const folders = crateFolders();
  const extra = pathFiles().filter((f) => !folders.some((d) => f === d || f.startsWith(`${d}/`)));
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

const rev = revision();
const cur = inputs();
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
