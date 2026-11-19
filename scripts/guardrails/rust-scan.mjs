// Shared low-level helpers for guardrail scripts (plain ESM; runs on Bun or Node >= 18).
import { execFileSync } from "node:child_process";

export function gitLsFiles(root, ...patterns) {
  const args = ["ls-files", "-z", ...patterns];
  const out = execFileSync("git", args, {
    cwd: root,
    maxBuffer: 32 * 1024 * 1024,
  }).toString("utf8");
  return out.split("\0").filter(Boolean);
}

export function countLines(text) {
  if (text.length === 0) return 0;
  const parts = text.split(/\r?\n/);
  return parts[parts.length - 1] === "" ? parts.length - 1 : parts.length;
}

// Strips Rust line (`//`) and nested block comments while preserving newlines and
// respecting string literals, raw strings (`r#"..."#`), byte strings, and char literals.
// When `blankStrings` is true, string/char literal bodies are replaced with spaces
// (preserving newlines) so text inside strings cannot trigger syntax regexes.
export function stripRust(src, { blankStrings = false } = {}) {
  const out = [];
  const n = src.length;
  let i = 0;

  while (i < n) {
    const c = src[i];
    const next = src[i + 1];

    // Line comment
    if (c === "/" && next === "/") {
      i += 2;
      while (i < n && src[i] !== "\n") i++;
      continue;
    }

    // Nested block comment
    if (c === "/" && next === "*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (src[i] === "/" && src[i + 1] === "*") {
          depth++;
          i += 2;
        } else if (src[i] === "*" && src[i + 1] === "/") {
          depth--;
          i += 2;
        } else {
          if (src[i] === "\n") out.push("\n");
          i++;
        }
      }
      out.push(" ");
      continue;
    }

    // Raw string literal: r"...", r#"..."#, br"...", cr"..."
    if (
      (c === "r" || ((c === "b" || c === "c") && next === "r")) &&
      (i === 0 || !/[A-Za-z0-9_]/.test(src[i - 1]))
    ) {
      let j = c === "r" ? i + 1 : i + 2;
      let hashes = 0;
      while (j < n && src[j] === "#") {
        hashes++;
        j++;
      }
      if (j < n && src[j] === '"') {
        const close = '"' + "#".repeat(hashes);
        const start = i;
        j++;
        const end = src.indexOf(close, j);
        const stop = end === -1 ? n : end + close.length;
        if (blankStrings) {
          out.push('"');
          for (let k = j; k < (end === -1 ? n : end); k++) {
            out.push(src[k] === "\n" ? "\n" : " ");
          }
          out.push('"');
        } else {
          out.push(src.slice(start, stop));
        }
        i = stop;
        continue;
      }
    }

    // Normal / byte / c-string literal
    if (
      c === '"' ||
      ((c === "b" || c === "c") && next === '"' && (i === 0 || !/[A-Za-z0-9_]/.test(src[i - 1])))
    ) {
      const start = i;
      if (c !== '"') i++;
      i++; // opening quote
      if (blankStrings) out.push('"');
      while (i < n && src[i] !== '"') {
        if (src[i] === "\\") {
          if (blankStrings) {
            if (src[i + 1] === "\n") out.push("\n");
            else out.push("  ");
          }
          i += 2;
        } else {
          if (blankStrings) out.push(src[i] === "\n" ? "\n" : " ");
          i++;
        }
      }
      if (i < n) i++; // closing quote
      if (blankStrings) out.push('"');
      else out.push(src.slice(start, i));
      continue;
    }

    // Char literal (distinguish from lifetime like `'a` or `'static`)
    if (c === "'") {
      if (next === "\\") {
        let j = i + 2;
        while (j < n && src[j] !== "'" && src[j] !== "\n") j++;
        if (j < n && src[j] === "'") {
          out.push(blankStrings ? "' '" : src.slice(i, j + 1));
          i = j + 1;
          continue;
        }
      } else if (i + 2 < n && src[i + 2] === "'" && next !== "\n") {
        out.push(blankStrings ? "' '" : src.slice(i, i + 3));
        i += 3;
        continue;
      }
    }

    out.push(c);
    i++;
  }

  return out.join("");
}

