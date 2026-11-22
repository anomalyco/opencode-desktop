// Checks that every external open goes through `crates/harness-log` (`harness_log::open_url`,
// `harness_log::reveal`), so a proof run that records external opens never opens the person's browser
// or file manager from a call site that forgot to record. The same rule is in `clippy.toml`
// (`disallowed-methods`); this check runs it in PR CI without compiling.
// Flags, outside the wrappers: a method call `.open_url(` / `.open_url_same_tab(`; any path to them
// (`Tx::open_url(tx, …)`, `rune::Tx::open_url` passed as a value) other than `harness_log::open_url`;
// and any use of `reveal_in_file_manager`. String literals and comments are not code and are skipped.
// Runs on Bun or Node >= 18. Usage: node scripts/guardrails/external-open.mjs
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));

// The wrappers themselves, and the definition of `reveal_in_file_manager`.
const ALLOWED = ["crates/harness-log/", "crates/browser-policy/"];
const RULES = [
  { re: /\.\s*open_url(_same_tab)?\s*\(/, use: "harness_log::open_url(tx, url)" },
  { re: /(?<!\bharness_log)::\s*open_url(_same_tab)?\b/, use: "harness_log::open_url(tx, url)" },
  { re: /\breveal_in_file_manager\b/, use: "harness_log::reveal(path)" },
];

// The line's code only: string and char literals emptied, then a `//` comment cut off.
function code(line) {
  return line
    .replace(/r(#*)"[\s\S]*?"\1/g, '""')
    .replace(/"(\\.|[^"\\])*"/g, '""')
    .replace(/'(\\.|[^'\\])'/g, "''")
    .replace(/\/\/.*$/, "");
}

const files = gitLsFiles(root, "apps/*.rs", "crates/*.rs", "features/*.rs");
let found = 0;

for (const f of files) {
  if (ALLOWED.some((p) => f.startsWith(p))) continue;
  const full = join(root, f);
  if (!existsSync(full)) continue;
  const lines = readFileSync(full, "utf8").split(/\r?\n/);
  let block = false;
  for (let i = 0; i < lines.length; i++) {
    let line = lines[i];
    if (block) {
      const end = line.indexOf("*/");
      if (end < 0) continue;
      line = line.slice(end + 2);
      block = false;
    }
    let c = code(line);
    const open = c.indexOf("/*");
    if (open >= 0) {
      const close = c.indexOf("*/", open + 2);
      if (close < 0) block = true;
      c = c.slice(0, open) + (close < 0 ? "" : c.slice(close + 2));
    }
    const rule = RULES.find((r) => r.re.test(c));
    if (!rule) continue;
    console.error(`  ERROR  ${f}:${i + 1}  call ${rule.use} instead, so a proof run records it: ${line.trim().replace(/\s+/g, " ")}`);
    found++;
  }
}

console.log(`external open: errors: ${found}`);
if (found > 0) process.exit(1);
