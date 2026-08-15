use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::paths::{resolve_optional_file, resolve_workspace};
use crate::daemon::{self, DaemonRequest};

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenameRequest {
    /// Workspace root; required when file is relative.
    pub(super) workspace: Option<PathBuf>,
    /// Python file containing the symbol to rename.
    pub(super) file: PathBuf,
    /// 1-based line containing the symbol.
    pub(super) line: usize,
    /// 1-based column on the symbol.
    pub(super) column: usize,
    /// New symbol name to preview.
    pub(super) new_name: String,
}

#[derive(Debug)]
pub(super) struct PreparedRename {
    workspace_root: PathBuf,
    file: PathBuf,
    line: usize,
    column: usize,
    new_name: String,
}

pub(super) fn prepare(request: RenameRequest) -> Result<PreparedRename> {
    if request.line == 0 {
        bail!("line must be a 1-based value");
    }
    if request.column == 0 {
        bail!("column must be a 1-based value");
    }
    let new_name = request.new_name.trim();
    if new_name.is_empty() {
        bail!("new_name must not be empty");
    }

    let cwd = env::current_dir().context("failed to determine current directory")?;
    let file = resolve_optional_file(Some(&request.file), request.workspace.as_deref())?
        .context("file is required for rename preview")?;
    let workspace_root = resolve_workspace(request.workspace.as_deref(), Some(&file), &cwd)?;

    Ok(PreparedRename {
        workspace_root,
        file,
        line: request.line,
        column: request.column,
        new_name: new_name.to_string(),
    })
}

pub(super) fn execute(prepared: PreparedRename) -> Result<daemon::DaemonWireResponse> {
    daemon::run_via_daemon_response(
        &prepared.workspace_root,
        DaemonRequest::Rename {
            file: prepared.file,
            line: prepared.line,
            column: prepared.column,
            new_name: prepared.new_name,
        },
    )
}
