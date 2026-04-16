# lspyx

`lspyx` is a CLI for Python semantic code navigation through Language Server Protocol servers.

`lspyx` supports Python through [`ty`](https://docs.astral.sh/ty/).

## Why use it

`lspyx` is built for the higher-signal semantic tasks agents actually do:

- jump to where a symbol comes from with `goto`
- find usages with `usages`
- inspect the symbol and hover details at a position with `inspect`
- summarize file structure with `outline`
- search repo-wide by symbol name with `find-symbol`

## Features

- Read-only semantic navigation
- Terse text output with workspace-relative paths
- Persistent daemon mode with automatic startup for semantic commands

## Installation

### Prerequisites

- [Rust](https://rust-lang.org/tools/install/)
- (Optional) [`uv`](https://docs.astral.sh/uv/getting-started/installation/)

### Quickstart

Install `lspyx`:

```bash
make install
```

This installs the binary to `~/.local/bin/lspyx`.

Verify the install and inspect command help:

```bash
lspyx --help
```

## Ty adapter

The built-in adapter looks for `ty` in this order:

1. `LSPYX_TY_PATH`
2. `<workspace>/.venv/bin/ty`
3. `ty` on `PATH`

The workspace root is inferred from the target file or current directory by
walking upward for `pyproject.toml`, `.git`, `Cargo.toml`, or `package.json`.
Use `--workspace` to force a specific root.

## Daemon mode

`lspyx` uses a persistent background `ty` session per workspace through a
Unix socket under `~/.cache/lspyx/`.

- Normal semantic commands ensure the daemon and run through it
- If the daemon cannot be started or reached, the command fails
- `lspyx daemon ensure` starts it separately when needed
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
