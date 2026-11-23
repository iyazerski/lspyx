use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::resolve_file;
use crate::cli::SymbolKindFilter;
use crate::daemon::{self, DaemonRequest};
use crate::file_position::FilePosition;
use crate::lsp::model::OutlineDepth;
use crate::workspace::resolve_workspace_root;

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ExploreRequest {
    /// Symbol text for workspace search; required when file is omitted.
    pub(super) query: Option<String>,
    /// Workspace root for query-only searches and relative file paths.
    pub(super) workspace: Option<PathBuf>,
    /// Python file to outline or inspect. Relative paths require workspace.
    pub(super) file: Option<PathBuf>,
    /// 1-based line for exact position inspection; use with column.
    pub(super) line: Option<usize>,
    /// 1-based column for exact position inspection; use with line.
    pub(super) column: Option<usize>,
    /// Maximum symbols, top-level outline entries, definitions, or usages to return.
    pub(super) limit: Option<usize>,
    /// Symbol kind filter for query-only searches.
    pub(super) kind: Option<SymbolKindFilter>,
    /// Outline nesting depth. Omit for the default depth; use full for the complete tree.
    pub(super) depth: Option<usize>,
    /// Return the complete outline tree; cannot be combined with depth.
    #[serde(default)]
    pub(super) full: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ExploreTarget {
    WorkspaceSymbols {
        query: String,
        kind: Option<SymbolKindFilter>,
    },
    Outline {
        file: PathBuf,
        depth: OutlineDepth,
    },
    Position(FilePosition),
}

#[derive(Debug)]
pub(super) struct PreparedExplore {
    workspace_root: PathBuf,
    pub(super) limit: Option<usize>,
    pub(super) target: ExploreTarget,
}

pub(super) fn prepare(request: ExploreRequest) -> Result<PreparedExplore> {
    if request.limit == Some(0) {
        bail!("limit must be greater than 0");
    }
    let has_outline_options = request.depth.is_some() || request.full;
    let workspace = request.workspace.as_deref();

    let target = match (request.file, request.line, request.column) {
        (None, None, None) => {
            if workspace.is_none() {
                bail!("workspace is required when file is omitted");
            }
            if has_outline_options {
                bail!("depth and full are only supported for file outlines");
            }
            let query = request
                .query
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .context("query is required when file is omitted")?;
            ExploreTarget::WorkspaceSymbols {
                query: query.to_string(),
                kind: request.kind,
            }
        }
        (Some(file), None, None) => {
            if request.kind.is_some() {
                bail!("kind is only supported for workspace symbol search");
            }
            if request.depth.is_some() && request.full {
                bail!("depth cannot be combined with full");
            }
            let depth = if request.full {
                OutlineDepth::Full
            } else {
                request
                    .depth
                    .map_or_else(OutlineDepth::default, OutlineDepth::Limited)
            };
            ExploreTarget::Outline {
                file: resolve_file(&file, workspace)?,
                depth,
            }
        }
        (Some(file), Some(line), Some(column)) => {
            let mut position = FilePosition::new(file, line, column)?;
            if request.kind.is_some() {
                bail!("kind is only supported for workspace symbol search");
            }
            if has_outline_options {
                bail!("depth and full are only supported for file outlines");
            }
            position.file = resolve_file(&position.file, workspace)?;
            ExploreTarget::Position(position)
        }
        (None, _, _) => bail!("line and column require file"),
        (Some(_), _, _) => bail!("line and column must be provided together"),
    };

    let file = match &target {
        ExploreTarget::Outline { file, .. }
        | ExploreTarget::Position(FilePosition { file, .. }) => Some(file.as_path()),
        ExploreTarget::WorkspaceSymbols { .. } => None,
    };
    let cwd = env::current_dir().context("failed to determine current directory")?;
    let workspace_root = resolve_workspace_root(workspace, file, &cwd)?;

    Ok(PreparedExplore {
        workspace_root,
        limit: request.limit,
        target,
    })
}

pub(super) fn execute(prepared: PreparedExplore) -> Result<String> {
    let limit = prepared.limit;
    let request = match prepared.target {
        ExploreTarget::WorkspaceSymbols { query, kind } => {
            DaemonRequest::FindSymbol { query, kind, limit }
        }
        ExploreTarget::Outline { file, depth } => DaemonRequest::Outline { file, depth, limit },
        ExploreTarget::Position(position) => DaemonRequest::Explore { position, limit },
    };
    daemon::run_via_daemon(&prepared.workspace_root, request)
}
