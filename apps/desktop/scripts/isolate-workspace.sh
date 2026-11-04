#!/usr/bin/env bash
# Strips workspace crates that depend on private anomalyco/wraith from an ephemeral CI checkout,
# lets Cargo prune unreachable feature edges from Cargo.lock, and verifies every remaining package
# (name, version, source, checksum) is an exact subset of the committed Cargo.lock before
# `cargo build --release --locked -p opencode-desktop` runs. Never commit the modified files.
set -euo pipefail
ROOT=${1:-$(cd "$(dirname "$0")/../../.." && pwd)}
ORIG_LOCK=$(mktemp "${TMPDIR:-/tmp}/opencode-rune-orig-lock.XXXXXX")
trap 'rm -f "$ORIG_LOCK"' EXIT
cp "$ROOT/Cargo.lock" "$ORIG_LOCK"

python3 - "$ROOT/Cargo.toml" <<'PY'
import pathlib, re, sys

manifest_path = pathlib.Path(sys.argv[1])
keep_members = [
    '"apps/desktop"',
    '"crates/browser-pane"',
    '"crates/hex-client"',
    '"crates/opencode-kit"',
    '"crates/otlp-lite"',
    '"crates/project-icon"',
    '"crates/rune-parts"',
    '"crates/terminal"',
]
src = manifest_path.read_text(encoding="utf-8")
src = re.sub(
    r"^members\s*=\s*\[[^\]]*\]",
    "members = [" + ", ".join(keep_members) + "]",
    src,
    count=1,
    flags=re.MULTILINE,
)
out_lines = [
    line
    for line in src.splitlines(True)
    if not line.startswith(("wraith-core =", "wraith-local =", "wraith-do =", "seer-domain ="))
]
manifest_path.write_text("".join(out_lines), encoding="utf-8")
PY

cargo metadata --format-version 1 --manifest-path "$ROOT/Cargo.toml" > /dev/null

python3 - "$ORIG_LOCK" "$ROOT/Cargo.lock" <<'PY'
import pathlib, sys, tomllib


def pkg_tuples(path: str) -> set[tuple[str, str, str, str]]:
    data = tomllib.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    return {
        (
            pkg["name"],
            pkg["version"],
            pkg.get("source", ""),
            pkg.get("checksum", ""),
        )
        for pkg in data.get("package", [])
    }


orig = pkg_tuples(sys.argv[1])
pruned = pkg_tuples(sys.argv[2])
unexpected = sorted(pruned - orig)
if unexpected:
    for name, ver, src, csum in unexpected:
        print(f"isolate-workspace: unexpected lockfile drift: {name} {ver} ({src}) {csum}", file=sys.stderr)
    sys.exit(1)
PY
