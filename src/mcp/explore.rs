use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::paths::{resolve_optional_file, resolve_workspace};
use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::daemon::{self, DaemonRequest};

const DEFAULT_OUTLINE_DEPTH: usize = 2;

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

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum ExploreRoute {
    WorkspaceSymbols,
    Outline,
    Position,
}

#[derive(Debug)]
pub(super) struct PreparedExplore {
    query: Option<String>,
    workspace_root: PathBuf,
    file: Option<PathBuf>,
    line: Option<usize>,
    column: Option<usize>,
    pub(super) limit: Option<usize>,
    pub(super) kind: Option<SymbolKindFilter>,
    depth: Option<usize>,
    full: bool,
    pub(super) route: ExploreRoute,
}

pub(super) fn prepare(request: ExploreRequest) -> Result<PreparedExplore> {
    let route = select_route(&request)?;
    validate_route_options(&request, &route)?;
    let query = validate_query(request.query.as_deref(), &route)?;
    let cwd = env::current_dir().context("failed to determine current directory")?;
    let file = resolve_optional_file(request.file.as_deref(), request.workspace.as_deref())?;
    let workspace_root = resolve_workspace(request.workspace.as_deref(), file.as_deref(), &cwd)?;

    Ok(PreparedExplore {
        query,
        workspace_root,
        file,
        line: request.line,
        column: request.column,
        limit: request.limit,
        kind: request.kind,
        depth: request.depth,
        full: request.full,
        route,
    })
}

pub(super) fn execute(prepared: PreparedExplore) -> Result<String> {
    match prepared.route {
        ExploreRoute::WorkspaceSymbols => daemon::run_via_daemon(
            &prepared.workspace_root,
            DaemonRequest::FindSymbol {
                query: prepared
                    .query
                    .context("query is required for workspace symbol search")?,
                kind: prepared.kind,
                limit: prepared.limit,
            },
        ),
        ExploreRoute::Outline => {
            let file = prepared
                .file
                .context("file is required for outline exploration")?;
            daemon::run_via_daemon(
                &prepared.workspace_root,
                DaemonRequest::Outline {
                    file,
                    depth: outline_depth(prepared.depth, prepared.full),
                    limit: prepared.limit,
                },
            )
        }
        ExploreRoute::Position => {
            let file = prepared
                .file
                .context("file is required for position exploration")?;
            let line = prepared
                .line
                .context("line is required for position exploration")?;
            let column = prepared
                .column
                .context("column is required for position exploration")?;
            run_position_bundle(&prepared.workspace_root, file, line, column, prepared.limit)
        }
    }
}

fn run_position_bundle(
    workspace_root: &Path,
    file: PathBuf,
    line: usize,
    column: usize,
    limit: Option<usize>,
) -> Result<String> {
    let inspect = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Inspect {
            file: file.clone(),
            line,
            column,
        },
    )?;
    if inspect_found_no_symbol(inspect.as_str()) {
        return Ok(section("Inspect", inspect));
    }

    let definition = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Goto {
            file: file.clone(),
            line,
            column,
            target: GotoTarget::Definition,
            limit,
        },
    )?;
    let usages = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Usages {
            file,
            line,
            column,
            include_declaration: true,
            limit,
        },
    )?;

    Ok(format!(
        "{}\n\n{}\n\n{}",
        section("Inspect", inspect),
        section("Definition", definition),
        section("Usages", usages),
    ))
}

pub(super) fn inspect_found_no_symbol(inspect: &str) -> bool {
    inspect.starts_with("no symbol found at ")
}

pub(super) fn validate_query(query: Option<&str>, route: &ExploreRoute) -> Result<Option<String>> {
    if route != &ExploreRoute::WorkspaceSymbols {
        return Ok(query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string));
    }

    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        bail!("query is required when file is omitted");
    };
    Ok(Some(query.to_string()))
}

pub(super) fn select_route(request: &ExploreRequest) -> Result<ExploreRoute> {
    match (&request.file, request.line, request.column) {
        (None, None, None) => {
            if request.workspace.is_none() {
                bail!("workspace is required when file is omitted");
            }
            Ok(ExploreRoute::WorkspaceSymbols)
        }
        (Some(_), None, None) => Ok(ExploreRoute::Outline),
        (Some(_), Some(line), Some(column)) => {
            if line == 0 {
                bail!("line must be a 1-based value");
            }
            if column == 0 {
                bail!("column must be a 1-based value");
            }
            Ok(ExploreRoute::Position)
        }
        (None, Some(_), _) | (None, _, Some(_)) => bail!("line and column require file"),
        (Some(_), Some(_), None) | (Some(_), None, Some(_)) => {
            bail!("line and column must be provided together")
        }
    }
}

fn validate_route_options(request: &ExploreRequest, route: &ExploreRoute) -> Result<()> {
    if request.limit == Some(0) {
        bail!("limit must be greater than 0");
    }
    if request.kind.is_some() && route != &ExploreRoute::WorkspaceSymbols {
        bail!("kind is only supported for workspace symbol search");
    }
    if (request.depth.is_some() || request.full) && route != &ExploreRoute::Outline {
        bail!("depth and full are only supported for file outlines");
    }
    if request.depth.is_some() && request.full {
        bail!("depth cannot be combined with full");
    }
    Ok(())
}

pub(super) fn outline_depth(depth: Option<usize>, full: bool) -> Option<usize> {
    if full {
        None
    } else {
        Some(depth.unwrap_or(DEFAULT_OUTLINE_DEPTH))
    }
}

fn section(title: &str, body: String) -> String {
    format!("## {title}\n\n{body}")
}
