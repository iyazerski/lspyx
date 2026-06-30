#!/usr/bin/env sh
set -eu

case "$(uname -s)" in
  Darwin|Linux)
    ;;
  *)
    echo "lspyx installer currently supports macOS and Linux only." >&2
    exit 1
    ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required. Install Rust from https://rustup.rs/, then rerun this script." >&2
  exit 1
fi

cargo install --git https://github.com/iyazerski/lspyx.git --locked

if [ ! -x "$HOME/.cargo/bin/lspyx" ]; then
  echo "cargo finished, but $HOME/.cargo/bin/lspyx was not found." >&2
  exit 1
fi

mkdir -p "$HOME/.local/bin"
ln -sf "$HOME/.cargo/bin/lspyx" "$HOME/.local/bin/lspyx"

PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export PATH

if ! command -v lspyx >/dev/null 2>&1; then
  echo "lspyx installed, but it is not on PATH. Add $HOME/.local/bin to PATH." >&2
  exit 1
fi

if ! command -v ty >/dev/null 2>&1; then
  if ! command -v uv >/dev/null 2>&1; then
    echo "ty is required. Install uv from https://docs.astral.sh/uv/getting-started/installation/, then rerun this script." >&2
    exit 1
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

if command -v codex >/dev/null 2>&1; then
  codex mcp add lspyx -- lspyx mcp serve
fi

cat <<'EOF'

lspyx is installed.

Manual MCP config for other agents:

[mcp_servers.lspyx]
command = "lspyx"
args = ["mcp", "serve"]
EOF
