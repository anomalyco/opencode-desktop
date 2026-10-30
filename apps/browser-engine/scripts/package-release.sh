#!/usr/bin/env bash
# Builds, trims, Developer ID signs (hardened runtime + secure timestamp), notarizes and staples
# `OpenCode Browser Engine.app` (arm64) when credentials are present, and writes:
#   <out-dir>/OpenCode-Browser-Engine-<version>-macos-arm64.zip
#   <out-dir>/SHA256SUMS
#
# Modes:
#   package-release.sh <out-dir>                      build + sign + notarize in one step
#   package-release.sh --assemble-only <unsigned.zip> build & assemble unsigned bundle zip on a worker
#   package-release.sh --sign-zip <unsigned.zip> <out-dir> sign, notarize, staple & package release zip
set -euo pipefail

ENGINE_VERSION="156.0.14"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$ENGINE_DIR/Cargo.toml"
ENTITLEMENTS="$ENGINE_DIR/entitlements.plist"

assemble_app() {
  local app="$1"
  export CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
  mkdir -p "$CEF_PATH"
  local target_dir="${CARGO_TARGET_DIR:-$(cd "$ENGINE_DIR/../.." && pwd)/target/browser-engine}"

  echo "Building opencode-browser-engine $ENGINE_VERSION (--release, aarch64-apple-darwin)..."
  cargo build -j 4 --release --target aarch64-apple-darwin --manifest-path "$MANIFEST" --target-dir "$target_dir" --bins

  local main_bin="$target_dir/aarch64-apple-darwin/release/opencode-browser-engine"
  local helper_bin="$target_dir/aarch64-apple-darwin/release/opencode-browser-engine-helper"
  local cef_dir
  cef_dir="$("$main_bin" --print-cef-dir)"

  mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources"
  cp "$main_bin" "$app/Contents/MacOS/opencode-browser-engine"

  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>opencode-browser-engine</string>
  <key>CFBundleDisplayName</key><string>OpenCode Browser Engine</string>
  <key>CFBundleIdentifier</key><string>co.anomaly.opencode.browser-engine</string>
  <key>CFBundleExecutable</key><string>opencode-browser-engine</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$ENGINE_VERSION</string>
  <key>CFBundleShortVersionString</key><string>$ENGINE_VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSEnvironment</key><dict>
    <key>MallocNanoZone</key><string>0</string>
  </dict>
</dict></plist>
PLIST

  local fw="$app/Contents/Frameworks/Chromium Embedded Framework.framework"
  ditto "$cef_dir/Chromium Embedded Framework.framework" "$fw"
  # Strip software Vulkan fallback (macOS uses Metal) and unused locale dirs (~41 MiB saved).
  rm -f "$fw/Libraries/libvk_swiftshader.dylib" "$fw/Libraries/vk_swiftshader_icd.json" "$fw/Libraries/libvulkan.dylib"
  find "$fw/Resources" -maxdepth 1 -name '*.lproj' ! -name 'en.lproj' -exec rm -rf {} +

  for suffix in "Helper" "Helper (GPU)" "Helper (Renderer)" "Helper (Plugin)" "Helper (Alerts)"; do
    local hname="opencode-browser-engine $suffix"
    local happ="$app/Contents/Frameworks/$hname.app"
    mkdir -p "$happ/Contents/MacOS"
    cp "$helper_bin" "$happ/Contents/MacOS/$hname"
    local id_suffix
    id_suffix="$(echo "$suffix" | tr ' ()' '-' | tr '[:upper:]' '[:lower:]' | sed 's/--*/-/g; s/-$//')"
    cat > "$happ/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$hname</string>
  <key>CFBundleDisplayName</key><string>$hname</string>
  <key>CFBundleIdentifier</key><string>co.anomaly.opencode.browser-engine.$id_suffix</string>
  <key>CFBundleExecutable</key><string>$hname</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$ENGINE_VERSION</string>
  <key>CFBundleShortVersionString</key><string>$ENGINE_VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>LSEnvironment</key><dict>
    <key>MallocNanoZone</key><string>0</string>
  </dict>
</dict></plist>
PLIST
  done
}