/**
 * Extracts all named brace structs `{ ... }` from comment-stripped Rust source.
 * Returns Map<structName, bodyText>.
 */
export function extractBraceStructs(cleanSrc) {
  const result = new Map();
  const re = /\b(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*(?:where[^{;]*)?\{/g;
  let m;
  while ((m = re.exec(cleanSrc)) !== null) {
    const name = m[1];
    const open = m.index + m[0].length - 1;
    let depth = 1;
    let i = open + 1;
    while (i < cleanSrc.length && depth > 0) {
      if (cleanSrc[i] === "{") depth++;
      else if (cleanSrc[i] === "}") depth--;
      i++;
    }
    if (depth === 0) {
      result.set(name, cleanSrc.slice(open + 1, i - 1));
    }
  }
  return result;
}

/**
 * Parses top-level fields from a comment-stripped Rust struct `{ ... }` body.
 * Handles attributes (`#[...]`) and multiline generic types.
 */
export function parseStructFields(body) {
  const chunks = [];
  let cur = "";
  let dp = 0, db = 0, dk = 0, da = 0;

  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === "(") dp++;
    else if (c === ")") dp--;
    else if (c === "{") db++;
    else if (c === "}") db--;
    else if (c === "[") dk++;
    else if (c === "]") dk--;
    else if (c === "<") da++;
    else if (c === ">" && body[i - 1] !== "-" && body[i - 1] !== "=") da = Math.max(0, da - 1);

    if (c === "," && dp === 0 && db === 0 && dk === 0 && da === 0) {
      chunks.push(cur);
      cur = "";
      continue;
    }
    cur += c;
  }
  if (cur.trim()) chunks.push(cur);

  const fields = [];
  for (let chunk of chunks) {
    // Strip leading `#[...]` attributes (handling balanced `[ ... ]`)
    chunk = chunk.trim();
    while (chunk.startsWith("#[")) {
      let depth = 0;
      let j = 1;
      for (; j < chunk.length; j++) {
        if (chunk[j] === "[") depth++;
        else if (chunk[j] === "]" && --depth === 0) break;
      }
      chunk = chunk.slice(j + 1).trim();
    }
    const fm = chunk.match(/^(?:pub(?:\([^)]*\))?\s+)?(?:r#)?([a-z_][a-z0-9_]*)\s*:\s*([\s\S]+)$/);
    if (fm) {
      fields.push({ name: fm[1], ty: fm[2].replace(/\s+/g, " ").trim() });
    }
  }
  return fields;
}

/**
 * Extracts balanced content inside `(...)`, `[...]`, or `{...}` starting at `openIdx`.
 */
export function extractBalanced(src, openIdx, openChar, closeChar) {
  let depth = 1;
  let i = openIdx + 1;
  while (i < src.length && depth > 0) {
    if (src[i] === '"') {
      i++;
      while (i < src.length && src[i] !== '"') {
        if (src[i] === "\\") i += 2;
        else i++;
      }
      i++;
      continue;
    }
    if (src[i] === openChar) depth++;
    else if (src[i] === closeChar) depth--;
    i++;
  }
  if (depth !== 0) return null;
  return { content: src.slice(openIdx + 1, i - 1), endIdx: i };
}

/**
 * Splits a comma-separated argument list at top-level depth.
 */
export function splitTopLevel(str) {
  const parts = [];
  let cur = "";
  let dp = 0, db = 0, dk = 0;
  for (let i = 0; i < str.length; i++) {
    const c = str[i];
    if (c === '"') {
      cur += c;
      i++;
      while (i < str.length && str[i] !== '"') {
        if (str[i] === "\\") {
          cur += str[i] + (str[i + 1] ?? "");
          i += 2;
        } else {
          cur += str[i];
          i++;
        }
      }
      if (i < str.length) cur += str[i];
      continue;
    }
    if (c === "(") dp++;
    else if (c === ")") dp--;
    else if (c === "{") db++;
    else if (c === "}") db--;
    else if (c === "[") dk++;
    else if (c === "]") dk--;
    if (c === "," && dp === 0 && db === 0 && dk === 0) {
      parts.push(cur.trim());
      cur = "";
      continue;
    }
    cur += c;
  }
  if (cur.trim()) parts.push(cur.trim());
  return parts;
}
