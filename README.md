# lspyx

`lspyx` is a CLI and MCP server for Python semantic code navigation through Language Server Protocol servers.

`lspyx` supports Python through [`ty`](https://docs.astral.sh/ty/).

## Why use it

`lspyx` is built for the higher-signal semantic tasks agents actually do:

- jump to where a symbol comes from with `goto`
- find usages with `usages`
- inspect the symbol and hover details at a position with `inspect`
- summarize file structure with `outline`
- search repo-wide by symbol name with `find-symbol`
- give agents the same semantic context through one MCP tool, `lspyx_explore`

## Installation

### Prerequisites

- [Rust](https://rust-lang.org/tools/install/)
- (Optional) [`uv`](https://docs.astral.sh/uv/getting-started/installation/)

### Quickstart

Install `lspyx`:

```bash
curl -fsSL https://raw.githubusercontent.com/iyazerski/lspyx/main/install.sh | sh
```

> Windows is not supported by the first installer because the current daemon uses
> Unix sockets and Unix process lifecycle.

## Ty adapter

The built-in adapter looks for `ty` in this order:

1. `LSPYX_TY_PATH`
2. `<workspace>/.venv/bin/ty`
3. `ty` on `PATH`

The workspace root is inferred from the target file or current directory by
walking upward for `pyproject.toml`, `.git`, `Cargo.toml`, or `package.json`.
Omit `--workspace` by default; use it only to force a different repo root.

## MCP mode

Run `lspyx` as a stdio MCP server:

```bash
lspyx mcp serve
```

The MCP server exposes one listed tool:

| Tool | Purpose |
|------|---------|
| `lspyx_explore` | Search workspace symbols, outline files, or inspect exact positions with hover details, definition, and usages. |

`lspyx_explore` accepts optional `query`, `workspace`, `file`, `line`,
`column`, `limit`, `kind`, `depth`, and `full` fields. Omit `file` and provide
`workspace` plus `query` to search workspace symbols; `kind` can restrict that
search to classes, functions, or methods. Provide `file` without a position to
outline a file; `depth` controls nesting and `full` returns the complete tree.
Provide `file`, `line`, and `column` together to inspect an exact symbol with
hover details, definition, and usages; `limit` bounds result lists. Relative
file paths require `workspace`; absolute file paths infer the workspace.

## Daemon mode

`lspyx` uses a persistent background `ty` session per workspace through a
Unix socket under `~/.cache/lspyx/`.

- Normal semantic commands start the daemon when needed
- `lspyx daemon ensure` still starts it explicitly when you want to prewarm it
- `lspyx daemon status` reports whether it is running
- `lspyx daemon stop` asks it to exit cleanly

## Development

Install git hooks:

```bash
pre-commit install
```

Run tests:

```bash
make test
```
