use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::lsp::parse::LspPosition;

/// Nesting depth shown when an outline request does not set one.
pub(crate) const DEFAULT_OUTLINE_DEPTH: usize = 2;

/// An LSP `SymbolKind` value; displays as a human-readable label.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SymbolKind(u64);

impl SymbolKind {
    pub(crate) const CLASS: Self = Self(5);
    pub(crate) const METHOD: Self = Self(6);
    pub(crate) const FUNCTION: Self = Self(12);
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Labels follow the numbering in the LSP specification.
        formatter.write_str(match self.0 {
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
        })
    }
}

/// How much of a document symbol tree an outline shows.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutlineDepth {
    /// Show symbols nested at most this many levels deep.
    Limited(usize),
    /// Show the complete tree.
    Full,
}

impl Default for OutlineDepth {
    fn default() -> Self {
        Self::Limited(DEFAULT_OUTLINE_DEPTH)
    }
}

impl fmt::Display for OutlineDepth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limited(depth) => write!(formatter, "{depth}"),
            Self::Full => formatter.write_str("full"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct LocationOutput {
    pub(crate) workspace_root: PathBuf,
    pub(crate) position: ResolvedPosition,
    pub(crate) target: Option<GotoTarget>,
    pub(crate) locations: Vec<LocationRecord>,
}

#[derive(Debug)]
pub(crate) struct WorkspaceSymbolOutput {
    pub(crate) workspace_root: PathBuf,
    pub(crate) query: String,
    pub(crate) kind: Option<SymbolKindFilter>,
    /// Number of matching symbols before the limit was applied.
    pub(crate) total: usize,
    pub(crate) symbols: Vec<WorkspaceSymbolRecord>,
}

#[derive(Debug)]
pub(crate) struct OutlineOutput {
    pub(crate) workspace_root: PathBuf,
    pub(crate) file: PathBuf,
    pub(crate) depth: OutlineDepth,
    pub(crate) symbols: Vec<DocumentSymbolNode>,
}

#[derive(Debug)]
pub(crate) struct SymbolAtOutput {
    pub(crate) workspace_root: PathBuf,
    pub(crate) position: ResolvedPosition,
    pub(crate) hover: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RenameOutput {
    pub(crate) workspace_root: PathBuf,
    pub(crate) position: ResolvedPosition,
    pub(crate) old_name: String,
    pub(crate) new_name: String,
    pub(crate) total: usize,
    pub(crate) total_files: usize,
    pub(crate) edits: Vec<RenameEditRecord>,
    #[serde(skip)]
    pub(crate) diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedPosition {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) requested_column: usize,
    pub(crate) resolved_column: Option<usize>,
    pub(crate) source_line: String,
    pub(crate) symbol: Option<SymbolAtRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SymbolAtRecord {
    pub(crate) name: String,
    pub(crate) start_column: usize,
    pub(crate) end_column: usize,
    pub(crate) kind: Option<SymbolKind>,
    pub(crate) detail: Option<String>,
}

/// A 1-based position; deserializes from a 0-based LSP position.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(from = "LspPosition")]
pub(crate) struct PositionRecord {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct RangeRecord {
    pub(crate) start: PositionRecord,
    pub(crate) end: PositionRecord,
}

impl RangeRecord {
    /// Return whether `position` lies within this range, inclusive of both ends.
    pub(crate) fn contains(&self, position: PositionRecord) -> bool {
        self.start <= position && position <= self.end
    }
}

#[derive(Debug)]
pub(crate) struct LocationRecord {
    pub(crate) file: PathBuf,
    pub(crate) range: RangeRecord,
    pub(crate) snippet: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RenameEditRecord {
    pub(crate) file: PathBuf,
    pub(crate) range: RangeRecord,
    pub(crate) new_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentSymbolNode {
    pub(crate) name: String,
    pub(crate) detail: Option<String>,
    pub(crate) kind: SymbolKind,
    pub(crate) range: RangeRecord,
    pub(crate) selection_range: RangeRecord,
    #[serde(default)]
    pub(crate) children: Vec<DocumentSymbolNode>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceSymbolRecord {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    pub(crate) container_name: Option<String>,
    pub(crate) file: PathBuf,
    pub(crate) range: RangeRecord,
    pub(crate) snippet: Option<String>,
}

/// Return `path` relative to `workspace_root` for display, or unchanged when outside it.
pub(crate) fn display_path(workspace_root: &Path, path: &Path) -> String {
    match path.strip_prefix(workspace_root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}
