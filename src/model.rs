use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::GotoTarget;

#[derive(Debug, Serialize)]
pub(crate) struct LocationOutput {
    pub(crate) ok: bool,
    pub(crate) command: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) position: ResolvedPosition,
    pub(crate) target: Option<GotoTarget>,
    pub(crate) locations: Vec<LocationRecord>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSymbolOutput {
    pub(crate) ok: bool,
    pub(crate) command: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) query: String,
    pub(crate) symbols: Vec<WorkspaceSymbolRecord>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutlineOutput {
    pub(crate) ok: bool,
    pub(crate) command: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) file: PathBuf,
    pub(crate) depth: Option<usize>,
    pub(crate) symbols: Vec<DocumentSymbolNode>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SymbolAtOutput {
    pub(crate) ok: bool,
    pub(crate) command: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) position: ResolvedPosition,
    pub(crate) symbol: Option<SymbolAtRecord>,
    pub(crate) hover: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedPosition {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) requested_column: usize,
    pub(crate) resolved_column: Option<usize>,
    pub(crate) source_line: Option<String>,
    pub(crate) symbol: Option<SymbolAtRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SymbolAtRecord {
    pub(crate) name: String,
    pub(crate) start_column: usize,
    pub(crate) end_column: usize,
    pub(crate) kind: Option<u64>,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PositionRecord {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RangeRecord {
    pub(crate) start: PositionRecord,
    pub(crate) end: PositionRecord,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LocationRecord {
    pub(crate) file: PathBuf,
    pub(crate) range: RangeRecord,
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DocumentSymbolNode {
    pub(crate) name: String,
    pub(crate) detail: Option<String>,
    pub(crate) kind: u64,
    pub(crate) range: RangeRecord,
    pub(crate) selection_range: RangeRecord,
    pub(crate) children: Vec<DocumentSymbolNode>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkspaceSymbolRecord {
    pub(crate) name: String,
    pub(crate) kind: u64,
    pub(crate) container_name: Option<String>,
    pub(crate) file: PathBuf,
    pub(crate) range: RangeRecord,
    pub(crate) snippet: Option<String>,
}

pub(crate) fn display_path(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .ok()
        .map(|relative| {
            if relative.as_os_str().is_empty() {
                ".".to_string()
            } else {
                relative.display().to_string()
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

/// Map an LSP SymbolKind integer to a human-readable label.
pub(crate) fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum-member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-parameter",
        _ => "unknown",
    }
}
