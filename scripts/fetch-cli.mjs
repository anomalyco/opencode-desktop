#!/usr/bin/env node
// Downloads the official @opencode/cli-<os>-<cpu> binary and writes opencode-cli[.exe] plus
// opencode-cli.version into --dest, matching packages/desktop/script/utils.ts (CLI_BINARIES,
// downloadCliToResources, copyCliToResources).
//
// Usage:
//   node scripts/fetch-cli.mjs --target <target> --dest <dir> [--channel <ch>] [--version <ver>]
//
// Plain ESM for Bun or Node >= 18 with zero external packages.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

// Exact target-to-npm-package table from packages/desktop/script/utils.ts:17-54.
const CLI_BINARIES = {
  "darwin-arm64": { pkg: "@opencode/cli-darwin-arm64", os: "darwin", bin: "opencode", lipo: "arm64" },
  "darwin-x64": { pkg: "@opencode/cli-darwin-x64-baseline", os: "darwin", bin: "opencode", lipo: "x86_64" },
  "windows-x64": { pkg: "@opencode/cli-windows-x64-baseline", os: "win32", bin: "opencode.exe" },
  "windows-arm64": { pkg: "@opencode/cli-windows-arm64", os: "win32", bin: "opencode.exe" },
  "linux-x64": { pkg: "@opencode/cli-linux-x64-baseline", os: "linux", bin: "opencode" },
  "linux-arm64": { pkg: "@opencode/cli-linux-arm64", os: "linux", bin: "opencode" },
};

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    if (k.startsWith("--") && i + 1 < argv.length) {
      args[k.slice(2)] = argv[++i];
    }
  }
  return args;
}

function tagForChannel(channel) {
  switch ((channel || "").toLowerCase()) {
    case "stable":
    case "prod":
    case "latest":
      return "latest";
    case "beta":
      return "beta";
    default:
      return "dev";
  }
}

async function fetchRetry(url, attempts = 4) {
  let lastErr;
  for (let i = 1; i <= attempts; i++) {
    try {
      const res = await fetch(url, { redirect: "follow" });
      if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
      return res;
    } catch (err) {
      lastErr = err;
      if (i < attempts) {
        await new Promise((r) => setTimeout(r, 1000 * i));
      }
    }
  }
  throw lastErr;
}

function tarBinary() {
  if (process.platform === "win32") {
    const sysRoot = process.env.SystemRoot || "C:\\Windows";
    return join(sysRoot, "System32", "tar.exe");
  }
  return "tar";
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const target = args.target;
  const destDir = args.dest;
  if (!target || !destDir) {
    console.error("Usage: fetch-cli.mjs --target <target> --dest <dir> [--channel <ch>] [--version <ver>]");
    process.exit(2);
  }
  const spec = CLI_BINARIES[target];
  if (!spec) {
    console.error(`Unsupported CLI target '${target}'. Expected one of: ${Object.keys(CLI_BINARIES).join(", ")}`);
    process.exit(2);
  }

  const versionSpec =
    args.version ||
    process.env.OPENCODE_CLI_VERSION ||
    tagForChannel(args.channel || process.env.OPENCODE_RUNE_BUILD_CHANNEL);

  const metaUrl = `https://registry.npmjs.org/${spec.pkg}/${encodeURIComponent(versionSpec)}`;
  console.error(`fetch-cli: resolving ${spec.pkg}@${versionSpec}`);
  const metaRes = await fetchRetry(metaUrl);
  const meta = await metaRes.json();
  const version = typeof meta.version === "string" ? meta.version.trim() : "";
  const tarballUrl = meta.dist?.tarball;
  const integrity = meta.dist?.integrity;
  if (!version || !tarballUrl) {
    throw new Error(`Invalid npm metadata for ${spec.pkg}@${versionSpec}`);
  }

  const work = mkdtempSync(join(tmpdir(), "opencode-cli-"));
  try {
    const tgzPath = join(work, "cli.tgz");
    console.error(`fetch-cli: downloading ${spec.pkg}@${version}`);
    const tgzRes = await fetchRetry(tarballUrl);
    if (!tgzRes.body) throw new Error("Empty tarball response");
    const hasher = integrity?.startsWith("sha512-") ? createHash("sha512") : null;
    const nodeStream = Readable.fromWeb(tgzRes.body);
    if (hasher) {
      nodeStream.on("data", (chunk) => hasher.update(chunk));
    }
    await pipeline(nodeStream, createWriteStream(tgzPath));

    if (hasher) {
      const actual = "sha512-" + hasher.digest("base64");
      if (actual !== integrity) {
        throw new Error(`SHA-512 integrity mismatch for ${spec.pkg}@${version}: got ${actual}, expected ${integrity}`);
      }
    }

    execFileSync(tarBinary(), ["-xzf", tgzPath, "-C", work], { stdio: "inherit" });
    const extracted = join(work, "package", "bin", spec.bin);
    if (!existsSync(extracted)) {
      throw new Error(`Missing expected binary ${extracted} in ${spec.pkg}@${version}`);
    }

    mkdirSync(destDir, { recursive: true });
    const destName = spec.os === "win32" ? "opencode-cli.exe" : "opencode-cli";
    const destBin = join(destDir, destName);
    const destVer = join(destDir, "opencode-cli.version");
    copyFileSync(extracted, destBin);
    if (spec.os !== "win32" && process.platform !== "win32") {
      chmodSync(destBin, 0o755);
    }
    const size = statSync(destBin).size;
    if (size === 0) {
      throw new Error(`Bundled CLI is empty: ${destBin}`);
    }
    if (spec.os === "darwin" && process.platform === "darwin") {
      const actualArch = execFileSync("lipo", ["-archs", destBin], { encoding: "utf8" }).trim();
      if (actualArch !== spec.lipo) {
        throw new Error(`Bundled CLI arch '${actualArch}', expected '${spec.lipo}'`);
      }
      execFileSync("codesign", ["--force", "--sign", "-", destBin], { stdio: "inherit" });
    }
    writeFileSync(destVer, `${version}\n`, "utf8");
    console.error(`fetch-cli: staged ${destBin} (${version}, ${size} bytes)`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(`fetch-cli: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
