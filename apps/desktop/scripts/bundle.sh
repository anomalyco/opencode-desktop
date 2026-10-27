#!/usr/bin/env bash
# Wraps a release build in "OpenCode (Rune).app" so it gets a Dock icon and a name,
# installs it to ~/Applications and opens it. Pass app arguments after the binary path:
#   apps/desktop/scripts/bundle.sh target/release/opencode-desktop --live
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=${1:-../../target/release/opencode-desktop}
shift || true
APP="$HOME/Applications/OpenCode (Rune).app"
test -x "$BIN" || { echo "no binary at $BIN (build with: cargo build --release -p opencode-desktop)" >&2; exit 1; }
pkill -f "$APP/Contents/MacOS/opencode-desktop" 2>/dev/null || true
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/opencode-desktop"
cp assets/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>OpenCode (Rune)</string>
  <key>CFBundleDisplayName</key><string>OpenCode (Rune)</string>
  <key>CFBundleIdentifier</key><string>com.kitlangton.opencode-rune</string>
  <key>CFBundleExecutable</key><string>opencode-desktop</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
codesign --force --sign - "$APP" >/dev/null 2>&1 || true
open "$APP" --args "$@"
echo "$APP"
