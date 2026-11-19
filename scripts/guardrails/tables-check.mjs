// Checks hand-maintained tables and generated bindings for drift:
//   1. apps/desktop/src/stories/order.rs <-> STORY_LIST story registrations
//   2. apps/desktop/src/shell/commands/order.rs <-> built-in CommandEntry registrations
//   3. crates/opencode-i18n/locales/rune/*.json coverage + unknown keys vs rune/en.json
//   4. crates/opencode-plugin-spec generated files <-> contract.json (`--check`)
// Runs on Bun or Node >= 18. Usage: scripts/tables-check.sh
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  extractBalanced,
  gitLsFiles,
  splitTopLevel,
  stripRust,
} from "./rust-scan.mjs";

const root = join(fileURLToPath(new URL("../..", import.meta.url)));
const errors = [];
const summaries = [];

function readText(relPath) {
  return readFileSync(join(root, relPath), "utf8");
}

function unescapeRustString(raw) {
  return raw
    .replace(/\\n/g, "\n")
    .replace(/\\r/g, "\r")
    .replace(/\\t/g, "\t")
    .replace(/\\"/g, '"')
    .replace(/\\\\/g, "\\");
}

const desktopRs = gitLsFiles(root, "apps/desktop/src/*.rs");

// ── 1. Stories: apps/desktop/src/stories/order.rs <-> STORY_LIST ──────────────

{
  const orderPath = "apps/desktop/src/stories/order.rs";
  const orderClean = stripRust(readText(orderPath));
  const arrM = orderClean.match(/\bORDER\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\]\s*;/);
  if (!arrM) {
    errors.push(`${orderPath}: failed to parse \`ORDER: &[&str]\``);
  } else {
    const ordered = [...arrM[1].matchAll(/"((?:\\.|[^"\\])*)"/g)].map((m) => unescapeRustString(m[1]));
    const registered = [];

    for (const f of desktopRs) {
      const raw = readText(f);
      if (!raw.includes("STORY_LIST")) continue;
      const clean = stripRust(raw);
      const regRe = /\bregister!\s*\([^;]*?\bSTORY_LIST\b[^=]*=\s*\[/g;
      let m;
      while ((m = regRe.exec(clean)) !== null) {
        const openIdx = m.index + m[0].length - 1;
        const bal = extractBalanced(clean, openIdx, "[", "]");
        if (!bal) {
          errors.push(`${f}: unbalanced brackets in STORY_LIST register! block`);
          continue;
        }
        const body = bal.content;
        const found = [];
        for (const sm of body.matchAll(/Story::(?:session|composer)\s*\(\s*"((?:\\.|[^"\\])*)"/g)) {
          found.push(unescapeRustString(sm[1]));
        }
        for (const pm of body.matchAll(/\bpath\s*:\s*"((?:\\.|[^"\\])*)"/g)) {
          found.push(unescapeRustString(pm[1]));
        }

        // Resolve indexed helper registrations such as `[story(0), story(1), ...]` backed by `const PATHS: [&str; N]`.
        if (found.length === 0) {
          const calls = [...body.matchAll(/\b[a-z_][a-z0-9_]*\s*\(\s*(\d+)\s*\)/g)];
          const pathsArr = clean.match(/\bconst\s+PATHS\s*:\s*\[&str;\s*\d+\]\s*=\s*\[([\s\S]*?)\]\s*;/);
          if (calls.length > 0 && pathsArr) {
            const ps = [...pathsArr[1].matchAll(/"((?:\\.|[^"\\])*)"/g)].map((x) => unescapeRustString(x[1]));
            for (const c of calls) {
              const idx = Number.parseInt(c[1], 10);
              if (idx < ps.length) found.push(ps[idx]);
              else errors.push(`${f}: STORY_LIST helper index ${idx} out of bounds for PATHS (len ${ps.length})`);
            }
          } else {
            errors.push(`${f}: could not extract story paths from STORY_LIST register! block`);
          }
        }

        for (const p of found) registered.push({ path: p, file: f });
      }
    }

    // Duplicate entries in order.rs
    const seenOrder = new Set();
    for (const p of ordered) {
      if (seenOrder.has(p)) errors.push(`${orderPath}: duplicate story "${p}"`);
      seenOrder.add(p);
    }

    // Duplicate registrations across files
    const seenReg = new Map();
    for (const { path, file } of registered) {
      if (seenReg.has(path)) {
        errors.push(`${file}: duplicate story registration "${path}" (also in ${seenReg.get(path)})`);
      } else {
        seenReg.set(path, file);
      }
    }

    // Stale entries in order.rs
    for (const p of ordered) {
      if (!seenReg.has(p)) {
        errors.push(`${orderPath}: story "${p}" is listed in ORDER but not registered in STORY_LIST`);
      }
    }

    // Registered stories missing from order.rs
    for (const { path, file } of registered) {
      if (!seenOrder.has(path)) {
        errors.push(`${file}: story "${path}" is registered in STORY_LIST but missing from ${orderPath}`);
      }
    }

    summaries.push(`stories: ${registered.length} registered, ${ordered.length} ordered`);
  }
}

