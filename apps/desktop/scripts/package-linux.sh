#!/usr/bin/env bash
# Package a native release build. All three formats install the same executable.
set -euo pipefail
bin=$(realpath "${1:?usage: package-linux.sh binary output-directory}")
mkdir -p "${2:?missing output directory}"
out=$(realpath "$2")
version=${OPENCODE_RUNE_VERSION:?}
expected="opencode-rune $version (${OPENCODE_RUNE_BUILD_CHANNEL:?})"
[[ $("$bin" --version) == "$expected" ]] || { echo "Expected $expected" >&2; exit 1; }

machine=$(uname -m)
case $machine in
  x86_64) arch=x64 deb_arch=amd64 runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64) arch=arm64 deb_arch=arm64 runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  *) echo "Unsupported machine: $machine" >&2; exit 1 ;;
esac
stem="$out/OpenCode-Rune-$version-linux-$arch"
work=$(mktemp -d "$out/.package.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
repo_root=$(realpath "$(dirname "$0")/../../..")
root=$work/root
icon=share/icons/hicolor/1024x1024/apps/opencode-rune.png
desktop=share/applications/opencode-rune.desktop
doc=share/doc/opencode-rune
install -Dm755 "$bin" "$root/usr/bin/opencode-rune"
install -Dm644 "$(dirname "$0")/../assets/icon/icon-1024.png" "$root/usr/$icon"
install -Dm644 "$repo_root/LICENSE" "$root/usr/$doc/LICENSE"
install -Dm644 "$repo_root/THIRD_PARTY_NOTICES.md" "$root/usr/$doc/THIRD_PARTY_NOTICES.md"
mkdir -p "$root/usr/share/applications"
cat > "$root/usr/$desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=OpenCode (Rune)
Exec=opencode-rune
Icon=opencode-rune
Categories=Development;
Terminal=false
EOF

# Type 2 AppImage: a pinned architecture-specific runtime followed by the AppDir as squashfs.
curl --fail --location --retry 3 "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$machine" -o "$work/runtime"
echo "$runtime_sha  $work/runtime" | sha256sum --check --status
mkdir "$work/AppDir"
cp -al "$root/usr" "$work/AppDir/"
ln -s usr/bin/opencode-rune "$work/AppDir/AppRun"
ln -s "usr/$icon" "$work/AppDir/opencode-rune.png"
ln -s "usr/$icon" "$work/AppDir/.DirIcon"
ln -s "usr/$desktop" "$work/AppDir/opencode-rune.desktop"
mksquashfs "$work/AppDir" "$work/app.squashfs" -root-owned -noappend -comp gzip -no-progress
cat "$work/runtime" "$work/app.squashfs" > "$stem.AppImage"
chmod +x "$stem.AppImage"
[[ $(cd "$work" && "$stem.AppImage" --appimage-extract-and-run --version) == "$expected" ]]

# Tilde sorts prereleases before stable; the run number stays numeric for upgrades.
pkg_version=${version/-dev-/~dev.}
mkdir -p "$work/rpm"
cat > "$work/app.spec" <<EOF
Name: opencode-rune
Version: $pkg_version
Release: 1
Summary: OpenCode (Rune) desktop client
License: MIT
Requires: libxkbcommon, fontconfig

%description
OpenCode (Rune) native desktop client.

%install
mkdir -p "%{buildroot}"
cp -a "$root/usr" "%{buildroot}/"

%files
/usr/bin/opencode-rune
/usr/$icon
/usr/$desktop
/usr/$doc/LICENSE
/usr/$doc/THIRD_PARTY_NOTICES.md
EOF
rpmbuild -bb --define "_topdir $work/rpm" --define 'debug_package %{nil}' "$work/app.spec"
cp "$work/rpm/RPMS/$machine/"*.rpm "$stem.rpm"
rpm -qp --list "$stem.rpm" | grep -Fx /usr/bin/opencode-rune

mkdir "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<EOF
Package: opencode-rune
Version: $pkg_version
Architecture: $deb_arch
Maintainer: Anomaly <support@opencode.ai>
Depends: libc6, libgcc-s1, libstdc++6, libasound2, libfontconfig1, libxkbcommon0, libwayland-client0, libx11-6
Description: OpenCode (Rune) native desktop client
EOF
dpkg-deb --root-owner-group --build "$root" "$stem.deb"
dpkg-deb --extract "$stem.deb" "$work/installed"
[[ $("$work/installed/usr/bin/opencode-rune" --version) == "$expected" ]]
