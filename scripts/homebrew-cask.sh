#!/bin/sh
# Print a Homebrew cask for the macOS desktop app of a release, using the
# .sha256 files next to the app zips.
# usage: scripts/homebrew-cask.sh <tag, e.g. v0.1.0-beta.1> <dir with *.sha256>
set -eu
TAG="$1"
DIR="$2"
VERSION="${TAG#v}"
sha() { cut -d' ' -f1 "$DIR/diskdiet-app-${TAG}-$1-apple-darwin.zip.sha256"; }
cat <<RUBY
cask "diskdiet" do
  arch arm: "aarch64", intel: "x86_64"

  version "${VERSION}"
  sha256 arm:   "$(sha aarch64)",
         intel: "$(sha x86_64)"

  url "https://github.com/anggerari/diskdiet/releases/download/v#{version}/diskdiet-app-v#{version}-#{arch}-apple-darwin.zip"
  name "Disk Diet"
  desc "Put your storage on a diet: clean dev junk, caches and Docker leftovers"
  homepage "https://github.com/anggerari/diskdiet"

  depends_on macos: :big_sur

  # The app and the \`diskdiet\` CLI ship together. Install this cask OR the
  # diskdiet formula, not both: they would both link bin/diskdiet.
  app "Disk Diet.app"
  binary "#{appdir}/Disk Diet.app/Contents/MacOS/diskdiet"

  # The app is not notarized yet (beta); let it open without a Gatekeeper prompt.
  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "{{appdir}}/Disk Diet.app"],
        must_succeed:   false,
        writable_paths: ["Disk Diet.app"],
        writable_base:  :appdir
  end

  zap trash: [
    "~/.config/diskdiet",
    "~/Library/Application Support/diskdiet",
    "~/Library/Logs/diskdiet.log",
  ]
end
RUBY
