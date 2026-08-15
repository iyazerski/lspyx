use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rmcp::{
    ErrorData,
    model::{CallToolResult, ContentBlock},
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::diagnostics::{DEFAULT_DIAGNOSTIC_LIMIT, render_diagnostics, run_diagnostics};
use crate::workspace::{canonicalize_path, resolve_workspace_root_for_target};

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct DiagnosticsRequest {
    /// Workspace root; required when path is omitted or relative.
    pub(super) workspace: Option<PathBuf>,
    /// File or directory to diagnose; omit to diagnose the workspace.
    pub(super) path: Option<PathBuf>,
    /// Maximum diagnostics to return; defaults to 100.
    pub(super) limit: Option<usize>,
}

#[derive(Debug)]
pub(super) struct PreparedDiagnostics {
    workspace_root: PathBuf,
    target: PathBuf,
    limit: usize,
}

pub(super) fn prepare(request: DiagnosticsRequest) -> Result<PreparedDiagnostics> {
    if request.limit == Some(0) {
        bail!("limit must be greater than 0");
    }

    let cwd = env::current_dir().context("failed to determine current directory")?;
    let explicit_workspace = request
        .workspace
        .as_deref()
        .map(canonicalize_path)
        .transpose()?;
    if explicit_workspace
        .as_deref()
        .is_some_and(|workspace| !workspace.is_dir())
    {
        bail!("workspace must be a directory");
    }

    let target = match request.path.as_deref() {
        Some(path) if path.is_absolute() => canonicalize_path(path)?,
        Some(path) => {
            let workspace = explicit_workspace
                .as_deref()
                .context("workspace is required when path is relative")?;
            canonicalize_path(&workspace.join(path))?
        }
        None => explicit_workspace
            .clone()
            .context("workspace is required when path is omitted")?,
    };
    let workspace_root = match explicit_workspace {
        Some(workspace) => workspace,
        None => resolve_workspace_root_for_target(&target, &cwd)?,
    };

    if !target.starts_with(&workspace_root) {
        bail!(
            "path {} is outside workspace {}",
            target.display(),
            workspace_root.display()
        );
    }

    Ok(PreparedDiagnostics {
        workspace_root,
        target,
        limit: request.limit.unwrap_or(DEFAULT_DIAGNOSTIC_LIMIT),
    })
}

pub(super) fn execute(prepared: PreparedDiagnostics) -> Result<CallToolResult, ErrorData> {
    let output = run_diagnostics(&prepared.workspace_root, &prepared.target, prepared.limit);
    let rendered = render_diagnostics(&output);
    let structured = serde_json::to_value(&output).map_err(|error| {
        ErrorData::internal_error(format!("failed to serialize diagnostics: {error}"), None)
    })?;
    let mut result = if output.all_tools_failed() {
        CallToolResult::error(vec![ContentBlock::text(rendered)])
    } else {
        CallToolResult::success(vec![ContentBlock::text(rendered)])
    };
    result.structured_content = Some(structured);
    Ok(result)
}
