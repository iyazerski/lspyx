<div align="center">

<img src="docs/icon.png" width="112" alt="lspyx icon">

# lspyx

**Python code navigation and diagnostics for AI coding agents.**

Give Claude Code, Codex, Cursor and other agents go to definition, find usages, safe rename previews,<br>
and ty and Ruff checks through one small MCP server. Or use the same answers from your terminal.

[![Latest release](https://img.shields.io/github/v/release/iyazerski/lspyx?color=3b6cf6)](https://github.com/iyazerski/lspyx/releases/latest)
[![crates.io](https://img.shields.io/crates/v/lspyx?color=orange)](https://crates.io/crates/lspyx)
[![CI](https://github.com/iyazerski/lspyx/actions/workflows/ci.yml/badge.svg)](https://github.com/iyazerski/lspyx/actions/workflows/ci.yml)
[![macOS · Linux](https://img.shields.io/badge/macOS%20·%20Linux-x86__64%20·%20ARM64-black)](#install)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)

<img src="docs/demo.gif" width="720" alt="lspyx finding a class, the usages of a method, a file outline and a definition in a small Python project">

</div>

## Why lspyx

Coding agents explore Python with `grep` and by reading whole files. That's slow, it fills the context window, and text search can't tell `Cart.add` from every other `add`. lspyx answers with the type checker instead.

- **Semantic answers.** Definitions, usages, types and hover docs come from [ty](https://github.com/astral-sh/ty), Astral's Python type checker and language server. A usage is a real reference, not a matching string.
- **Small, focused output.** Every result is a `file:line:column` with its source line, so the agent can jump straight there. A `limit` keeps big results short.
- **Safe rename previews.** `rename` returns a complete unified diff for a cross-file rename and checks every edit. It never writes files: the agent reviews and applies the diff.
- **Your repo's own checks.** `diagnostics` runs [Ruff](https://github.com/astral-sh/ruff) and ty with your project's settings, on a file, a folder or the whole repo.
- **Fast after the first call.** A background daemon per workspace keeps ty warm, so later calls take milliseconds. It stops by itself after 30 idle minutes.
- **Uses your project's tools.** lspyx prefers `ty` and `ruff` from the project's `.venv`, then the ones on your `PATH`.
- **Local and small.** One Rust binary for macOS and Linux. It sends nothing over the network, except when you run `lspyx update`.
- **Free and open source** under the MIT license.

## MCP tools

| Tool | What it does | Pass |
| --- | --- | --- |
| `explore` | Search symbols by name, outline a file, or inspect the symbol at a position with its definition and usages. | `query`, or `file`, or `file` + `line` + `column`. Optional `limit`, `kind`, `depth`. |
| `diagnostics` | Run Ruff and ty with the repo's settings and return their findings. | `workspace` and the smallest useful `path`. Optional `limit`. |
| `rename` | Preview a semantic rename as a complete unified diff, without changing files. | `file` + `line` + `column` + `new_name`. |

Lines and columns start at 1. Relative paths need `workspace`, the repo root.

## Install

lspyx runs on macOS and Linux, on x86_64 and ARM64. Windows isn't supported. Pick one:

- **Homebrew:**

  ```sh
  brew install iyazerski/tap/lspyx
  ```

- **Cargo:**

  ```sh
  cargo install lspyx --locked
  ```

- **Script.** It puts lspyx in `~/.local/bin` and installs ty and Ruff with [uv](https://docs.astral.sh/uv/) if they're missing:

  ```sh
  curl -fsSL https://raw.githubusercontent.com/iyazerski/lspyx/main/install.sh | sh
  ```

lspyx needs ty and Ruff, either in your project's `.venv` or on your `PATH`. With Homebrew or Cargo, install them yourself if you don't have them:

```sh
brew install ty ruff     # or: uv tool install ty && uv tool install ruff
```

Then check the setup from your project folder:

```sh
lspyx doctor
```

## Connect your agent

lspyx speaks MCP over stdio. The command is `lspyx mcp serve`.

**Claude Code**

```sh
claude mcp add --scope user lspyx -- lspyx mcp serve
```

**Codex**

```sh
codex mcp add lspyx -- lspyx mcp serve
```

**pi**

```sh
pi mcp add lspyx -- lspyx mcp serve
```

**Cursor:** add this to `~/.cursor/mcp.json`, or to `.cursor/mcp.json` in a project:

```json
{
  "mcpServers": {
    "lspyx": { "command": "lspyx", "args": ["mcp", "serve"] }
  }
}
```

**VS Code:** add this to `.vscode/mcp.json`:

```json
{
  "servers": {
    "lspyx": { "type": "stdio", "command": "lspyx", "args": ["mcp", "serve"] }
  }
}
```

**Other clients:** run `lspyx` with the arguments `mcp serve` as a stdio server.

Then add this line to your `AGENTS.md` or `CLAUDE.md`, so the agent reaches for lspyx instead of `grep`:

```md
- Use lspyx MCP for Python code navigation, diagnostics, and refactors. Use `explore` to understand code, `diagnostics` to check changes, and `rename` to preview cross-file renames.
```

## Use it from the terminal

The CLI gives the same navigation answers as `explore`. Positions are `file:line:column`, starting at 1.

| Command | What it does |
| --- | --- |
| `lspyx find-symbol User --kind class` | Search the workspace for symbols by name. |
| `lspyx outline src/app.py` | Show a file's classes, functions and methods. Add `--full` for the whole tree. |
| `lspyx inspect src/app.py:42:17` | Show the symbol at a position, with its signature and docs. |
| `lspyx goto src/app.py:42:17` | Jump to the definition. Add `--kind declaration` or `--kind type`. |
| `lspyx usages src/app.py:42:17` | List every usage. Add `--exclude-declaration` to skip the definition. |
| `lspyx doctor` | Show which ty and Ruff lspyx uses, and whether the daemon is running. |
| `lspyx daemon status` | Check, start (`ensure`) or `stop` the workspace daemon. |
| `lspyx update` | Install the latest release if it's newer. |

Most commands take `--limit` and `--workspace`. Run `lspyx <command> --help` for all options.

## How it works

1. lspyx finds the workspace root: the nearest folder above the file, or the current folder, with a `pyproject.toml`, `.git`, `Cargo.toml` or `package.json`.
2. The first request starts a daemon for that workspace. The daemon runs `ty server` and talks to it over the Language Server Protocol. It adds `src` folders and Python groups under `python/` as import roots, so monorepos resolve too.
3. Later requests from the CLI and the MCP server reuse the daemon over a Unix socket in `~/.cache/lspyx`. The daemon exits after 30 idle minutes.
4. `diagnostics` runs `ruff check` and `ty check` directly, so they read your `pyproject.toml`, `ruff.toml` and `ty.toml` settings.

To use a specific build of ty or Ruff, set `LSPYX_TY_PATH` or `LSPYX_RUFF_PATH`.

## FAQ

**Why not just let the agent use grep?**
Grep finds text. lspyx finds the symbol. It skips comments, strings and other things with the same name, follows imports, and returns only the lines that matter, so the agent reads less and guesses less.

**Does lspyx change my code?**
No. Every tool only reads. `rename` returns a diff for the agent or you to apply.

**Does it support other languages?**
No, only Python. It's built around ty and Ruff.

**Do I need Pyright or Pylance?**
No. lspyx uses ty as its language server.

**Does it work on Windows?**
Not natively, because the daemon uses Unix sockets. WSL may work with the Linux build, but it isn't tested.

**How do I update it?**
Run `lspyx update` for a script install, `brew upgrade lspyx` for Homebrew, or `cargo install lspyx --locked` for Cargo. Running daemons and MCP servers switch to the new version when they restart.

## Uninstall

- **Homebrew:** `brew uninstall lspyx`
- **Cargo:** `cargo uninstall lspyx`
- **Script:** `rm ~/.local/bin/lspyx`

To remove its cache too, run `rm -rf ~/.cache/lspyx`. Remove the server from your agent too, for example with `claude mcp remove lspyx`.

## Build from source

You need [Rust](https://rustup.rs) 1.89 or newer.

```sh
make install   # build and put lspyx in ~/.local/bin
make test
```

Pre-commit hooks run with [prek](https://github.com/j178/prek): `prek install`.

## License

[MIT](LICENSE) © Ihar Yazerski. lspyx includes open-source software under its own licenses: see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). Issues and pull requests are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).

lspyx isn't affiliated with or endorsed by Astral, Anthropic, OpenAI, Cursor or Microsoft. ty, Ruff, Claude, Codex, Cursor and VS Code are trademarks of their owners.