// ── 2. Commands: apps/desktop/src/shell/commands/order.rs <-> CommandEntry ────

{
  const cmdOrderPath = "apps/desktop/src/shell/commands/order.rs";
  const orderClean = stripRust(readText(cmdOrderPath));
  const consts = new Map(); // NAME -> value
  const valToNames = new Map();

  for (const m of orderClean.matchAll(/\bpub\s+const\s+([A-Z0-9_]+)\s*:\s*u16\s*=\s*(\d+)\s*;/g)) {
    const name = m[1];
    const val = Number.parseInt(m[2], 10);
    if (consts.has(name)) {
      errors.push(`${cmdOrderPath}: duplicate order constant ${name}`);
    }
    consts.set(name, val);
    const list = valToNames.get(val) ?? [];
    list.push(name);
    valToNames.set(val, list);
  }

  for (const [val, names] of valToNames) {
    if (names.length > 1) {
      errors.push(`${cmdOrderPath}: duplicate order value ${val} shared by ${names.join(", ")}`);
    }
  }

  const usedConsts = new Map(); // constName -> [source]
  const cmdIds = new Map(); // idExpr -> [source]
  let registeredCmds = 0;

  for (const f of desktopRs) {
    const raw = readText(f);
    if (!raw.includes("CommandEntry")) continue;
    const clean = stripRust(raw);
    const re = /\bCommandEntry(?:::<[^>]+>)?::(from_key|new)\s*\(/g;
    let m;
    while ((m = re.exec(clean)) !== null) {
      const openIdx = m.index + m[0].length - 1;
      const bal = extractBalanced(clean, openIdx, "(", ")");
      if (!bal) {
        errors.push(`${f}: unbalanced parens in CommandEntry::${m[1]}(...)`);
        continue;
      }
      const args = splitTopLevel(bal.content);
      const idArg = args[0] ?? "";
      const orderArg = args[2] ?? "";
      registeredCmds++;

      const idList = cmdIds.get(idArg) ?? [];
      idList.push(f);
      cmdIds.set(idArg, idList);

      const ordM = orderArg.match(/^(?:(?:crate::(?:shell::)?)?commands::)?order::([A-Z0-9_]+)$/);
      if (!ordM) {
        errors.push(`${f}: command ${idArg} must pass a \`commands::order::*\` constant (found \`${orderArg}\`)`);
      } else {
        const constName = ordM[1];
        if (!consts.has(constName)) {
          errors.push(`${f}: command ${idArg} references unknown order constant \`order::${constName}\``);
        }
        const uses = usedConsts.get(constName) ?? [];
        uses.push(`${f} (${idArg})`);
        usedConsts.set(constName, uses);
      }
    }
  }

  for (const [idArg, files] of cmdIds) {
    if (files.length > 1) {
      errors.push(`duplicate built-in command id ${idArg} registered in: ${files.join(", ")}`);
    }
  }
  for (const [constName, uses] of usedConsts) {
    if (uses.length > 1) {
      errors.push(`${cmdOrderPath}: order::${constName} is shared by multiple commands: ${uses.join(", ")}`);
    }
  }
  for (const name of consts.keys()) {
    if (!usedConsts.has(name)) {
      errors.push(`${cmdOrderPath}: order::${name} is defined but unused by any registered command`);
    }
  }

  summaries.push(`commands: ${registeredCmds} registered, ${consts.size} unique order slots`);
}

// ── 3. i18n: crates/opencode-i18n/locales/rune/*.json coverage + extra keys ───

{
  const runeDirRel = "crates/opencode-i18n/locales/rune";
  const runeDir = join(root, runeDirRel);
  const enPath = join(runeDir, "en.json");
  if (!existsSync(enPath)) {
    errors.push(`${runeDirRel}/en.json: missing source locale`);
  } else {
    let enObj;
    try {
      enObj = JSON.parse(readFileSync(enPath, "utf8"));
    } catch (err) {
      errors.push(`${runeDirRel}/en.json: invalid JSON (${err.message})`);
      enObj = null;
    }

    if (enObj && typeof enObj === "object" && !Array.isArray(enObj)) {
      const enKeys = Object.keys(enObj);
      const enSet = new Set(enKeys);
      const PLURAL_SUFFIX = /\.(zero|one|two|few|many|other)$/;
      const enPluralStems = new Set(
        enKeys.filter((k) => PLURAL_SUFFIX.test(k)).map((k) => k.replace(PLURAL_SUFFIX, "")),
      );

      const files = readdirSync(runeDir)
        .filter((f) => f.endsWith(".json") && f !== "en.json")
        .sort();

      const coverages = [];
      let minCov = { pct: 100, count: enKeys.length, file: "" };
      let maxCov = { pct: 0, count: 0, file: "" };

      for (const lf of files) {
        const rel = `${runeDirRel}/${lf}`;
        let obj;
        try {
          obj = JSON.parse(readFileSync(join(runeDir, lf), "utf8"));
        } catch (err) {
          errors.push(`${rel}: invalid JSON (${err.message})`);
          continue;
        }
        if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
          errors.push(`${rel}: expected a JSON object`);
          continue;
        }

        let present = 0;
        for (const [k, v] of Object.entries(obj)) {
          if (typeof v !== "string") {
            errors.push(`${rel}: key "${k}" must have a string value`);
          }
          if (enSet.has(k)) {
            present++;
          } else {
            const pm = k.match(PLURAL_SUFFIX);
            const validPluralSibling = pm !== null && enPluralStems.has(k.slice(0, -pm[0].length));
            if (!validPluralSibling) {
              errors.push(`${rel}: unknown key "${k}" not present in ${runeDirRel}/en.json`);
            }
          }
        }

        const pct = enKeys.length > 0 ? Math.round((present / enKeys.length) * 100) : 100;
        coverages.push(pct);
        if (present < minCov.count || (present === minCov.count && !minCov.file)) {
          minCov = { pct, count: present, file: lf };
        }
        if (present > maxCov.count) {
          maxCov = { pct, count: present, file: lf };
        }
      }

      coverages.sort((a, b) => a - b);
      const median = coverages.length ? coverages[Math.floor(coverages.length / 2)] : 100;
      summaries.push(
        `rune i18n: ${files.length} locales vs en.json (${enKeys.length} keys), ` +
          `coverage ${minCov.pct}%-${maxCov.pct}% (median ${median}%, lowest ${minCov.file} ${minCov.count}/${enKeys.length})`,
      );
    }
  }
}

// ── 4. Plugin contract bindings: opencode-plugin-spec -- --check ──────────────

{
  const res = spawnSync("cargo", ["run", "-q", "-p", "opencode-plugin-spec", "--", "--check"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (res.error) {
    errors.push(`opencode-plugin-spec --check: failed to run cargo (${res.error.message})`);
  } else if (res.status !== 0) {
    const detail = (res.stderr || res.stdout || "").trim().replace(/\r?\n/g, " | ");
    errors.push(`opencode-plugin-spec --check failed (exit ${res.status}): ${detail}`);
  } else {
    const out = (res.stdout || "").trim();
    summaries.push(`plugin spec: ${out || "ok"}`);
  }
}

// ── Report ────────────────────────────────────────────────────────────────────

for (const s of summaries) console.log(`  INFO   ${s}`);
if (errors.length > 0) {
  for (const e of errors) console.error(`  ERROR  ${e}`);
  console.error(`tables check: ${errors.length} error(s)`);
  process.exit(1);
}

console.log("tables check: ok");
