// Checks folder shape for tracked sources (prefix families and folder breadth).
// Runs on Bun or Node >= 18 (no Python dependency).
// Usage: scripts/folder-shape.sh [--warn-only] [--update-exceptions [--allow-raise]]
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const WARN = 40;
const HARD = 60;
const EXC_REL = "scripts/folder-shape-exceptions.txt";
const EXC_PATH = join(root, EXC_REL);

const SOURCE = /\.(rs|tsx?|swift)$/;
const SKIPPED =
  /(^|\/)(node_modules|vendor|vendored|third_party|target|dist|build|\.build|out|generated|__generated__|gen|fixtures?|Pods|DerivedData)\/|\.d\.ts$|\.(generated|gen|g|pb)\.(rs|tsx?|swift)$|_generated\.rs$/;
const TEST =
  /(^|\/)(tests?|benches|e2e|__tests__)\/|(^|\/)[^/]*Tests\/|(^|\/)tests?\.rs$|_tests?\.rs$|\.(test|spec)\.tsx?$|Tests\.swift$/;
const PROGRAMS = /(^|\/)(examples|benches|bin)$/;

const args = process.argv.slice(2);
let warnOnly = false;
let update = false;
let raiseOk = false;
for (const a of args) {
  if (a === "--warn-only") warnOnly = true;
  else if (a === "--update-exceptions") update = true;
  else if (a === "--allow-raise") raiseOk = true;
  else {
    console.error(`unknown option: ${a}`);
    process.exit(2);
  }
}

const files = gitLsFiles(root);
const dirs = new Set();
for (const f of files) {
  const parts = f.split("/").slice(0, -1);
  for (let i = 0; i < parts.length; i++) {
    dirs.add(parts.slice(0, i + 1).join("/"));
  }
}

const byFolder = new Map();
for (const f of files) {
  if (SOURCE.test(f) && !SKIPPED.test(f) && !TEST.test(f)) {
    const dir = dirname(f);
    const key = dir === "" ? "." : dir;
    const list = byFolder.get(key) ?? [];
    list.push(f);
    byFolder.set(key, list);
  }
}

const found = new Map(); // key -> { count, hint }
for (const [folder, paths] of byFolder) {
  if (paths.length > WARN) {
    found.set(`${folder}/`, { count: paths.length, hint: "group into capability subfolders" });
  }
  const stems = [];
  for (const p of paths) {
    const name = basename(p);
    if (name.endsWith(".rs")) {
      if (PROGRAMS.test(folder)) continue;
      stems.push({ lang: "rs", stem: name.slice(0, -3), seps: "_" });
    } else if (/\.tsx?$/.test(name)) {
      stems.push({ lang: "ts", stem: name.replace(/\.tsx?$/, ""), seps: "_-" });
    }
  }
  const members = new Map(); // `${lang}|${prefix}` -> count
  for (const { lang, stem, seps } of stems) {
    let at = -1;
    for (let i = 0; i < stem.length; i++) {
      if (seps.includes(stem[i])) {
        at = i;
        break;
      }
    }
    if (at > 0 && at < stem.length - 1) {
      const prefix = stem.slice(0, at);
      const mk = `${lang}|${prefix}`;
      members.set(mk, (members.get(mk) ?? 0) + 1);
    }
  }
  for (const [mk, n] of members) {
    const [lang, prefix] = mk.split("|");
    const base = folder === "." ? prefix : `${folder}/${prefix}`;
    const anchored = dirs.has(base) || stems.some((s) => s.lang === lang && s.stem === prefix);
    if (n >= (anchored ? 2 : 3)) {
      const sep = lang === "rs" ? "_" : "-";
      found.set(`${base}${sep}*`, { count: n, hint: `move into ${prefix}/` });
    }
  }
}

function loadExceptions() {
  const map = new Map();
  if (!existsSync(EXC_PATH)) return map;
  for (const raw of readFileSync(EXC_PATH, "utf8").split(/\r?\n/)) {
    const clean = raw.split("#")[0].trim();
    if (!clean) continue;
    const parts = clean.split(/\s+/);
    const key = parts[0].endsWith("*") || parts[0].endsWith("/") ? parts[0] : `${parts[0]}/`;
    const limit = parts.length > 1 ? Number.parseInt(parts[1], 10) : 1e9;
    map.set(key, limit);
  }
  return map;
}

const exc = loadExceptions();
if (update) {
  const next = new Map();
  for (const [k, { count: n }] of found) {
    if (!(k.endsWith("*") || n > HARD)) continue;
    if (!exc.has(k)) {
      if (raiseOk) next.set(k, n);
      else console.error(`not listing new violation ${k} (${n}); use --allow-raise`);
    } else {
      const old = exc.get(k);
      next.set(k, n <= old || raiseOk ? n : old);
    }
  }
  const lines = [
    "# <key> <ceiling>: existing folder-shape violations (see scripts/folder-shape.sh). Ceilings only go down.",
    "# Refresh with scripts/folder-shape.sh --update-exceptions.",
    ...[...next.keys()].sort().map((k) => `${k} ${next.get(k)}`),
    "",
  ];
  writeFileSync(EXC_PATH, lines.join("\n"), "utf8");
  console.log(`wrote ${EXC_REL} (${next.size} entries)`);
  process.exit(0);
}

let errors = 0;
let warns = 0;
const gh = process.env.GITHUB_ACTIONS === "true";
const rows = [];
const sorted = [...found.entries()].sort((a, b) => b[1].count - a[1].count || a[0].localeCompare(b[0]));

for (const [k, { count: n, hint }] of sorted) {
  const fam = k.endsWith("*");
  if (fam || n > HARD) {
    const limit = exc.get(k);
    if (limit !== undefined && n <= limit) {
      warns++;
      rows.push(`${String(n).padStart(6)}  warn   ${k} (${hint}; exception, ceiling ${limit})`);
    } else {
      errors++;
      const limStr = limit !== undefined && limit < 1e9 ? `; limit ${limit}` : "";
      rows.push(`${String(n).padStart(6)}  ERROR  ${k} (${hint}${limStr})`);
      if (gh) console.log(`::error file=${k}::${n} files: ${hint}`);
    }
  } else {
    warns++;
    rows.push(`${String(n).padStart(6)}  warn   ${k} (${hint}, soft limit ${WARN})`);
  }
}

if (rows.length > 0) console.log(rows.join("\n"));
const families = [...found.keys()].filter((k) => k.endsWith("*")).length;
console.log(
  `folder shape: families: ${families}, folders over ${WARN}: ${found.size - families}, errors: ${errors}, warnings: ${warns}`,
);
if (!warnOnly && errors > 0) process.exit(1);
