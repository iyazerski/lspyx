use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::resolve_file;
use crate::cli::{SymbolKindFilter, outline_depth};
use crate::daemon::{self, DaemonRequest};
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

/// The validated exploration to run, selected by which request fields are present.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum ExploreTarget {
    WorkspaceSymbols {
        query: String,
        kind: Option<SymbolKindFilter>,
    },
    Outline {
        file: PathBuf,
        depth: Option<usize>,
    },
    Position {
        file: PathBuf,
        line: usize,
        column: usize,
    },
}

#[derive(Debug)]
pub(super) struct PreparedExplore {
    workspace_root: PathBuf,
    pub(super) limit: Option<usize>,
    pub(super) target: ExploreTarget,
}

pub(super) fn prepare(request: ExploreRequest) -> Result<PreparedExplore> {
    let mut target = select_target(&request)?;
    let workspace = request.workspace.as_deref();
    let file = match &mut target {
        ExploreTarget::Outline { file, .. } | ExploreTarget::Position { file, .. } => {
            *file = resolve_file(file, workspace)?;
            Some(file.as_path())
        }
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
        ExploreTarget::Position { file, line, column } => DaemonRequest::Explore {
            file,
            line,
            column,
            limit,
        },
    };
    daemon::run_via_daemon(&prepared.workspace_root, request)
}

/// Validate option combinations and pick the exploration target without touching the filesystem.
pub(super) fn select_target(request: &ExploreRequest) -> Result<ExploreTarget> {
    if request.limit == Some(0) {
        bail!("limit must be greater than 0");
    }
    let has_outline_options = request.depth.is_some() || request.full;

    match (&request.file, request.line, request.column) {
        (None, None, None) => {
            if request.workspace.is_none() {
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
            Ok(ExploreTarget::WorkspaceSymbols {
                query: query.to_string(),
                kind: request.kind,
            })
        }
        (Some(file), None, None) => {
            reject_kind(request)?;
            if request.depth.is_some() && request.full {
                bail!("depth cannot be combined with full");
            }
            Ok(ExploreTarget::Outline {
                file: file.clone(),
                depth: outline_depth(request.depth, request.full),
            })
        }
        (Some(file), Some(line), Some(column)) => {
            if line == 0 {
                bail!("line must be a 1-based value");
            }
            if column == 0 {
                bail!("column must be a 1-based value");
            }
            reject_kind(request)?;
            if has_outline_options {
                bail!("depth and full are only supported for file outlines");
            }
            Ok(ExploreTarget::Position {
                file: file.clone(),
                line,
                column,
            })
        }
        (None, _, _) => bail!("line and column require file"),
        (Some(_), _, _) => bail!("line and column must be provided together"),
    }
}

fn reject_kind(request: &ExploreRequest) -> Result<()> {
    if request.kind.is_some() {
        bail!("kind is only supported for workspace symbol search");
    }
    Ok(())
}
