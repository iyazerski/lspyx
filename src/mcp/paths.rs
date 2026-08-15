use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::workspace::{canonicalize_path, resolve_workspace_root};

pub(super) fn resolve_optional_file(
    file: Option<&Path>,
    workspace: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let Some(file) = file else {
        return Ok(None);
    };

    let candidate = if file.is_absolute() {
        file.to_path_buf()
    } else if let Some(workspace) = workspace {
        workspace.join(file)
    } else {
        bail!("workspace is required when file is relative");
    };

    Ok(Some(canonicalize_path(&candidate)?))
}

pub(super) fn resolve_workspace(
    workspace: Option<&Path>,
    file: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf> {
    if workspace.is_none() && file.is_none() {
        bail!("workspace is required when file is omitted");
    }

    resolve_workspace_root(workspace, file, cwd)
}
