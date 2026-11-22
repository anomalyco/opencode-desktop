// Every prop a built-in plugin passes to a `ui.<node>(...)` call must be one the contract gives that
// node (crates/opencode-plugin-spec/contract.json: the node's props type, with what it extends). A
// prop the host doesn't know is ignored at run time, so a typo or a wished-for prop (`autofocus` on
// a button, #664) silently does nothing.
// Checks the props table literals of `ui.<node>(` calls in tracked `plugins/**/*.luau`: the first
// argument, and the second when it has keys (the `(style, props, children)` form). Known misses are
// listed, one `path node prop` per line, in scripts/plugin-props-exceptions.txt: a ratchet; a listed
// miss that is gone must be removed from the list.
// Runs on Bun or Node >= 18. Usage: node scripts/guardrails/plugin-props.mjs [--list]
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const contract = JSON.parse(readFileSync(join(root, "crates/opencode-plugin-spec/contract.json"), "utf8"));
const types = new Map(contract.types.map((t) => [t.name, t]));
const fieldsOf = (name, seen = new Set()) => {
  const t = types.get(name);
  if (!t || seen.has(name)) return [];
  seen.add(name);
  return [...(t.fields ?? []).map((f) => f.name), ...(t.extends ?? []).flatMap((b) => fieldsOf(b, seen))];
};
const allowed = new Map(contract.nodes.map((n) => [n.name, new Set(fieldsOf(n.props))]));

