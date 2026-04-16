---
name: lspyx
description: "Use `lspyx` CLI for semantic Python code navigation"
---

# Lspyx

`lspyx` is a CLI for Python semantic code navigation through LSP.

## Workflow

1. Start the daemon with `lspyx daemon ensure` before running any semantic commands.
2. If you only know a name, start with `find-symbol <query>`.
3. Then run the narrow semantic command that answers the question.
4. Fall back to `rg` only when `lspyx` is unavailable, unsupported, or the task is not semantic navigation.

## Rules

- If you are targeting a different repo than the current working directory, pass `--workspace /abs/path/to/repo`.
- Use `--limit N` to cap the number of results returned.
- Use `outline --depth N` for structure and `outline --full` only when the complete symbol tree matters.
- Keep queries narrow: resolve the symbol first, then inspect exact locations.
