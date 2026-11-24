#!/usr/bin/env bash
# Builds OpenCode (Rune).app (anomaly.opencode-rune by default) into OUT, ad-hoc signed, for the
# OpenCode (Rune) release and PR workflows. opencode-rune-sign-macos.sh signs it with the
# Developer ID afterwards. Local dev installs (anomaly.opencode-rune.dev) use
# apps/desktop/scripts/bundle.sh and are never touched here.
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
#   OPENCODE_RUNE_TARGET         (optional Cargo target triple, e.g. x86_64-apple-darwin)
#   OPENCODE_RUNE_PROFILE        (default: "release"; e.g. "release-thin" for faster dev CI)
#   OPENCODE_RUNE_SYMBOLS        (folder for opencode-desktop.dSYM.zip; default: $RUNNER_TEMP/macos, else OUT)
#   OPENCODE_RUNE_KEEP_SYMBOLS   (1 ships the exe with its symbol names and writes no dSYM)
set -euo pipefail
OUT=${1:?usage: build-app.sh OUT}
: "${OPENCODE_RUNE_VERSION:?Set OPENCODE_RUNE_VERSION}"
: "${OPENCODE_RUNE_BUILD_CHANNEL:?Set OPENCODE_RUNE_BUILD_CHANNEL}"
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
APP_NAME=${OPENCODE_RUNE_APP_NAME:-OpenCode (Rune)}
BUNDLE_ID=${OPENCODE_RUNE_BUNDLE_ID:-anomaly.opencode-rune}
ICON_PATH=${OPENCODE_RUNE_ICON:-$ROOT/apps/desktop/assets/icon.icns}
URL_SCHEME=${OPENCODE_RUNE_URL_SCHEME:-opencode-rune}
PROFILE=${OPENCODE_RUNE_PROFILE:-release}
# The fonts and sounds are files in the bundle, not bytes in the exe (apps/desktop/build.rs:
# cfg(bundle_assets); src/workspace/system/bundle.rs): macOS reads a new exe whole before its
# first run, and a file in Resources only when the app opens it.
export OPENCODE_RUNE_BUNDLE_ASSETS=1
# Functions unwind by the linker's compact table instead of a DWARF entry each
# (rustc-compact-unwind.sh): the exe loses most of __eh_frame. OPENCODE_RUNE_COMPACT_UNWIND=0 builds as before.
if [ "${OPENCODE_RUNE_COMPACT_UNWIND:-1}" = 1 ]; then
  export RUSTC_WRAPPER="$ROOT/apps/desktop/scripts/rustc-compact-unwind.sh"
fi

case "$OPENCODE_RUNE_VERSION" in
  0.0.0-dev-[0-9]*) BUILD=${OPENCODE_RUNE_VERSION#0.0.0-dev-} ;;
  0.0.0-pr-[0-9]*-[0-9]*) BUILD=${OPENCODE_RUNE_VERSION##*-} ;;
  *) BUILD=$(printf '%s\n' "${OPENCODE_RUNE_VERSION%%-*}" | awk -F. '{ print ($1 * 10000) + ($2 * 100) + $3 }') ;;
esac

host_triple=$(rustc -vV | sed -n 's/^host: //p')
target_triple=${OPENCODE_RUNE_TARGET:-$host_triple}
cross=0
[ "$target_triple" = "$host_triple" ] || cross=1

if [ -n "${OPENCODE_RUNE_TARGET:-}" ] || { [ "$target_triple" = aarch64-apple-darwin ] && [ "$PROFILE" = release ]; }; then
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/$target_triple/$PROFILE/opencode-desktop"
else
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/$PROFILE/opencode-desktop"
fi

if [ "${OPENCODE_RUNE_SKIP_CARGO_BUILD:-0}" != "1" ]; then
  if [ "$target_triple" = aarch64-apple-darwin ] && [ "$PROFILE" = release ]; then
    # The training build names its target. Keep the same crate identities here and leave
    # host build scripts and proc macros uninstrumented. No training happens on CI.
    (
      cd "$ROOT"
      profile="apps/desktop/pgo/$target_triple.profdata"
      test -s "$profile" || { echo "Missing release profile: $profile" >&2; exit 1; }
      echo "profile-guided: $target_triple ($(head -n 1 "${profile%.profdata}.txt"))" >&2
      # rustc for a dependency may run in that dependency's directory.
      export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="${CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS:+$CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS }-Cprofile-use=$ROOT/$profile -Cllvm-args=-no-pgo-warn-mismatch"
      cargo build --release --locked --target "$target_triple" -p opencode-desktop
    )
  else
    cargo_args=(build --profile "$PROFILE" --locked -p opencode-desktop --manifest-path "$ROOT/Cargo.toml")
    if [ -n "${OPENCODE_RUNE_TARGET:-}" ]; then
      cargo_args+=(--target "$target_triple")
    fi
    cargo "${cargo_args[@]}"
  fi
