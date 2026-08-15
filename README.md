# lspyx

`lspyx` is a CLI and MCP server for Python semantic code navigation and diagnostics.

Use CLI to:
- jump to where a symbol comes from with `goto`
- find usages with `usages`
- inspect the symbol and hover details at a position with `inspect`
- summarize file structure with `outline`
- search repo-wide by symbol name with `find-symbol`

The MCP server exposes two tools:

| Tool | Purpose |
|------|---------|
| `explore` | Find symbols, outline files, or inspect a position with definition and usages. |
| `diagnostics` | Run repository-configured Ruff and ty checks for a workspace, directory, or file. |

## Installation

> Windows is not supported.

Install `lspyx`:

```bash
curl -fsSL https://raw.githubusercontent.com/iyazerski/lspyx/main/install.sh | sh
```

Add this line to the `AGENTS.md` / `CLAUDE.md` so agents use LSPYX efficiently:

```md
- Use lspyx MCP for Python code navigation and diagnostics. Use `explore` to understand code and `diagnostics` to check changes.
```
