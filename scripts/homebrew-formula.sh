#!/bin/sh
# Print the Homebrew formula for lspyx VERSION, taking checksums from the
# release archives' .sha256 files in DIST. The release workflow writes it to
# Formula/lspyx.rb in iyazerski/homebrew-tap.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 VERSION DIST" >&2
  exit 2
fi
version="$1"
dist="$2"
base="https://github.com/iyazerski/lspyx/releases/download/v$version"

sha() {
  cut -d' ' -f1 "$dist/lspyx-$1.tar.gz.sha256"
}

cat <<EOF
class Lspyx < Formula
  desc "Python code navigation and diagnostics for AI coding agents"
  homepage "https://github.com/iyazerski/lspyx"
  version "$version"
  license "MIT"

  on_macos do
    on_arm do
      url "$base/lspyx-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$base/lspyx-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$base/lspyx-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sha aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$base/lspyx-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sha x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "lspyx"
  end

  def caveats
    <<~EOS
      lspyx runs ty and Ruff. It uses the ones in your project's .venv first,
      then the ones on your PATH. To install them with Homebrew:
        brew install ty ruff
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/lspyx --version")
  end
end
EOF
