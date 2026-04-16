---
name: lspyx
description: "Use `lspyx` CLI for semantic Python navigation with task-shaped commands: find-symbol, goto, usages, inspect, and outline"
---

# Lspyx

Use `lspyx` for precise Python symbol navigation.

## Workflow

1. If you only know a name, start with `find-symbol <query>`.
2. Then run the narrow semantic command that answers the question.
3. Fall back to `rg` only when `lspyx` is unavailable, unsupported, or the task is not semantic navigation.

## Command Choice

- `find-symbol <query>`: find candidate symbols by name across the workspace.
- `goto <file:line>`: jump to a definition, declaration, or type from a position.
- `usages <file:line>`: find usages from a position.
- `inspect <file:line>`: identify the symbol under a cursor and read hover details.
- `outline <file>`: inspect file structure, either bounded or full.

## Rules

- Positions use `file:line` format (1-based).
- If you are targeting a different repo than the current working directory, pass `--workspace /abs/path/to/repo`.
- Use `--limit N` to cap the number of results returned.
- Use `outline --depth N` for structure and `outline --full` only when the complete symbol tree matters.
- Keep queries narrow: resolve the symbol first, then inspect exact locations.
