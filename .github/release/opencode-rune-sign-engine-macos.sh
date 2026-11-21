#!/usr/bin/env bash
# opencode-rune-sign-engine-macos.sh <unsigned.zip> <out dir> [arch]:
# Verifies an unsigned `OpenCode Browser Engine.app` bundle against an exact
# Mach-O and helper-bundle allowlist, signs it inside-out with Anomaly's
# Developer ID identity (hardened runtime, secure timestamp, engine
# entitlements), notarizes and staples it, and writes:
#   <out dir>/OpenCode-Browser-Engine-<ENGINE_VERSION>-macos-<arch>.zip
#
# Runs inside the `release` environment where only /.github/release/ is checked
# out. Fails hard on any unexpected Mach-O, helper plist mismatch, broken or
# escaping symlink, or missing signing/notary credentials.
. "$(dirname "$0")/lib.sh"

input=${1:?Usage: opencode-rune-sign-engine-macos.sh <unsigned.zip> <out dir> [arch]}
out=${2:?Usage: opencode-rune-sign-engine-macos.sh <unsigned.zip> <out dir> [arch]}
arch=${3:-arm64}

[[ "$arch" = arm64 || "$arch" = x64 ]] || die "Unsupported macOS engine architecture '$arch'."
expected_cpu=$([ "$arch" = arm64 ] && echo arm64 || echo x86_64)
[[ "${ENGINE_VERSION:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  die "ENGINE_VERSION must be X.Y.Z.R, got '${ENGINE_VERSION:-}'."

release=$(cd "$(dirname "$0")" && pwd)
entitlements="$release/opencode-rune-engine.entitlements"
[ -f "$entitlements" ] || die "Missing engine entitlements at $entitlements."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

/usr/bin/ditto -x -k "$input" "$work/unpacked"
app="$work/unpacked/OpenCode Browser Engine.app"
if [ ! -d "$app" ] || [ -L "$app" ] || [ "$(find "$work/unpacked" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" != 1 ]; then
  die "The macOS engine build archive must hold exactly OpenCode Browser Engine.app."
fi

check_plist() {
  local plist=$1 expected_id=$2 expected_exe=$3
  [ -f "$plist" ] && [ ! -L "$plist" ] || die "Missing Info.plist: $plist"
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")" = "$expected_id" ] ||
    die "$plist has unexpected CFBundleIdentifier (expected $expected_id)."
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist")" = "$expected_exe" ] ||
    die "$plist has unexpected CFBundleExecutable (expected $expected_exe)."
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist")" = "$ENGINE_VERSION" ] ||
    die "$plist CFBundleVersion is not $ENGINE_VERSION."
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")" = "$ENGINE_VERSION" ] ||
    die "$plist CFBundleShortVersionString is not $ENGINE_VERSION."
}

check_plist "$app/Contents/Info.plist" "co.anomaly.opencode.browser-engine" "opencode-browser-engine"

# Refuse non-regular/non-dir/non-symlink entries (FIFOs, sockets, device nodes).
special=$(find "$app" ! -type d ! -type f ! -type l)
[ -z "$special" ] || die "Bundle contains unexpected special file types: $special"

# Refuse absolute, broken, or escaping symlinks anywhere inside the bundle.
python3 - "$app" <<'PY' || die "Bundle contains an absolute, broken, or escaping symlink."
import os, sys

root = os.path.realpath(sys.argv[1])
bad = []
for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
    for name in dirnames + filenames:
        path = os.path.join(dirpath, name)
        if os.path.islink(path):
            raw = os.readlink(path)
            if os.path.isabs(raw):
                bad.append(f"{path} -> {raw} (absolute)")
                continue
            target = os.path.realpath(path)
            if not os.path.exists(target):
                bad.append(f"{path} -> {raw} (broken)")
                continue
            if os.path.commonpath([root, target]) != root:
                bad.append(f"{path} -> {raw} (escapes bundle)")
if bad:
    for line in bad:
        print(line, file=sys.stderr)
    sys.exit(1)
PY

# Exact nested .app allowlist (no extra, missing, or renamed helper bundles).
expected_helpers=$(printf '%s\n' \
  'Contents/Frameworks/opencode-browser-engine Helper (Alerts).app' \
  'Contents/Frameworks/opencode-browser-engine Helper (GPU).app' \
  'Contents/Frameworks/opencode-browser-engine Helper (Plugin).app' \
  'Contents/Frameworks/opencode-browser-engine Helper (Renderer).app' \
  'Contents/Frameworks/opencode-browser-engine Helper.app' | LC_ALL=C sort)
found_helpers=$(
  cd "$app" || exit 1
  find . -mindepth 1 -name '*.app' -print | sed 's|^\./||' | LC_ALL=C sort
)
if [ "$found_helpers" != "$expected_helpers" ]; then
  diff_out=$(diff <(printf '%s\n' "$expected_helpers") <(printf '%s\n' "$found_helpers") || true)
  die "Unexpected nested .app bundles in OpenCode Browser Engine.app: $diff_out"
fi

