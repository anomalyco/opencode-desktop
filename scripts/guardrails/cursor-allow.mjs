// The pointer's cursor follows the official desktop app element by element: the arrow on controls and
// drag handles ("the pointer cursor is reserved for links", official's
// packages/desktop/src/renderer/styles.css), and another cursor only where official's source gives
// one. So a cursor that is not the arrow never lands unnoticed, every place the app sets one is listed
// in scripts/cursor-allow.txt with the official line that justifies it:
//   - a `.cursor(...)` call, by the `Cursor::` variants it names (`dynamic` when it names none);
//   - a text link, `.link(...)`: Rune shows the hand over it unless a `.cursor(...)` follows;
//   - selectable text, `.selectable(...)`: Rune shows the I-beam over it.
// One line per file and kind: `path | kind | count | reason`. A call the list doesn't have, a count
// that differs, and a line whose calls are gone all fail: the list is exact, not a ceiling.
// Text fields (the I-beam) and what Rune's kit sets inside its own components are not listed.
// Runs on Bun or Node >= 18. Usage: node scripts/guardrails/cursor-allow.mjs [--list]
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { extractBalanced, gitLsFiles, stripRust } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const LIST = "scripts/cursor-allow.txt";
// The terminal emulator's own `cursor` is the text caret of the grid, not the pointer.
const SKIP = ["crates/terminal/", "crates/ureq-web/", "apps/browser-engine/"];

const found = new Map(); // "path | kind" -> { count, lines }
const add = (file, kind, line) => {
  const key = `${file} | ${kind}`;
  const at = found.get(key) ?? { count: 0, lines: [] };
  at.count++;
  at.lines.push(line);
  found.set(key, at);
};

for (const f of gitLsFiles(root, "apps/*.rs", "crates/*.rs", "features/*.rs")) {
  if (SKIP.some((p) => f.startsWith(p))) continue;
  const full = join(root, f);
  if (!existsSync(full)) continue;
  const raw = readFileSync(full, "utf8");
  if (!/\.(cursor|link|selectable)\s*\(/.test(raw)) continue;
  const src = stripRust(raw, { blankStrings: true });
  const lineOf = (i) => src.slice(0, i).split("\n").length;
  for (const m of src.matchAll(/\.(cursor|link|selectable)\s*\(/g)) {
    const open = m.index + m[0].length - 1;
    if (m[1] !== "cursor") {
      add(f, m[1], lineOf(m.index));
      continue;
    }
    const inner = extractBalanced(src, open, "(", ")");
    const body = inner?.content ?? "";
    // `.cursor()` with no argument reads a cursor (a text caret, a list's cursor): not a pointer shape.
    if (!body.trim()) continue;
    const names = [...new Set([...body.matchAll(/\bCursor::([A-Z][A-Za-z]*)/g)].map((x) => x[1]))];
    add(f, names.length ? names.join("+") : "dynamic", lineOf(m.index));
  }
}

if (process.argv.includes("--list")) {
  for (const key of [...found.keys()].sort()) console.log(`${key} | ${found.get(key).count} | ?`);
  process.exit(0);
}

const listed = new Map();
let errors = 0;
const fail = (msg) => {
  console.error(`  ERROR  ${msg}`);
  errors++;
};
readFileSync(join(root, LIST), "utf8")
  .split(/\r?\n/)
  .forEach((line, i) => {
    if (!line.trim() || line.startsWith("#")) return;
    const parts = line.split(" | ");
    const reason = parts.slice(3).join(" | ").trim();
    const count = Number(parts[2]);
    if (parts.length < 4 || !Number.isInteger(count) || count < 1) return fail(`${LIST}:${i + 1}  expected \`path | kind | count | reason\`: ${line}`);
    if (!reason || reason === "?") return fail(`${LIST}:${i + 1}  no reason: name the official line (or the browser default) that decides this cursor`);
    const key = `${parts[0].trim()} | ${parts[1].trim()}`;
    if (listed.has(key)) return fail(`${LIST}:${i + 1}  listed twice: ${key}`);
    listed.set(key, count);
  });

for (const [key, at] of [...found.entries()].sort()) {
  const want = listed.get(key);
  const [file, kind] = key.split(" | ");
  if (want === undefined) fail(`${file}:${at.lines[0]}  \`${kind}\` is not in ${LIST}: the official app shows the arrow unless its source says otherwise; match it, then add \`${key} | ${at.count} | <official file:line>\``);
  else if (want !== at.count) fail(`${file}  \`${kind}\` occurs ${at.count} times (lines ${at.lines.join(", ")}), ${LIST} says ${want}: check each against the official app and correct the count`);
}
for (const key of listed.keys()) if (!found.has(key)) fail(`${LIST}  a line whose calls are gone: remove \`${key}\``);

console.log(`cursor allow-list: ${found.size} entries, ${[...found.values()].reduce((n, at) => n + at.count, 0)} calls, errors: ${errors}`);
if (errors > 0) process.exit(1);
