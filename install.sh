#!/usr/bin/env sh
set -eu

case "$(uname -s)" in
  Darwin)
    platform="apple-darwin"
    ;;
  Linux)
    platform="unknown-linux-musl"
    ;;
  *)
    echo "lspyx installer currently supports macOS and Linux only." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64|amd64)
    architecture="x86_64"
    ;;
  arm64|aarch64)
    architecture="aarch64"
    ;;
  *)
    echo "lspyx installer supports x86_64 and ARM64 only." >&2
    exit 1
    ;;
esac

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required to download lspyx." >&2
  exit 1
fi

asset="lspyx-$architecture-$platform.tar.gz"
download_url="https://github.com/iyazerski/lspyx/releases/latest/download/$asset"
install_tmp="$(mktemp -d)"
trap 'rm -rf "$install_tmp"' EXIT HUP INT TERM

echo "Downloading $asset..."
curl -fsSL "$download_url" -o "$install_tmp/$asset"
curl -fsSL "$download_url.sha256" -o "$install_tmp/$asset.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$install_tmp" && sha256sum -c "$asset.sha256")
else
  (cd "$install_tmp" && shasum -a 256 -c "$asset.sha256")
fi

tar -xzf "$install_tmp/$asset" -C "$install_tmp"

mkdir -p "$HOME/.local/bin"
install -m 755 "$install_tmp/lspyx" "$HOME/.local/bin/lspyx"

PATH="$HOME/.local/bin:$PATH"
export PATH

if ! command -v lspyx >/dev/null 2>&1; then
  echo "lspyx installed, but it is not on PATH. Add $HOME/.local/bin to PATH." >&2
  exit 1
fi

if ! command -v ty >/dev/null 2>&1; then
  if ! command -v uv >/dev/null 2>&1; then
    echo "Installing uv..."
    curl -LsSf https://astral.sh/uv/install.sh -o "$install_tmp/uv-install.sh"
    sh "$install_tmp/uv-install.sh"
    PATH="$HOME/.local/bin:$PATH"
    export PATH
  fi

  uv tool install ty

  if [ -d "$HOME/.local/bin" ]; then
    PATH="$HOME/.local/bin:$PATH"
    export PATH
  fi
fi

if ! command -v ty >/dev/null 2>&1; then
  echo "ty was installed, but it is not on PATH. Add $HOME/.local/bin to PATH." >&2
  exit 1
fi

if ! command -v ruff >/dev/null 2>&1; then
  if ! command -v uv >/dev/null 2>&1; then
    echo "uv was installed, but it is not on PATH. Add $HOME/.local/bin to PATH." >&2
    exit 1
  fi

  uv tool install ruff

  if [ -d "$HOME/.local/bin" ]; then
    PATH="$HOME/.local/bin:$PATH"
    export PATH
  fi
fi

if ! command -v ruff >/dev/null 2>&1; then
  echo "ruff was installed, but it is not on PATH. Add $HOME/.local/bin to PATH." >&2
  exit 1
fi

echo "lspyx is installed to $HOME/.local/bin/lspyx"
