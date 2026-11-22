// Checks that every external open goes through `crates/harness-log` (`harness_log::open_url`,
// `harness_log::reveal`), so a proof run that records external opens never opens the person's browser
// or file manager from a call site that forgot to record. The same rule is in `clippy.toml`
// (`disallowed-methods`); this check runs it in PR CI without compiling.
// Runs on Bun or Node >= 18. Usage: node scripts/guardrails/external-open.mjs
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));

// The wrappers themselves, and the definition of `reveal_in_file_manager`.
const ALLOWED = ["crates/harness-log/", "crates/browser-policy/"];
const RULES = [
  { re: /\.open_url(_same_tab)?\s*\(/, use: "harness_log::open_url(tx, url)" },
  { re: /\breveal_in_file_manager\s*\(/, use: "harness_log::reveal(path)" },
];

const files = gitLsFiles(root, "apps/*.rs", "crates/*.rs", "features/*.rs");
let found = 0;

for (const f of files) {
  if (ALLOWED.some((p) => f.startsWith(p))) continue;
  const full = join(root, f);
  if (!existsSync(full)) continue;
  const lines = readFileSync(full, "utf8").split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*\/\//.test(line)) continue;
    for (const rule of RULES) {
      if (!rule.re.test(line)) continue;
      console.error(`  ERROR  ${f}:${i + 1}  call ${rule.use} instead, so a proof run records it: ${line.trim().replace(/\s+/g, " ")}`);
      found++;
    }
  }
}

console.log(`external open: errors: ${found}`);
if (found > 0) process.exit(1);
