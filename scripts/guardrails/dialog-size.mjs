// Checks that dialogs take one of Desktop's three DialogSize presets (`opencode_ui::dialog::DialogSize`,
// re-exported as `session_view::dialog::DialogSize`) instead of hardcoding dialog-scale dimensions.
// Runs on Bun or Node >= 18. Usage: scripts/dialog-size.sh
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gitLsFiles } from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));

const ROLE_RE = /role\("(alert)?dialog"\)/;
const N = "(3[89][0-9]|[4-9][0-9]{2}|[0-9]{4,})(\\.[0-9]*)?";
const SIZE_RE = new RegExp(
  `\\.(w|h|max_w|max_h)\\(\\s*${N}\\s*\\)` +
    `|const ([A-Z_]+_)?(W|H|WIDTH|HEIGHT): f32 = ${N}\\s*;` +
    `|\\((win_w|win_h)\\s*-\\s*[0-9]+(\\.[0-9]*)?\\)\\s*(\\.min\\(|/)`,
);

const files = gitLsFiles(root, "apps/*.rs", "crates/*.rs", "features/*.rs");
let found = 0;

for (const f of files) {
  if (f === "crates/opencode-ui/src/dialog.rs") continue;
  const full = join(root, f);
  if (!existsSync(full)) continue;
  const text = readFileSync(full, "utf8");
  if (!ROLE_RE.test(text)) continue;

  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*\/\//.test(line)) continue;
    if (line.includes("// dialog-size: ok")) continue;
    if (SIZE_RE.test(line)) {
      const compact = line.trim().replace(/\s+/g, " ");
      console.error(`  ERROR  ${f}:${i + 1}  choose a DialogSize preset instead of sizing the dialog: ${compact}`);
      found++;
    }
  }
}

console.log(`dialog size: errors: ${found}`);
if (found > 0) process.exit(1);
