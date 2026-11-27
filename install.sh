#!/bin/sh
# diskdiet installer: downloads the latest release (including betas) for your platform.
#   curl -fsSL https://raw.githubusercontent.com/anggerari/diskdiet/main/install.sh | sh
# Options (env vars): DISKDIET_VERSION=v0.1.0-beta.1  DISKDIET_INSTALL_DIR=~/.local/bin
set -eu

REPO="anggerari/diskdiet"
DIR="${DISKDIET_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
  Linux-x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "diskdiet: no prebuilt binary for $(uname -s) $(uname -m). Build from source: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
esac

TAG="${DISKDIET_VERSION:-}"
if [ -z "$TAG" ]; then
  # The releases feed lists prereleases too and isn't subject to the
  # GitHub API's anonymous rate limit.
  TAG=$(curl -fsSL "https://github.com/$REPO/releases.atom" | grep -m1 -o 'releases/tag/[^"<]*' | sed 's#releases/tag/##') || true
fi
if [ -z "$TAG" ]; then
  TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=1" | grep -m1 '"tag_name"' | cut -d'"' -f4) || true
fi
[ -n "$TAG" ] || { echo "diskdiet: could not find a release" >&2; exit 1; }

FILE="diskdiet-$TAG-$TARGET.tar.gz"
URL="https://github.com/$REPO/releases/download/$TAG/$FILE"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading diskdiet $TAG for ${TARGET}..."
curl -fsSL "$URL" -o "$TMP/$FILE"
curl -fsSL "$URL.sha256" -o "$TMP/$FILE.sha256"

EXPECTED=$(cut -d' ' -f1 "$TMP/$FILE.sha256")
if command -v shasum >/dev/null 2>&1; then
  ACTUAL=$(shasum -a 256 "$TMP/$FILE" | cut -d' ' -f1)
else
  ACTUAL=$(sha256sum "$TMP/$FILE" | cut -d' ' -f1)
fi
[ "$EXPECTED" = "$ACTUAL" ] || { echo "diskdiet: checksum mismatch" >&2; exit 1; }

tar -xzf "$TMP/$FILE" -C "$TMP"
mkdir -p "$DIR"
install -m 755 "$TMP/diskdiet" "$DIR/diskdiet"
[ -f "$TMP/diskdiet-gui" ] && install -m 755 "$TMP/diskdiet-gui" "$DIR/diskdiet-gui"

echo "Installed diskdiet $TAG to $DIR"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "Add it to your PATH:  export PATH=\"$DIR:\$PATH\"" ;;
esac
echo "Try:  diskdiet scan ~"