check_helper() {
  local suffix=$1 id_suffix=$2
  local hname="opencode-browser-engine $suffix"
  local happ="$app/Contents/Frameworks/$hname.app"
  [ -d "$happ" ] && [ ! -L "$happ" ] || die "Missing helper bundle: $hname.app"
  check_plist "$happ/Contents/Info.plist" "co.anomaly.opencode.browser-engine.$id_suffix" "$hname"
  [ -f "$happ/Contents/MacOS/$hname" ] && [ ! -L "$happ/Contents/MacOS/$hname" ] ||
    die "Missing helper executable: $hname"
}

check_helper "Helper" "helper"
check_helper "Helper (GPU)" "helper-gpu"
check_helper "Helper (Renderer)" "helper-renderer"
check_helper "Helper (Plugin)" "helper-plugin"
check_helper "Helper (Alerts)" "helper-alerts"

# Exact Mach-O allowlist across the entire bundle (derived from real arm64 and x64 assemblies).
expected_macho=$(printf '%s\n' \
  'Contents/Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework' \
  'Contents/Frameworks/Chromium Embedded Framework.framework/Libraries/libcef_sandbox.dylib' \
  'Contents/Frameworks/opencode-browser-engine Helper (Alerts).app/Contents/MacOS/opencode-browser-engine Helper (Alerts)' \
  'Contents/Frameworks/opencode-browser-engine Helper (GPU).app/Contents/MacOS/opencode-browser-engine Helper (GPU)' \
  'Contents/Frameworks/opencode-browser-engine Helper (Plugin).app/Contents/MacOS/opencode-browser-engine Helper (Plugin)' \
  'Contents/Frameworks/opencode-browser-engine Helper (Renderer).app/Contents/MacOS/opencode-browser-engine Helper (Renderer)' \
  'Contents/Frameworks/opencode-browser-engine Helper.app/Contents/MacOS/opencode-browser-engine Helper' \
  'Contents/MacOS/opencode-browser-engine' | LC_ALL=C sort)
found_macho=$(
  cd "$app" || exit 1
  find . -type f -print0 | while IFS= read -r -d '' f; do
    if file -b "$f" | grep -q 'Mach-O'; then
      printf '%s\n' "${f#./}"
    fi
  done | LC_ALL=C sort
)
if [ "$found_macho" != "$expected_macho" ]; then
  diff_out=$(diff <(printf '%s\n' "$expected_macho") <(printf '%s\n' "$found_macho") || true)
  die "Unexpected Mach-O code in OpenCode Browser Engine.app: $diff_out"
fi
while IFS= read -r rel; do
  actual_cpu=$(lipo -archs "$app/$rel")
  [ "$actual_cpu" = "$expected_cpu" ] ||
    die "$rel has Mach-O architecture '$actual_cpu', expected '$expected_cpu' for macos-$arch."
done <<< "$expected_macho"
echo "ok - verified OpenCode Browser Engine.app $ENGINE_VERSION ($arch; 8 allowlisted Mach-O files, 5 helper bundles)"

# Fail hard before signing if any signing or notarization credential is absent.
[ -n "${SEER_KEYCHAIN:-}" ] || die "SEER_KEYCHAIN is required to sign the macOS browser engine."
[ -n "${SEER_SIGNING_IDENTITY:-}" ] || die "SEER_SIGNING_IDENTITY is required to sign the macOS browser engine."
[ -n "${SEER_NOTARY_KEY_ID:-}" ] || die "SEER_NOTARY_KEY_ID is required to notarize the macOS browser engine."
[ -n "${SEER_NOTARY_ISSUER:-}" ] || die "SEER_NOTARY_ISSUER is required to notarize the macOS browser engine."
[ -n "${SEER_NOTARY_KEY:-}" ] && [ -s "$SEER_NOTARY_KEY" ] ||
  die "SEER_NOTARY_KEY must point to a non-empty App Store Connect API key file."

sign() {
  codesign --force --timestamp --options runtime --keychain "$SEER_KEYCHAIN" --sign "$SEER_SIGNING_IDENTITY" "$@"
}

# Sign strictly the allowlisted nested Mach-O, framework, helper bundles, and outer app inside-out.
fw="$app/Contents/Frameworks/Chromium Embedded Framework.framework"
sign "$fw/Libraries/libcef_sandbox.dylib"
sign "$fw"
for suffix in "Helper" "Helper (GPU)" "Helper (Renderer)" "Helper (Plugin)" "Helper (Alerts)"; do
  sign --entitlements "$entitlements" "$app/Contents/Frameworks/opencode-browser-engine $suffix.app"
done
sign --entitlements "$entitlements" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarize.zip"
xcrun notarytool submit "$work/notarize.zip" \
  --key "$SEER_NOTARY_KEY" \
  --key-id "$SEER_NOTARY_KEY_ID" \
  --issuer "$SEER_NOTARY_ISSUER" \
  --wait \
  --timeout 45m
xcrun stapler staple "$app"
xcrun stapler validate "$app"

mkdir -p "$out"
zip="$out/OpenCode-Browser-Engine-$ENGINE_VERSION-macos-$arch.zip"
rm -f "$zip"
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
ls -lh "$zip"