fi

case "$target_triple" in
  x86_64-apple-darwin) expected_arch=x86_64 ;;
  aarch64-apple-darwin) expected_arch=arm64 ;;
  *) expected_arch= ;;
esac
if [ -n "$expected_arch" ] && command -v lipo >/dev/null 2>&1; then
  actual_arch=$(lipo -archs "$BIN")
  [ "$actual_arch" = "$expected_arch" ] || {
    echo "opencode-desktop arch reported '$actual_arch', expected '$expected_arch'" >&2
    exit 1
  }
fi

if [ "$cross" = "0" ]; then
  EXPECTED_VER="opencode-rune $OPENCODE_RUNE_VERSION ($OPENCODE_RUNE_BUILD_CHANNEL)"
  ACTUAL_VER=$("$BIN" --version)
  [ "$ACTUAL_VER" = "$EXPECTED_VER" ] || {
    echo "opencode-desktop --version reported '$ACTUAL_VER', expected '$EXPECTED_VER'" >&2
    exit 1
  }
fi

mkdir -p "$OUT"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/opencode-desktop"
# The exe ships without its symbol names. macOS reads every byte of a new executable file before
# its first run (XProtect: 5.6-5.9 ms per MiB on an M4), and the names are 27.5 MiB of a 180 MiB
# thin-LTO exe and 9 MiB of a fat-LTO one. They go into a dSYM first (same UUID as the exe, so
# crash reports, `atos -o`, `sample`, lldb and strobe resolve against it), zipped into SYMBOLS:
# the CI job's artifact folder when it exists, else OUT. Unzip it beside an exe to profile it.
if [ "${OPENCODE_RUNE_KEEP_SYMBOLS:-0}" != "1" ]; then
  SYMBOLS=${OPENCODE_RUNE_SYMBOLS:-${RUNNER_TEMP:+$RUNNER_TEMP/macos}}
  [ -n "$SYMBOLS" ] && [ -d "$SYMBOLS" ] || SYMBOLS=$OUT
  rm -rf "$OUT/opencode-desktop.dSYM" "$SYMBOLS/opencode-desktop.dSYM.zip"
  dsymutil "$BIN" -o "$OUT/opencode-desktop.dSYM" 2>&1 | grep -v 'no debug symbols in executable' >&2 || true
  [ "$(dwarfdump --uuid "$OUT/opencode-desktop.dSYM" | awk '{ print $2 }')" = "$(dwarfdump --uuid "$BIN" | awk '{ print $2 }')" ] || {
    echo "opencode-desktop.dSYM does not carry the exe's UUID" >&2
    exit 1
  }
  /usr/bin/ditto -c -k --keepParent "$OUT/opencode-desktop.dSYM" "$SYMBOLS/opencode-desktop.dSYM.zip"
  rm -rf "$OUT/opencode-desktop.dSYM"
  strip "$APP/Contents/MacOS/opencode-desktop" 2>&1 | grep -v 'will invalidate the code signature' >&2 || true
  [ "$(stat -f %z "$APP/Contents/MacOS/opencode-desktop")" -lt "$(stat -f %z "$BIN")" ] || {
    echo "strip left opencode-desktop as large as it was" >&2
    exit 1
  }
  echo "opencode-desktop: $(stat -f %z "$BIN") -> $(stat -f %z "$APP/Contents/MacOS/opencode-desktop") bytes without symbol names; they are in $SYMBOLS/opencode-desktop.dSYM.zip" >&2
fi
cp "$ICON_PATH" "$APP/Contents/Resources/icon.icns"
cp -R "$ROOT/apps/desktop/assets/fonts" "$ROOT/apps/desktop/assets/sounds" "$APP/Contents/Resources/"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$APP/Contents/Resources/THIRD_PARTY_NOTICES.md"

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
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHumanReadableCopyright</key><string>Copyright © $(date +%Y) OpenCode</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSDesktopFolderUsageDescription</key><string>OpenCode (Rune) uses files on your Desktop for projects you open and commands you run there.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>OpenCode (Rune) uses files in your Documents folder for projects you open and commands you run there.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>OpenCode (Rune) uses files in your Downloads folder for projects you open and commands you run there.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>OpenCode (Rune) uses files on network volumes for projects you open and commands you run there.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>OpenCode (Rune) uses files on removable volumes for projects you open and commands you run there.</string>
</dict></plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --sign - "$APP"
echo "$APP"
