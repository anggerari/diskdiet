#!/bin/sh
# Print a Homebrew formula for a release, using the .sha256 files next to the archives.
# usage: scripts/homebrew-formula.sh <tag, e.g. v0.1.0-beta.1> <dir with *.sha256>
set -eu
TAG="$1"
DIR="$2"
VERSION="${TAG#v}"
BASE="https://github.com/anggerari/diskdiet/releases/download/${TAG}"
sha() { cut -d' ' -f1 "$DIR/diskdiet-${TAG}-$1.tar.gz.sha256"; }
cat <<RUBY
class Diskdiet < Formula
  desc "Put your storage on a diet: clean node_modules, build artifacts, caches and Docker"
  homepage "https://github.com/anggerari/diskdiet"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "${BASE}/diskdiet-${TAG}-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "${BASE}/diskdiet-${TAG}-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_intel do
      url "${BASE}/diskdiet-${TAG}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "$(sha x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "diskdiet"
    bin.install "diskdiet-gui"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/diskdiet --version")
  end
end
RUBY
