<div align="center">

<img src="docs/icon.png" width="112" alt="lspyx icon">

# lspyx

**Python code navigation and diagnostics for AI coding agents.**

Give Claude Code, Codex, Cursor and other agents go to definition, find usages, safe rename previews,<br>
and ty and Ruff checks through one small MCP server. Or use the same answers from your terminal.

[![Latest release](https://img.shields.io/github/v/release/iyazerski/lspyx?color=3b6cf6)](https://github.com/iyazerski/lspyx/releases/latest)
[![crates.io](https://img.shields.io/crates/v/lspyx?color=orange)](https://crates.io/crates/lspyx)
[![macOS · Linux](https://img.shields.io/badge/macOS%20·%20Linux-x86__64%20·%20ARM64-black)](#install)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Buy me a coffee](https://img.shields.io/badge/buy%20me%20a%20coffee-FFDD00?logo=buymeacoffee&logoColor=black)](https://buymeacoffee.com/iyazerski)

<img src="docs/demo.gif" width="720" alt="lspyx finding a class, the usages of a method, a file outline and a definition in a small Python project">

</div>

## Why lspyx

Coding agents explore Python with `grep` and by reading whole files. That's slow, it fills the context window, and text search can't tell `Cart.add` from every other `add`. lspyx answers with the type checker instead.

- **Semantic answers.** Definitions, usages, types and docs come from [ty](https://github.com/astral-sh/ty), Astral's Python type checker. A usage is a real reference, not a matching string.
- **Short output.** Each result is a `file:line:column` with its source line, and `limit` keeps big results short.
- **Both checkers in one call.** `diagnostics` runs [Ruff](https://github.com/astral-sh/ruff) and ty with your project's settings, on a file, a folder or the whole repo.
- **Safe rename previews.** `rename` returns a complete, checked diff for a cross-file rename. It never writes files.
- **Fast.** A daemon per workspace keeps ty warm, so later calls take milliseconds. It stops after 30 idle minutes.
- **No setup.** lspyx uses `ty` and `ruff` from the project's `.venv` first, then from your `PATH`, and finds `src/` layouts and monorepo packages by itself.
- **Local, free and open source.** One Rust binary under the MIT license. It sends nothing over the network, except when you run `lspyx update`.

## Install

For macOS and Linux, on x86_64 and ARM64. Pick one:

- **Homebrew:** `brew install iyazerski/tap/lspyx`
- **Cargo:** `cargo install lspyx --locked`
- **Script:** `curl -fsSL https://raw.githubusercontent.com/iyazerski/lspyx/main/install.sh | sh`

lspyx needs ty and Ruff in your project's `.venv` or on your `PATH`. The script installs them with [uv](https://docs.astral.sh/uv/) if they're missing. Otherwise, run `brew install ty ruff` or `uv tool install ty && uv tool install ruff`. Then run `lspyx doctor` in your project to check the setup.

## Connect your agent

```sh
claude mcp add --scope user lspyx -- lspyx mcp serve   # Claude Code
codex mcp add lspyx -- lspyx mcp serve                 # Codex
pi mcp add lspyx -- lspyx mcp serve                    # pi
```

For Cursor, add this to `~/.cursor/mcp.json`. Other clients use the same command and arguments:

```json
{ "mcpServers": { "lspyx": { "command": "lspyx", "args": ["mcp", "serve"] } } }
```

Then add this line to your `AGENTS.md` or `CLAUDE.md`, so the agent reaches for lspyx instead of `grep`:

```md
- Use lspyx MCP for Python code navigation, diagnostics, and refactors. Use `explore` to understand code, `diagnostics` to check changes, and `rename` to preview cross-file renames.
```

## Tools

| MCP tool | What it does |
| --- | --- |
| `explore` | Search symbols by name, outline a file, or inspect the symbol at a position with its definition and usages. |
| `diagnostics` | Run Ruff and ty with the repo's settings on a file, a folder or the whole repo. |
| `rename` | Preview a semantic rename as a complete unified diff, without changing files. |

The CLI gives the same navigation answers: `lspyx find-symbol`, `outline`, `inspect`, `goto` and `usages`. Positions are `file:line:column`, starting at 1. Run `lspyx --help` for all commands.

## FAQ

<details>
<summary><b>Why not just let the agent use grep?</b></summary>

Grep finds text. lspyx finds the symbol. It skips comments, strings and other things with the same name, follows imports, and returns only the lines that matter, so the agent reads less and guesses less.

</details>
<details>
<summary><b>How does it work?</b></summary>

The first request in a workspace starts a daemon that runs `ty server` and talks to it over the Language Server Protocol. Later requests from the CLI and the MCP server reuse it through a Unix socket in `~/.cache/lspyx`. `diagnostics` runs `ruff check` and `ty check` directly, so they read your `pyproject.toml`, `ruff.toml` and `ty.toml`. To use a specific ty or Ruff, set `LSPYX_TY_PATH` or `LSPYX_RUFF_PATH`.

</details>
<details>
<summary><b>Does lspyx change my code?</b></summary>

No. Every tool only reads. `rename` returns a diff for the agent or you to apply.

</details>
<details>
<summary><b>Does it support other languages, or need Pyright?</b></summary>

No to both. lspyx is for Python only, and it uses ty as its language server.

</details>
<details>
<summary><b>Does it work on Windows?</b></summary>

Not natively, because the daemon uses Unix sockets. WSL may work with the Linux build, but it isn't tested.

</details>
<details>
<summary><b>How do I update or uninstall it?</b></summary>

Update with `lspyx update` for a script install, `brew upgrade lspyx` for Homebrew, or `cargo install lspyx --locked` for Cargo. Running daemons and MCP servers switch to the new version when they restart.

Uninstall with `brew uninstall lspyx`, `cargo uninstall lspyx`, or `rm ~/.local/bin/lspyx`. Then remove the cache with `rm -rf ~/.cache/lspyx`, and remove the server from your agent, for example with `claude mcp remove lspyx`.

</details>

## Build from source

You need [Rust](https://rustup.rs) 1.89 or newer.

```sh
make install   # build and put lspyx in ~/.local/bin
make test
```

Pre-commit hooks run with [prek](https://github.com/j178/prek): `prek install`.

## License

[MIT](LICENSE) © Ihar Yazerski. lspyx includes open-source software under its own licenses: see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). Issues and pull requests are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).

lspyx isn't affiliated with or endorsed by Astral, Anthropic, OpenAI or Cursor. ty, Ruff, Claude, Codex and Cursor are trademarks of their owners.
