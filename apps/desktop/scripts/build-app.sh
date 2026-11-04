#!/usr/bin/env bash
# Builds OpenCode (Rune).app (anomaly.opencode-rune by default) into OUT, ad-hoc signed, for the
# OpenCode (Rune) release and PR workflows. opencode-rune-sign-macos.sh signs it with the
# Developer ID afterwards. Kit's local install (com.kitlangton.opencode-rune-desktop) remains
# apps/desktop/scripts/bundle.sh and is never touched here.
#
#   apps/desktop/scripts/build-app.sh OUT
#
# Environment:
#   OPENCODE_RUNE_VERSION        (stamped into apps/desktop/Cargo.toml first)
#   OPENCODE_RUNE_BUILD_CHANNEL  (compiled in; `opencode-desktop --version` prints both)
#   OPENCODE_RUNE_APP_NAME       (default: "OpenCode (Rune)")
#   OPENCODE_RUNE_BUNDLE_ID      (default: "anomaly.opencode-rune")
#   OPENCODE_RUNE_ICON           (default: "$ROOT/apps/desktop/assets/icon.icns")
#   OPENCODE_RUNE_URL_SCHEME     (default: "opencode-rune")
set -euo pipefail
OUT=${1:?usage: build-app.sh OUT}
: "${OPENCODE_RUNE_VERSION:?Set OPENCODE_RUNE_VERSION}"
: "${OPENCODE_RUNE_BUILD_CHANNEL:?Set OPENCODE_RUNE_BUILD_CHANNEL}"
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
APP_NAME=${OPENCODE_RUNE_APP_NAME:-OpenCode (Rune)}
BUNDLE_ID=${OPENCODE_RUNE_BUNDLE_ID:-anomaly.opencode-rune}
ICON_PATH=${OPENCODE_RUNE_ICON:-$ROOT/apps/desktop/assets/icon.icns}
URL_SCHEME=${OPENCODE_RUNE_URL_SCHEME:-opencode-rune}

case "$OPENCODE_RUNE_VERSION" in
  0.0.0-dev-[0-9]*) BUILD=${OPENCODE_RUNE_VERSION#0.0.0-dev-} ;;
  0.0.0-pr-[0-9]*-[0-9]*) BUILD=${OPENCODE_RUNE_VERSION##*-} ;;
  *) BUILD=$(printf '%s\n' "${OPENCODE_RUNE_VERSION%%-*}" | awk -F. '{ print ($1 * 10000) + ($2 * 100) + $3 }') ;;
esac

if [ "${OPENCODE_RUNE_SKIP_CARGO_BUILD:-0}" != "1" ]; then
  cargo build --release --locked -p opencode-desktop --manifest-path "$ROOT/Cargo.toml"
fi
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/opencode-desktop"
EXPECTED_VER="opencode-rune $OPENCODE_RUNE_VERSION ($OPENCODE_RUNE_BUILD_CHANNEL)"
ACTUAL_VER=$("$BIN" --version)
[ "$ACTUAL_VER" = "$EXPECTED_VER" ] || {
  echo "opencode-desktop --version reported '$ACTUAL_VER', expected '$EXPECTED_VER'" >&2
  exit 1
}

mkdir -p "$OUT"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/opencode-desktop"
cp "$ICON_PATH" "$APP/Contents/Resources/icon.icns"

xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}
ESC_NAME=$(xml_escape "$APP_NAME")
ESC_ID=$(xml_escape "$BUNDLE_ID")
ESC_SCHEME=$(xml_escape "$URL_SCHEME")

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$ESC_NAME</string>
  <key>CFBundleDisplayName</key><string>$ESC_NAME</string>
  <key>CFBundleIdentifier</key><string>$ESC_ID</string>
  <key>CFBundleExecutable</key><string>opencode-desktop</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$OPENCODE_RUNE_VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key><array><dict><key>CFBundleURLName</key><string>$ESC_ID</string><key>CFBundleURLSchemes</key><array><string>$ESC_SCHEME</string></array></dict></array>
  <key>NSMicrophoneUsageDescription</key><string>OpenCode (Rune) uses the microphone when you dictate prompts or join voice sessions.</string>
</dict></plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --sign - "$APP"
echo "$APP"
