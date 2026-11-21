#!/usr/bin/env bash
# Build the Linux sidecar (x64 or arm64) and an index the pane can install from file: or HTTP.
#
#   package-linux.sh [--arch x64|arm64] [<out-dir>]
#
# CEF is Spotify's prebuilt minimal distribution, fetched once and checked against the SHA-256
# pinned below; cef-dll-sys in Cargo.lock must name the same CEF build.
set -euo pipefail

engine=$(cd "$(dirname "$0")/.." && pwd)
arch=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --arch)
      [ $# -ge 2 ] || { echo "--arch requires a value (x64 or arm64)" >&2; exit 1; }
      arch="$2"
      shift 2
      ;;
    --arch=*)
      arch="${1#--arch=}"
      shift
      ;;
    --)
      shift
      if [ $# -gt 0 ]; then
        [ -z "$out" ] || { echo "Unexpected extra argument: $1" >&2; exit 1; }
        out="$1"
        shift
      fi
      ;;
    -*)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
    *)
      [ -z "$out" ] || { echo "Unexpected extra argument: $1" >&2; exit 1; }
      out="$1"
      shift
      ;;
  esac
done

out=${out:-"$engine/dist"}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$engine"

[[ $(uname -s) == 'Linux' ]] || { echo "package-linux.sh must run on Linux (got $(uname -s))" >&2; exit 1; }
host_machine=$(uname -m)
if [ -z "$arch" ]; then
  case "$host_machine" in
    x86_64) arch=x64 ;;
    aarch64) arch=arm64 ;;
    *) echo "Unsupported Linux machine '$host_machine' (expected x86_64 or aarch64)" >&2; exit 1 ;;
  esac
fi

case "$arch" in
  x64 | x86_64)
    arch=x64
    expected_machine=x86_64
    cef_platform=linux64
    cef_dir_name=cef_linux_x86_64
    cef_sha256=600ac0b8f37cb953f884e7c89998a6c200d24959aa74820ad431b61d5ebcf360
    target_key=linux-x64
    ;;
  arm64 | aarch64)
    arch=arm64
    expected_machine=aarch64
    cef_platform=linuxarm64
    cef_dir_name=cef_linux_aarch64
    cef_sha256=aa652f6c583214114b8aed209313f2a81ade8b4f92d30681768d0f1f8332e0e3
    target_key=linux-arm64
    ;;
  *)
    echo "Unsupported Linux architecture '$arch' (expected x64 or arm64)" >&2
    exit 1
    ;;
esac

[[ "$host_machine" == "$expected_machine" ]] || {
  echo "package-linux.sh --arch $arch must run on Linux $expected_machine (got $host_machine)" >&2
  exit 1
}

cef_full_version='156.0.26+g8ea0c73+chromium-156.0.8078.88'
cef_build=$(grep -A1 '^name = "cef-dll-sys"$' "$engine/Cargo.lock" | sed -n 's/^version = ".*+\(.*\)"$/\1/p')
if [ "$cef_build" != "${cef_full_version%%+*}" ]; then
  echo "Cargo.lock's cef-dll-sys is CEF $cef_build, not ${cef_full_version%%+*}: update cef_full_version and the pinned SHA-256s in package-linux.sh." >&2
  exit 1
fi

export CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
mkdir -p "$CEF_PATH"
archive_name="cef_binary_${cef_full_version}_${cef_platform}_minimal.tar.bz2"
versioned_dir="$CEF_PATH/$cef_build"
cef_dir="$versioned_dir/$cef_dir_name"
if [ ! -d "$versioned_dir" ] && [ -f "$CEF_PATH/archive.json" ] && [ -f "$CEF_PATH/libcef.so" ] &&
   grep -Fq "cef_binary_${cef_build}+" "$CEF_PATH/archive.json" &&
   grep -Fq "_${cef_platform}_minimal.tar.bz2" "$CEF_PATH/archive.json"; then
  cef_dir="$CEF_PATH"
