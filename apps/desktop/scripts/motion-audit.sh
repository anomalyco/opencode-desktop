#!/usr/bin/env bash
# Motion audit of OpenCode (Rune), against docs/kit-design.md and Kit's motion rules.
#   apps/desktop/scripts/motion-audit.sh [SCENARIO...] [--out DIR] [--no-build]
#   (AUDIT_BIN=path audits that binary, e.g. an older build, with --no-build)
# Builds once, records every scenario in scripts/audit/*.json headless on the fixture (120 fps,
# virtual clock, every element per frame), runs Rune's scripts/motion-audit/analyze.py over the
# traces, and exits non-zero on any finding that scripts/audit/allow.json doesn't list with a
# reason. The analyzer comes from the Rune rev Cargo.toml pins (RUNE=checkout overrides it). On
# Studio, the build and the runs share one HiveMind cargo lease on mini (AUDIT_WORKER pins a worker).
#
# A scenario is a Rune script plus: "scene", or "playground" (a storybook story's path, with
# "variant" by name) to start on one with its motion playing, "secs", "fixture" (keys merged over the fixture),
# "png": [w, h] (a test image, pasted with "png": "@PNG"), "from" (events of another script),
# and expectations on events (see analyze.py): "expect": "instant" | "follow" | "smooth" |
# "leaves", with "element" (a key) and "scope" (an element name part).
set -euo pipefail
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
HERE="$(dirname "$SELF")"
ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -z "${RUNE:-}" ]]; then
  RUNE=$(cd "$ROOT" && cargo metadata --format-version 1 2>/dev/null | python3 -c '
import json, os, sys
m = json.load(sys.stdin)
p = next(p for p in m["packages"] if p["name"] == "rune" and "anomalyco/rune" in (p["source"] or ""))
print(os.path.dirname(os.path.dirname(os.path.dirname(p["manifest_path"]))))')
fi
ANALYZE="$RUNE/scripts/motion-audit/analyze.py"
test -f "$ANALYZE" || { echo "motion-audit: no scripts/motion-audit/analyze.py in $RUNE (set RUNE)" >&2; exit 2; }

if [[ -z "${HIVEMIND_ACTIVE:-}" ]] && command -v hivemind >/dev/null; then
  if [[ -n "${AUDIT_WORKER:-}" ]]; then WHERE=(--worker "$AUDIT_WORKER"); else WHERE=(--prefer mini); fi
  exec hivemind run --kind cargo "${WHERE[@]}" -- "$SELF" "$@"
fi

OUT="$ROOT/target/motion-audit"; BUILD=1; NAMES=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) sed -n '2,15p' "$SELF"; exit 0 ;;
    *) NAMES+=("$1"); shift ;;
  esac
done
cd "$ROOT"
mkdir -p "$OUT"
[[ $BUILD = 1 ]] && cargo build --release -p opencode-desktop --quiet
BIN="${AUDIT_BIN:-${CARGO_TARGET_DIR:-target}/release/opencode-desktop}"

# Resolve each scenario into a script, fixture and frame count under $OUT.
PLAN=$(python3 - "$HERE/audit" "$ROOT/apps/desktop/assets/fixture.json" "$OUT" "${NAMES[@]+"${NAMES[@]}"}" <<'PY'
import glob, json, os, struct, sys, zlib
src, base, out, names = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
def png(w, h, path):
    row = b"\x00" + bytes([90, 140, 200, 255]) * w
    body = zlib.compress(row * h)
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)) + chunk(b"IDAT", body) + chunk(b"IEND", b""))
files = sorted(glob.glob(os.path.join(src, "*.json")))
files = [f for f in files if os.path.basename(f) != "allow.json"]
if names:
    files = [f for f in files if os.path.basename(f)[:-5] in names]
    missing = set(names) - {os.path.basename(f)[:-5] for f in files}
    if missing:
        sys.exit(f"motion-audit: no scenario {', '.join(sorted(missing))} in {src}")
for f in files:
    name = os.path.basename(f)[:-5]
    sc = json.load(open(f))
    events = sc.get("events", [])
    if "from" in sc:
        events = json.load(open(os.path.join(src, sc["from"])))["events"] + events
    fixture = base
    if "fixture" in sc:
        fx = json.load(open(base))
        fx.update(sc["fixture"])
        fixture = os.path.join(out, f"{name}.fixture.json")
        json.dump(fx, open(fixture, "w"))
    if "png" in sc:
        path = os.path.join(out, f"{name}.png")
        png(*sc["png"], path)
        for e in events:
            if e.get("png") == "@PNG":
                e["png"] = path
    script = os.path.join(out, f"{name}.script.json")
    json.dump({"events": events}, open(script, "w"), indent=1)
    mode, target = ("playground", sc["playground"]) if "playground" in sc else ("scene", sc.get("scene", "session"))
    print(name, mode, target, sc.get("variant", "-"), int(round(sc.get("secs", 3.0) * 120)), fixture, script, sep="\t")
PY
)

TRACES=()
while IFS=$'\t' read -r name mode target variant frames fixture script; do
  [[ -z "$name" ]] && continue
  LAUNCH=("--$mode" "$target")
  [[ "$variant" != - ]] && LAUNCH+=(--variant "$variant")
  "$BIN" --fixture "$fixture" "${LAUNCH[@]}" --script "$script" --fps 120 --frames "$frames" \
    --trace-all --trace "$OUT/$name.jsonl" >"$OUT/$name.log" 2>&1 || { echo "motion-audit: $name failed:" >&2; tail -5 "$OUT/$name.log" >&2; exit 2; }
  TRACES+=(--script "$script" "$OUT/$name.jsonl")
done <<<"$PLAN"

python3 "$ANALYZE" "${TRACES[@]}" --allow "$HERE/audit/allow.json" --json "$OUT/report.json" --fail
