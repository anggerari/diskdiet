#!/bin/sh
# Wrap the GUI binary in a minimal macOS app bundle: "Disk Diet.app".
# usage: scripts/bundle-macos.sh <path/to/diskdiet-gui> <out-dir> <version>
set -eu
BIN="$1"
OUT="$2"
VERSION="$3"
APP="$OUT/Disk Diet.app"
ICON="$(dirname "$0")/../assets/DiskDiet.icns"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/diskdiet-gui"
cp "$ICON" "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Disk Diet</string>
  <key>CFBundleDisplayName</key><string>Disk Diet</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>dev.diskdiet.app</string>
  <key>CFBundleExecutable</key><string>diskdiet-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
</dict>
</plist>
PLIST
echo "Created $APP"
