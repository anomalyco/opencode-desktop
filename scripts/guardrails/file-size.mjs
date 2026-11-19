// Repo-wide Rust file-size monitor and 1,500-line exception ratchet across every tracked .rs file
// under apps/, crates/, and features/ — including files outside arch-check Rule 8
// (apps/web/, apps/installer/, apps/browser-engine/, crates/browser-pane/, and generated files).
// Runs on Bun or Node >= 18. Usage: scripts/file-size.sh [--warn-only] [--update-exceptions [--allow-raise]]
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { countLines, gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const SOFT = 800;
const HARD = 1500;
const EXC_REL = "scripts/file-size-exceptions.txt";
const EXC_PATH = join(root, EXC_REL);

const args = process.argv.slice(2);
let warnOnly = false;
let update = false;
let allowRaise = false;
for (const a of args) {
  if (a === "--warn-only") warnOnly = true;
  else if (a === "--update-exceptions") update = true;
  else if (a === "--allow-raise") allowRaise = true;
  else {
    console.error(`unknown option: ${a}`);
    process.exit(2);
  }
}

function loadExceptions() {
  const map = new Map();
  if (!existsSync(EXC_PATH)) return map;
  for (const raw of readFileSync(EXC_PATH, "utf8").split(/\r?\n/)) {
    const clean = raw.split("#")[0].trim();
    if (!clean) continue;
    const parts = clean.split(/\s+/);
    if (parts.length >= 2) {
      map.set(parts[0], Number.parseInt(parts[1], 10));
    }
  }
  return map;
}

const files = gitLsFiles(root, "apps/*.rs", "crates/*.rs", "features/*.rs");
const rows = [];
for (const p of files) {
  const full = join(root, p);
  if (!existsSync(full)) continue;
  rows.push({ path: p, lines: countLines(readFileSync(full, "utf8")) });
}

const exc = loadExceptions();
const gh = process.env.GITHUB_ACTIONS === "true";
const ann = (level, file, msg) => {
  if (gh) console.log(`::${level} file=${file}::${msg}`);
};

let errors = 0;
let warns = 0;
let oversoft = 0;
let overhard = 0;
const out = [];

for (const { path: p, lines } of rows) {
  if (lines <= SOFT) continue;
  oversoft++;
  if (lines > HARD) {
    overhard++;
    const limit = exc.get(p) ?? HARD;
    if (lines > limit) {
      errors++;
      out.push({ lines, text: `${String(lines).padStart(6)}  ERROR  ${p} (limit ${limit})` });
      ann("error", p, `${lines} lines, limit ${limit}; split a module out before adding more`);
    } else {
      warns++;
      out.push({ lines, text: `${String(lines).padStart(6)}  warn   ${p} (exception, ceiling ${limit})` });
      ann("warning", p, `${lines} lines (exception ceiling ${limit}, soft limit ${SOFT})`);
    }
  } else {
    warns++;
    out.push({ lines, text: `${String(lines).padStart(6)}  warn   ${p}` });
    ann("warning", p, `${lines} lines (soft limit ${SOFT})`);
  }
}

out.sort((a, b) => b.lines - a.lines || a.text.localeCompare(b.text));
for (const r of out) console.log(r.text);
console.log(`over ${SOFT}: ${oversoft}, over ${HARD}: ${overhard}, errors: ${errors}, warnings: ${warns}`);

if (update) {
  const entries = [];
  for (const { path: p, lines } of rows) {
    if (lines <= HARD) continue;
    const old = exc.get(p);
    if (old === undefined) {
      if (allowRaise) entries.push([p, lines]);
      else console.error(`not listing new oversized file ${p} (${lines}); use --allow-raise`);
    } else if (lines > old && !allowRaise) {
      entries.push([p, old]);
    } else {
      entries.push([p, lines]);
    }
  }
  entries.sort((a, b) => a[0].localeCompare(b[0]));
  const content = [
    `# <path> <ceiling>: line ceilings for files above ${HARD}. Ceilings only go down.`,
    `# Refresh with scripts/file-size.sh --update-exceptions.`,
    ...entries.map(([p, n]) => `${p} ${n}`),
    "",
  ].join("\n");
  writeFileSync(EXC_PATH, content, "utf8");
  console.log(`wrote ${EXC_REL} (${entries.length} entries)`);
  process.exit(0);
}

if (!warnOnly && errors > 0) process.exit(1);
