#!/bin/sh
# Build a Debian/Ubuntu package with the CLI, the desktop app, a menu entry
# and the icon.
# usage: scripts/package-deb.sh <dir with diskdiet + diskdiet-gui> <out-dir> <version> [arch]
set -eu
BIN="$1"
OUT="$2"
VERSION="$3"
ARCH="${4:-amd64}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Debian sorts "~" before anything, so 0.1.0~beta.4 < 0.1.0 like semver.
DEBVER="$(printf '%s' "$VERSION" | sed 's/-/~/')"
PKG="$OUT/deb-root"

rm -rf "$PKG"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/bin" "$PKG/usr/share/applications" \
  "$PKG/usr/share/icons/hicolor/256x256/apps" "$PKG/usr/share/icons/hicolor/1024x1024/apps" \
  "$PKG/usr/share/doc/diskdiet"
install -m 755 "$BIN/diskdiet" "$BIN/diskdiet-gui" "$PKG/usr/bin/"
install -m 644 "$ROOT/assets/icon-256.png" "$PKG/usr/share/icons/hicolor/256x256/apps/diskdiet.png"
install -m 644 "$ROOT/assets/icon-1024.png" "$PKG/usr/share/icons/hicolor/1024x1024/apps/diskdiet.png"
install -m 644 "$ROOT/README.md" "$PKG/usr/share/doc/diskdiet/README.md"
install -m 644 "$ROOT/LICENSE" "$PKG/usr/share/doc/diskdiet/copyright"

cat > "$PKG/usr/share/applications/diskdiet.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Disk Diet
GenericName=Storage Cleaner
Comment=Clean up node_modules, build folders, caches and Docker leftovers
Exec=diskdiet-gui
Icon=diskdiet
Terminal=false
Categories=Utility;System;
Keywords=disk;storage;clean;cache;docker;node_modules;
StartupWMClass=diskdiet
DESKTOP

SIZE="$(du -sk "$PKG/usr" | cut -f1)"
cat > "$PKG/DEBIAN/control" <<CONTROL
Package: diskdiet
Version: ${DEBVER}
Architecture: ${ARCH}
Maintainer: anggerari <anggerari@users.noreply.github.com>
Installed-Size: ${SIZE}
Depends: libc6, libgl1, libxkbcommon0
Recommends: git, zenity
Section: utils
Priority: optional
Homepage: https://github.com/anggerari/diskdiet
Description: Put your storage on a diet (beta)
 Finds and removes node_modules, build folders, package-manager caches,
 Docker leftovers and big forgotten files. Comes with the diskdiet command
 and the Disk Diet desktop app.
CONTROL

# Keep "~" out of the file name (release asset names); it's only for dpkg.
DEB="$OUT/diskdiet_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKG" "$DEB"
rm -rf "$PKG"
echo "Created $DEB"
