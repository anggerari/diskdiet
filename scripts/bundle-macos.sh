#!/bin/sh
# Wrap the GUI binary in a minimal macOS .app bundle.
# usage: scripts/bundle-macos.sh <path/to/diskdiet-gui> <out-dir> <version>
set -eu
BIN="$1"
OUT="$2"
VERSION="$3"
APP="$OUT/diskdiet.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/diskdiet-gui"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>diskdiet</string>
  <key>CFBundleDisplayName</key><string>diskdiet</string>
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
