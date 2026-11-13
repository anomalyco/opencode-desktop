#!/usr/bin/env bash
# Build the Linux x64 sidecar and an index the pane can install from file: or HTTP.
set -euo pipefail
engine=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$engine/dist"}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$engine"
[[ $(uname -sm) == 'Linux x86_64' ]]
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
name="OpenCode-Browser-Engine-$version.$revision-linux-x64.tar.zst"
tar --format=ustar --sort=name --mtime=@0 --owner=0 --group=0 -C "$work" -cf - "$bundle" | zstd -T2 -10 -o "$work/$name"
size=$(stat -c %s "$work/$name")
sha=$(sha256sum "$work/$name" | cut -d' ' -f1)
jq -n --arg version "$version-$revision" --arg name "$name" --arg sha "$sha" --argjson size "$size" \
  '{schema:1,artifact:"browser-engine",release:{version:$version,targets:{"linux-x64":{url:$name,size:$size,sha256:$sha}}}}' > "$work/index.json"
mv "$work/$name" "$work/index.json" "$out/"
printf '%s (%s bytes)\nsha256 %s\n' "$out/$name" "$size" "$sha"
