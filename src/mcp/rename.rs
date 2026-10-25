use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::resolve_file;
use crate::daemon::{self, DaemonReply, DaemonRequest};
use crate::file_position::FilePosition;
use crate::workspace::resolve_workspace_root;

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
    position: FilePosition,
    new_name: String,
}

/// Validate the request and resolve its file and workspace root.
pub(super) fn prepare(request: RenameRequest) -> Result<PreparedRename> {
    let mut position = FilePosition::new(request.file, request.line, request.column)?;
    let new_name = request.new_name.trim();
    if new_name.is_empty() {
        bail!("new_name must not be empty");
    }

    let cwd = env::current_dir().context("failed to determine current directory")?;
    position.file = resolve_file(&position.file, request.workspace.as_deref())?;
    let workspace_root =
        resolve_workspace_root(request.workspace.as_deref(), Some(&position.file), &cwd)?;

    Ok(PreparedRename {
        workspace_root,
        position,
        new_name: new_name.to_string(),
    })
}

/// Run the prepared rename preview through the workspace daemon.
pub(super) fn execute(prepared: PreparedRename) -> Result<DaemonReply> {
    daemon::run_via_daemon_reply(
        &prepared.workspace_root,
        DaemonRequest::Rename {
            position: prepared.position,
            new_name: prepared.new_name,
        },
    )
}