elif [ ! -f "$cef_dir/archive.json" ] || [ ! -f "$cef_dir/libcef.so" ]; then
  download="$CEF_PATH/$archive_name"
  if [ ! -f "$download" ]; then
    url_name="${archive_name//+/%2B}"
    echo "Downloading $archive_name" >&2
    curl -fsSL --retry 3 -o "$download.part" "https://cef-builds.spotifycdn.com/$url_name"
    mv -f "$download.part" "$download"
  fi
  actual_sha256=$(sha256sum "$download" | cut -d' ' -f1)
  if [ "$actual_sha256" != "$cef_sha256" ]; then
    rm -f "$download"
    echo "$archive_name has SHA-256 $actual_sha256, not the pinned $cef_sha256" >&2
    exit 1
  fi
  sha1=$(sha1sum "$download" | cut -d' ' -f1)
  unpack=$(mktemp -d "$CEF_PATH/.unpack.XXXXXXXX")
  tar -xjf "$download" -C "$unpack"
  root="$unpack/${archive_name%.tar.bz2}"
  stage="$unpack/stage"
  mv "$root/Release" "$stage"
  mv "$root/Resources"/* "$stage/"
  for item in CMakeLists.txt cmake include libcef_dll CREDITS.html LICENSE.txt; do
    if [ -e "$root/$item" ]; then
      mv "$root/$item" "$stage/"
    fi
  done
  jq -n --arg name "$archive_name" --arg sha1 "$sha1" '{type:"minimal",name:$name,sha1:$sha1}' > "$stage/archive.json"
  mkdir -p "$versioned_dir"
  rm -rf "$cef_dir"
  mv "$stage" "$cef_dir"
  rm -rf "$unpack" "$download"
fi

cargo build --locked --release --bins
bin=$(cargo metadata --no-deps --format-version 1 | jq -r .target_directory)/release
cef=$("$bin/opencode-browser-engine" --print-cef-dir)
version=$(sed -n 's/^#define CEF_VERSION "\([0-9.]*\)+.*"/\1/p' "$cef/include/cef_version.h")
revision=$(sed -n 's/^pub const ENGINE_REVISION: u32 = \([0-9]*\);/\1/p' ../../crates/browser-pane/src/installer.rs)
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && $revision =~ ^[0-9]+$ ]]
work=$(mktemp -d "$out/.linux.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
bundle='OpenCode Browser Engine'
app="$work/$bundle"
mkdir -p "$app/locales"
install -m755 "$bin/opencode-browser-engine" "$bin/opencode-browser-engine-helper" "$app/"
# The distribution carries debug symbols in libcef.so. Only its dynamic symbols ship.
strip --strip-unneeded -o "$app/libcef.so" "$cef/libcef.so"
install -m644 "$cef"/{chrome_100_percent.pak,chrome_200_percent.pak,icudtl.dat,resources.pak,v8_context_snapshot.bin,CREDITS.html} "$app/"
install -m644 "$cef/libvulkan.so.1" "$app/"
if [[ -f "$cef/LICENSE.txt" ]]; then
  install -m644 "$cef/LICENSE.txt" "$app/"
else
  # download-cef's flattened runtime omits the root license; its original tarball is adjacent.
  archive=$(jq -er .name "$cef/archive.json")
  tar -xOjf "$(dirname "$cef")/$archive" "${archive%.tar.bz2}/LICENSE.txt" > "$app/LICENSE.txt"
fi
for locale in $(jq -r '.locales[].id' ../../crates/opencode-i18n/locales/locales.json); do
  case $locale in en) locale=en-US;; zh) locale=zh-CN;; zht) locale=zh-TW;; no) locale=nb;; esac
  if [[ -f "$cef/locales/$locale.pak" ]]; then install -m644 "$cef/locales/$locale.pak" "$app/locales/"; fi
done
name="OpenCode-Browser-Engine-$version.$revision-$target_key.tar.zst"
tar --format=ustar --sort=name --mtime=@0 --owner=0 --group=0 -C "$work" -cf - "$bundle" | zstd -T2 -10 -o "$work/$name"
size=$(stat -c %s "$work/$name")
sha=$(sha256sum "$work/$name" | cut -d' ' -f1)
jq -n --arg version "$version-$revision" --arg target "$target_key" --arg name "$name" --arg sha "$sha" --argjson size "$size" \
  '{schema:1,artifact:"browser-engine",release:{version:$version,targets:{($target):{url:$name,size:$size,sha256:$sha}}}}' > "$work/index.json"
mv "$work/$name" "$work/index.json" "$out/"
printf '%s (%s bytes)\nsha256 %s\n' "$out/$name" "$size" "$sha"
