# lspyx

`lspyx` is a CLI and MCP server for Python semantic code navigation and diagnostics. It's built for the higher-signal semantic tasks agents actually do:

- jump to where a symbol comes from with `goto`
- find usages with `usages`
- inspect the symbol and hover details at a position with `inspect`
- summarize file structure with `outline`
- search repo-wide by symbol name with `find-symbol`
- give agents the same semantic context through the `explore` MCP tool
- run repository-configured Ruff and ty checks through `diagnostics`

## Installation

### Prerequisites

- [Rust](https://rust-lang.org/tools/install/)
- [`ty`](https://docs.astral.sh/ty/)
- [`ruff`](https://docs.astral.sh/ruff/)

### Quickstart

Install `lspyx`:

```bash
curl -fsSL https://raw.githubusercontent.com/iyazerski/lspyx/main/install.sh | sh
```

> Windows is not supported by the first installer because the current daemon uses
> Unix sockets and Unix process lifecycle.

### Agent instructions

Add these lines to the `AGENTS.md` / `CLAUDE.md` so agents use LSPYX efficiently:

```md
- For any Python work, use the LSPYX MCP for agent-friendly language navigation and diagnostics. Use `explore` to understand code and `diagnostics` to check changes. Start with the smallest useful file or directory scope.
```

### MCP

Run `lspyx` as a stdio MCP server:

```bash
lspyx mcp serve
```

The MCP server exposes two tools:

| Tool | Purpose |
|------|---------|
| `explore` | Find symbols, outline files, or inspect a position with definition and usages. |
| `diagnostics` | Run repository-configured Ruff and ty checks for a workspace, directory, or file. |
