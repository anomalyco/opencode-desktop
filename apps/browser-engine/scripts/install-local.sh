#!/usr/bin/env bash
set -euo pipefail

# Builds, trims, signs, and installs `OpenCode Browser Engine.app` into
# `~/Library/Application Support/co.anomaly.opencode/browser-engine/<version>/`
# (or a custom directory passed as `$1`).

ENGINE_VERSION="156.0.14.1"
DEST_ROOT="${1:-$HOME/Library/Application Support/co.anomaly.opencode/browser-engine/$ENGINE_VERSION}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$SCRIPT_DIR/../Cargo.toml"

export CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
mkdir -p "$CEF_PATH"

TARGET_DIR="${CARGO_TARGET_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)/target/browser-engine}"

echo "Building opencode-browser-engine (--release)..."
cargo build -j 4 --release --manifest-path "$MANIFEST" --target-dir "$TARGET_DIR" --bins

MAIN_BIN="$TARGET_DIR/release/opencode-browser-engine"
HELPER_BIN="$TARGET_DIR/release/opencode-browser-engine-helper"
CEF_DIR="$("$MAIN_BIN" --print-cef-dir)"

STAGE="$DEST_ROOT.staging.$$"
APP="$STAGE/OpenCode Browser Engine.app"
rm -rf "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"

cp "$MAIN_BIN" "$APP/Contents/MacOS/opencode-browser-engine"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>opencode-browser-engine</string>
  <key>CFBundleDisplayName</key><string>OpenCode Browser Engine</string>
  <key>CFBundleIdentifier</key><string>co.anomaly.opencode.browser-engine</string>
  <key>CFBundleExecutable</key><string>opencode-browser-engine</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>156.0.14.1</string>
  <key>CFBundleShortVersionString</key><string>156.0.14.1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSEnvironment</key><dict>
    <key>MallocNanoZone</key><string>0</string>
  </dict>
</dict></plist>
PLIST

ditto "$CEF_DIR/Chromium Embedded Framework.framework" "$APP/Contents/Frameworks/Chromium Embedded Framework.framework"
# Strip software Vulkan fallback (macOS uses Metal) and unused locale dirs (~40 MiB saved).
rm -f "$APP/Contents/Frameworks/Chromium Embedded Framework.framework/Libraries/libvk_swiftshader.dylib"
rm -f "$APP/Contents/Frameworks/Chromium Embedded Framework.framework/Libraries/vk_swiftshader_icd.json"
find "$APP/Contents/Frameworks/Chromium Embedded Framework.framework/Resources" -maxdepth 1 -name '*.lproj' ! -name 'en.lproj' -exec rm -rf {} +

SIGN_ID="${MAC_APP_SIGN:--}"

for SUFFIX in "Helper" "Helper (GPU)" "Helper (Renderer)" "Helper (Plugin)" "Helper (Alerts)"; do
  HNAME="opencode-browser-engine $SUFFIX"
  HAPP="$APP/Contents/Frameworks/$HNAME.app"
  mkdir -p "$HAPP/Contents/MacOS"
  cp "$HELPER_BIN" "$HAPP/Contents/MacOS/$HNAME"
  ID_SUFFIX="$(echo "$SUFFIX" | tr ' ()' '-' | tr '[:upper:]' '[:lower:]' | sed 's/--*/-/g; s/-$//')"
  cat > "$HAPP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$HNAME</string>
  <key>CFBundleDisplayName</key><string>$HNAME</string>
  <key>CFBundleIdentifier</key><string>co.anomaly.opencode.browser-engine.$ID_SUFFIX</string>
  <key>CFBundleExecutable</key><string>$HNAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>156.0.14.1</string>
  <key>CFBundleShortVersionString</key><string>156.0.14.1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>LSEnvironment</key><dict>
    <key>MallocNanoZone</key><string>0</string>
  </dict>
</dict></plist>
PLIST
  codesign --force --sign "$SIGN_ID" "$HAPP" >/dev/null 2>&1 || codesign --force --sign - "$HAPP" >/dev/null 2>&1 || true
done

codesign --force --deep --sign "$SIGN_ID" "$APP/Contents/Frameworks/Chromium Embedded Framework.framework" >/dev/null 2>&1 \
  || codesign --force --deep --sign - "$APP/Contents/Frameworks/Chromium Embedded Framework.framework" >/dev/null 2>&1 || true
codesign --force --sign "$SIGN_ID" "$APP" >/dev/null 2>&1 || codesign --force --sign - "$APP" >/dev/null 2>&1 || true

mkdir -p "$(dirname "$DEST_ROOT")"
rm -rf "$DEST_ROOT.old"
[ -d "$DEST_ROOT" ] && mv "$DEST_ROOT" "$DEST_ROOT.old"
mv "$STAGE" "$DEST_ROOT"
rm -rf "$DEST_ROOT.old"

echo "Installed: $DEST_ROOT/OpenCode Browser Engine.app"