// Luau source with comments and string contents blanked (same length), so braces and keys are found
// only in code.
function blank(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "-" && src[i + 1] === "-") {
      const long = /^--\[(=*)\[/.exec(src.slice(i, i + 12));
      const end = long ? src.indexOf(`]${long[1]}]`, i + 4) : src.indexOf("\n", i);
      const stop = end < 0 ? src.length : long ? end + long[1].length + 2 : end;
      out += src.slice(i, stop).replace(/[^\n]/g, " ");
      i = stop;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < src.length && src[j] !== c && src[j] !== "\n") j += src[j] === "\\" ? 2 : 1;
      out += c + " ".repeat(Math.max(0, j - i - 1)) + (src[j] === c ? c : "");
      i = src[j] === c ? j + 1 : j;
      continue;
    }
    const long = c === "[" && /^\[(=*)\[/.exec(src.slice(i, i + 12));
    if (long) {
      const end = src.indexOf(`]${long[1]}]`, i + 2);
      const stop = end < 0 ? src.length : end + long[1].length + 2;
      out += src.slice(i, stop).replace(/[^\n]/g, " ");
      i = stop;
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

// The index just past the bracket that closes the one at `open`.
function close(code, open) {
  let depth = 0;
  for (let i = open; i < code.length; i++) {
    const c = code[i];
    if (c === "{" || c === "(" || c === "[") depth++;
    else if (c === "}" || c === ")" || c === "]") {
      depth--;
      if (depth === 0) return i + 1;
    }
  }
  return code.length;
}

// The index just past the `end` that closes the `function` at `at`: counts `function`, `do`,
// `repeat` and statement `if` (+1) against `end` and `until` (-1). An `if` right after `=`, `(`,
// `,`, `{`, `[`, `return`, an operator or `and`/`or`/`not` is an if-expression: it has no `end`.
function skipFunction(code, at) {
  let depth = 0;
  const word = /[A-Za-z_][A-Za-z0-9_]*/g;
  word.lastIndex = at;
  let m;
  while ((m = word.exec(code)) !== null) {
    const w = m[0];
    if (/[A-Za-z0-9_.:]/.test(code[m.index - 1] ?? "")) continue;
    if (w === "function" || w === "do" || w === "repeat") depth++;
    else if (w === "if") {
      const before = code.slice(Math.max(0, m.index - 12), m.index).replace(/\s+$/, "");
      if (!/(?:[=(,{[+\-*/%^<>~#]|\.\.|\breturn|\band|\bor|\bnot)$/.test(before)) depth++;
    } else if (w === "end" || w === "until") {
      depth--;
      if (depth === 0) return m.index + w.length;
    }
  }
  return code.length;
}

// The top-level arguments of the call whose `(` is at `open`: [start, end) of each.
function args(code, open) {
  const out = [];
  let depth = 0;
  let start = open + 1;
  for (let i = open; i < code.length; i++) {
    const c = code[i];
    if (c === "f" && code.startsWith("function", i) && !/[A-Za-z0-9_.:]/.test(code[i - 1] ?? "") && !/[A-Za-z0-9_]/.test(code[i + 8] ?? "")) {
      i = skipFunction(code, i) - 1;
      continue;
    }
    if (c === "{" || c === "(" || c === "[") depth++;
    else if (c === "}" || c === ")" || c === "]") {
      depth--;
      if (depth === 0) {
        out.push([start, i]);
        return out;
      }
    } else if (c === "," && depth === 1) {
      out.push([start, i]);
      start = i + 1;
    }
  }
  return out;
}

// The keys at the top level of the table literal spanning [open, end): `name =` (not `==`).
// A `function ... end` value is skipped whole, so its own `local a, b = ...` reads as no key.
function keys(code, open, end) {
  const out = [];
  let depth = 0;
  for (let i = open; i < end; i++) {
    const c = code[i];
    if (c === "f" && code.startsWith("function", i) && !/[A-Za-z0-9_.:]/.test(code[i - 1] ?? "") && !/[A-Za-z0-9_]/.test(code[i + 8] ?? "")) {
      i = skipFunction(code, i) - 1;
      continue;
    }
    if (c === "{" || c === "(" || c === "[") depth++;
    else if (c === "}" || c === ")" || c === "]") depth--;
    else if (depth === 1 && /[A-Za-z_]/.test(c) && !/[A-Za-z0-9_.:]/.test(code[i - 1] ?? "")) {
      const m = /^([A-Za-z_][A-Za-z0-9_]*)\s*=(?!=)/.exec(code.slice(i, i + 80));
      const before = code.slice(open + 1, i).replace(/\s+$/, "");
      if (m && (before === "" || before.endsWith(",") || before.endsWith(";") || before.endsWith("{"))) out.push({ key: m[1], at: i });
      if (m) i += m[1].length - 1;
    }
  }
  return out;
}

const misses = [];
let calls = 0;
let tables = 0;
for (const file of gitLsFiles(root, "plugins/**/*.luau")) {
  const src = readFileSync(join(root, file), "utf8");
  const code = blank(src);
  const line = (at) => code.slice(0, at).split("\n").length;
  for (const m of code.matchAll(/\bui\.([A-Za-z_][A-Za-z0-9_]*)\s*\(/g)) {
    const node = m[1];
    const known = allowed.get(node);
    if (!known) continue;
    // Every argument that is a table literal with keys: props, a style, or props over a style
    // (`ui.text(s, STYLE, { ... })`); a children array has no keys.
    for (const [a, b] of args(code, m.index + m[0].length - 1).slice(0, 3)) {
      let i = a;
      while (/\s/.test(code[i] ?? "")) i++;
      if (code[i] !== "{") continue;
      const found = keys(code, i, close(code, i));
      if (found.length) tables++;
      for (const k of found) if (!known.has(k.key)) misses.push({ file, line: line(k.at), node, prop: k.key });
      if (b < i) break;
    }
    calls++;
  }
}

const exceptionsPath = "scripts/plugin-props-exceptions.txt";
let listed = [];
try {
  listed = readFileSync(join(root, exceptionsPath), "utf8").split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith("#"));
} catch {}
const id = (x) => `${x.file} ${x.node} ${x.prop}`;
const found = new Set(misses.map(id));
const errors = [
  ...misses.filter((x) => !listed.includes(id(x))).map((x) => `${x.file}:${x.line}: ui.${x.node} has no prop \`${x.prop}\` in the contract: the host ignores it`),
  ...listed.filter((l) => !found.has(l)).map((l) => `${exceptionsPath}: "${l}" no longer occurs: remove it`),
];
if (process.argv.includes("--list")) for (const x of misses) console.log(`${x.file}:${x.line} ui.${x.node} ${x.prop}`);
for (const e of errors) console.error(`  ERROR  ${e}`);
console.log(`plugin props: ${calls} ui calls, ${tables} props tables checked; ${misses.length} unknown prop use(s), ${listed.length} listed, ${errors.length} error(s)`);
process.exit(errors.length ? 1 : 0);
