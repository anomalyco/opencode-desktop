#!/usr/bin/env bash
# Builds, trims, Developer ID signs (hardened runtime + secure timestamp), notarizes and staples
# `OpenCode Browser Engine.app` (arm64 or x64) when credentials are present, and writes:
#   <out-dir>/OpenCode-Browser-Engine-<version>-macos-<arch>.zip
#   <out-dir>/SHA256SUMS
#
# Modes:
#   package-release.sh [--arch arm64|x64] <out-dir>                      build + sign + notarize in one step
#   package-release.sh [--arch arm64|x64] --assemble-only <unsigned.zip> build & assemble unsigned bundle zip on a worker
#   package-release.sh [--arch arm64|x64] --sign-zip <unsigned.zip> <out-dir> sign, notarize, staple & package release zip
#
# CEF is Spotify's prebuilt minimal distribution, fetched once and checked against the SHA-256
# pinned below; cef-dll-sys in Cargo.lock must name the same CEF build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# The engine's version: the CEF build cef-dll-sys names in Cargo.lock, then the engine revision the
# app accepts (ENGINE_REVISION), as package-windows.ps1 has it.
CEF_BUILD="$(grep -A1 '^name = "cef-dll-sys"$' "$ENGINE_DIR/Cargo.lock" | sed -n 's/^version = ".*+\(.*\)"$/\1/p')"
ENGINE_REVISION="$(sed -n 's/^pub const ENGINE_REVISION: u32 = \([0-9]*\);$/\1/p' "$ENGINE_DIR/../../crates/browser-pane/src/installer.rs")"
[[ -n "$CEF_BUILD" && -n "$ENGINE_REVISION" ]] || { echo "CEF build or ENGINE_REVISION not found" >&2; exit 1; }
ENGINE_VERSION="$CEF_BUILD.$ENGINE_REVISION"
MANIFEST="$ENGINE_DIR/Cargo.toml"
ENTITLEMENTS="$ENGINE_DIR/entitlements.plist"

CEF_FULL_VERSION="156.0.26+g8ea0c73+chromium-156.0.8078.88"
if [ "$CEF_BUILD" != "${CEF_FULL_VERSION%%+*}" ]; then
  echo "Cargo.lock's cef-dll-sys is CEF $CEF_BUILD, not ${CEF_FULL_VERSION%%+*}: update CEF_FULL_VERSION and the pinned SHA-256s in package-release.sh." >&2
  exit 1
fi

ARCH="arm64"
MODE="all"
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --arch)
      [ $# -ge 2 ] || { echo "--arch requires a value (arm64 or x64)" >&2; exit 1; }
      ARCH="$2"
      shift 2
      ;;
    --arch=*)
      ARCH="${1#--arch=}"
      shift
      ;;
    --assemble-only)
      MODE="assemble-only"
      shift
      ;;
    --sign-zip)
      MODE="sign-zip"
      shift
      ;;
    --)
      shift
      while [ $# -gt 0 ]; do
        ARGS+=("$1")
        shift
      done
      ;;
    -*)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done

case "$ARCH" in
  arm64 | aarch64)
    ARCH="arm64"
    RUST_TARGET="aarch64-apple-darwin"
    CEF_PLATFORM="macosarm64"
    CEF_DIR_NAME="cef_macos_aarch64"
    CEF_SHA256="6d7936cdd55ef30cf351a6358b2d922becefe87b25a8a8870a2b9ff5af2d39f6"
    ;;
  x64 | x86_64)
    ARCH="x64"
    RUST_TARGET="x86_64-apple-darwin"
    CEF_PLATFORM="macosx64"
    CEF_DIR_NAME="cef_macos_x86_64"
    CEF_SHA256="28b343526664cacb8d855891ce112c62437684b4d9cbdc55fb5a6d7e5eac18fb"
    ;;
  *)
    echo "Unsupported macOS architecture '$ARCH' (expected arm64 or x64)" >&2
    exit 1
    ;;
esac