sign_and_notarize() {
  local app="$1"
  local out_dir="$2"
  local work="$3"

  local sign_id="${MAC_APP_SIGN:-Developer ID Application: Example Developer (TEAMID0000)}"
  local providers_json="$HOME/.config/app-store-connect/providers.json"
  local notary_key_id="${NOTARY_KEY_ID:-}"
  local notary_issuer="${NOTARY_ISSUER:-}"
  if [ -z "$notary_key_id" ] && [ -f "$providers_json" ]; then
    local team_id
    team_id="$(echo "$sign_id" | sed -n 's/.*(\([A-Z0-9]*\)).*/\1/p')"
    read -r notary_key_id notary_issuer < <(python3 - "$providers_json" "$team_id" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
tid = sys.argv[2] or d.get("defaultTeamId", "")
p = d.get("providers", {}).get(tid, {})
print(p.get("keyId", ""), p.get("issuerId", ""))
PY
    ) || true
  fi
  local notary_key="${NOTARY_KEY:-$HOME/.appstoreconnect/private_keys/AuthKey_${notary_key_id:-none}.p8}"

  sign_macho() {
    codesign --force --timestamp --options runtime --sign "$sign_id" "$@"
  }

  local fw="$app/Contents/Frameworks/Chromium Embedded Framework.framework"
  echo "Signing nested CEF Mach-O binaries inside-out..."
  while IFS= read -r -d '' f; do
    [ "$(basename "$f")" = "Chromium Embedded Framework" ] && continue
    if file -b "$f" | grep -q "Mach-O"; then
      if file -b "$f" | grep -q "executable"; then
        sign_macho --entitlements "$ENTITLEMENTS" "$f"
      else
        sign_macho "$f"
      fi
    fi
  done < <(find "$fw" -type f -print0)

  sign_macho "$fw"

  for suffix in "Helper" "Helper (GPU)" "Helper (Renderer)" "Helper (Plugin)" "Helper (Alerts)"; do
    sign_macho --entitlements "$ENTITLEMENTS" "$app/Contents/Frameworks/opencode-browser-engine $suffix.app"
  done

  sign_macho --entitlements "$ENTITLEMENTS" "$app"
  codesign --verify --deep --strict --verbose=2 "$app"

  if [ -n "$notary_key_id" ] && [ -n "$notary_issuer" ] && [ -f "$notary_key" ]; then
    echo "Submitting to Apple Notary Service..."
    /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarize.zip"
    xcrun notarytool submit "$work/notarize.zip" \
      --key "$notary_key" \
      --key-id "$notary_key_id" \
      --issuer "$notary_issuer" \
      --wait \
      --timeout 30m
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
    spctl -a -vvv -t exec "$app"
  else
    echo "No notary credentials found; bundle is Developer ID signed without notarization."
  fi

  mkdir -p "$out_dir"
  out_dir="$(cd "$out_dir" && pwd)"
  local zip_name="OpenCode-Browser-Engine-$ENGINE_VERSION-macos-arm64.zip"
  local zip_path="$out_dir/$zip_name"
  rm -f "$zip_path" "$out_dir/SHA256SUMS"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$zip_path"
  (cd "$out_dir" && shasum -a 256 "$zip_name" > SHA256SUMS)
  cat "$out_dir/SHA256SUMS"
  stat -f '%z %N' "$zip_path"
  ls -lh "$zip_path"
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ "${1:-}" = "--assemble-only" ]; then
  UNSIGNED_ZIP="${2:?Usage: package-release.sh --assemble-only <unsigned.zip>}"
  mkdir -p "$(dirname "$UNSIGNED_ZIP")"
  APP="$WORK/OpenCode Browser Engine.app"
  assemble_app "$APP"
  rm -f "$UNSIGNED_ZIP"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$APP" "$UNSIGNED_ZIP"
  ls -lh "$UNSIGNED_ZIP"
  exit 0
fi

if [ "${1:-}" = "--sign-zip" ]; then
  UNSIGNED_ZIP="${2:?Usage: package-release.sh --sign-zip <unsigned.zip> <out-dir>}"
  OUT_DIR="${3:?Usage: package-release.sh --sign-zip <unsigned.zip> <out-dir>}"
  /usr/bin/ditto -x -k "$UNSIGNED_ZIP" "$WORK/unpacked"
  sign_and_notarize "$WORK/unpacked/OpenCode Browser Engine.app" "$OUT_DIR" "$WORK"
  exit 0
fi

OUT_DIR="${1:?Usage: package-release.sh <out-dir>}"
APP="$WORK/OpenCode Browser Engine.app"
assemble_app "$APP"
sign_and_notarize "$APP" "$OUT_DIR" "$WORK"