# Prepare CEF as cef-dll-sys expects it under CEF_PATH (<build>/<dir>, with archive.json),
# checking the downloaded minimal tarball against the pinned SHA-256. Prints the resolved CEF dir
# so cross-builds never need to execute the built target binary.
prepare_cef() {
  local archive_name="cef_binary_${CEF_FULL_VERSION}_${CEF_PLATFORM}_minimal.tar.bz2"
  local versioned_dir="$CEF_PATH/$CEF_BUILD"
  local cef_dir="$versioned_dir/$CEF_DIR_NAME"

  if [ ! -d "$versioned_dir" ] && [ -f "$CEF_PATH/archive.json" ] &&
     [ -d "$CEF_PATH/Chromium Embedded Framework.framework" ] &&
     grep -Fq "cef_binary_${CEF_BUILD}+" "$CEF_PATH/archive.json" &&
     grep -Fq "_${CEF_PLATFORM}_minimal.tar.bz2" "$CEF_PATH/archive.json"; then
    printf '%s\n' "$CEF_PATH"
    return
  fi

  if [ ! -f "$cef_dir/archive.json" ] || [ ! -d "$cef_dir/Chromium Embedded Framework.framework" ]; then
    local download="$CEF_PATH/$archive_name"
    if [ ! -f "$download" ]; then
      local url_name="${archive_name//+/%2B}"
      echo "Downloading $archive_name" >&2
      curl -fsSL --retry 3 -o "$download.part" "https://cef-builds.spotifycdn.com/$url_name"
      mv -f "$download.part" "$download"
    fi
    local sha256
    sha256="$(shasum -a 256 "$download" | awk '{print $1}')"
    if [ "$sha256" != "$CEF_SHA256" ]; then
      rm -f "$download"
      echo "$archive_name has SHA-256 $sha256, not the pinned $CEF_SHA256" >&2
      exit 1
    fi
    local sha1
    sha1="$(shasum -a 1 "$download" | awk '{print $1}')"
    local unpack
    unpack="$(mktemp -d "$CEF_PATH/.unpack.XXXXXXXX")"
    tar -xjf "$download" -C "$unpack"
    local root="$unpack/${archive_name%.tar.bz2}"
    local stage="$unpack/stage"
    mv "$root/Release" "$stage"
    local item
    for item in CMakeLists.txt cmake include libcef_dll CREDITS.html LICENSE.txt; do
      if [ -e "$root/$item" ]; then
        mv "$root/$item" "$stage/"
      fi
    done
    printf '{\n  "type": "minimal",\n  "name": "%s",\n  "sha1": "%s"\n}\n' "$archive_name" "$sha1" > "$stage/archive.json"
    mkdir -p "$versioned_dir"
    rm -rf "$cef_dir"
    mv "$stage" "$cef_dir"
    rm -rf "$unpack" "$download"
  fi

  printf '%s\n' "$cef_dir"
}

assemble_app() {
  local app="$1"
  export CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
  mkdir -p "$CEF_PATH"
  local cef_dir
  cef_dir="$(prepare_cef)"
  local target_dir="${CARGO_TARGET_DIR:-$(cd "$ENGINE_DIR/../.." && pwd)/target/browser-engine}"

  echo "Building opencode-browser-engine $ENGINE_VERSION (--release, $RUST_TARGET)..."
  cargo build -j 4 --locked --release --target "$RUST_TARGET" --manifest-path "$MANIFEST" --target-dir "$target_dir" --bins

  local main_bin="$target_dir/$RUST_TARGET/release/opencode-browser-engine"
  local helper_bin="$target_dir/$RUST_TARGET/release/opencode-browser-engine-helper"
  local host_arch
  host_arch="$(uname -m)"
  if { [ "$ARCH" = "arm64" ] && [ "$host_arch" = "arm64" ]; } ||
     { [ "$ARCH" = "x64" ] && [ "$host_arch" = "x86_64" ]; }; then
    local printed_cef_dir
    printed_cef_dir="$("$main_bin" --print-cef-dir)"
    if [ -n "$printed_cef_dir" ]; then
      cef_dir="$printed_cef_dir"
    fi
  fi
  [ -d "$cef_dir/Chromium Embedded Framework.framework" ] || {
    echo "Chromium Embedded Framework.framework not found in $cef_dir" >&2
    exit 1
  }

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

  local sign_id="${MAC_APP_SIGN:?Set MAC_APP_SIGN to a Developer ID Application identity}"
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
  local zip_name="OpenCode-Browser-Engine-$ENGINE_VERSION-macos-$ARCH.zip"
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

if [ "$MODE" = "assemble-only" ]; then
  [ "${#ARGS[@]}" -eq 1 ] || { echo "Usage: package-release.sh [--arch arm64|x64] --assemble-only <unsigned.zip>" >&2; exit 1; }
  UNSIGNED_ZIP="${ARGS[0]}"
  mkdir -p "$(dirname "$UNSIGNED_ZIP")"
  APP="$WORK/OpenCode Browser Engine.app"
  assemble_app "$APP"
  rm -f "$UNSIGNED_ZIP"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$APP" "$UNSIGNED_ZIP"
  ls -lh "$UNSIGNED_ZIP"
  exit 0
fi

if [ "$MODE" = "sign-zip" ]; then
  [ "${#ARGS[@]}" -eq 2 ] || { echo "Usage: package-release.sh [--arch arm64|x64] --sign-zip <unsigned.zip> <out-dir>" >&2; exit 1; }
  UNSIGNED_ZIP="${ARGS[0]}"
  OUT_DIR="${ARGS[1]}"
  /usr/bin/ditto -x -k "$UNSIGNED_ZIP" "$WORK/unpacked"
  sign_and_notarize "$WORK/unpacked/OpenCode Browser Engine.app" "$OUT_DIR" "$WORK"
  exit 0
fi

[ "${#ARGS[@]}" -eq 1 ] || { echo "Usage: package-release.sh [--arch arm64|x64] <out-dir>" >&2; exit 1; }
OUT_DIR="${ARGS[0]}"
APP="$WORK/OpenCode Browser Engine.app"
assemble_app "$APP"
sign_and_notarize "$APP" "$OUT_DIR" "$WORK"
