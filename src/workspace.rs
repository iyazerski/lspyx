use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
pub(crate) struct AdapterStatus {
    pub(crate) ty: BinaryStatus,
    pub(crate) ruff: BinaryStatus,
}

#[derive(Debug, Serialize)]
pub(crate) struct BinaryStatus {
    pub(crate) found: bool,
    pub(crate) path: Option<String>,
}

pub(crate) fn adapter_status(workspace_root: &Path) -> AdapterStatus {
    let ty_path = locate_ty_binary(workspace_root).ok();
    let ruff_path = locate_ruff_binary(workspace_root).ok();

    AdapterStatus {
        ty: BinaryStatus {
            found: ty_path.is_some(),
            path: ty_path.map(|path| path.display().to_string()),
        },
        ruff: BinaryStatus {
            found: ruff_path.is_some(),
            path: ruff_path.map(|path| path.display().to_string()),
        },
    }
}

pub(crate) fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to resolve path {}", path.display()))
}

pub(crate) fn detect_workspace_root(file: Option<&Path>, cwd: &Path) -> PathBuf {
    let seed = file
        .and_then(|value| value.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf());

    for candidate in seed.ancestors() {
        for marker in ["pyproject.toml", ".git", "Cargo.toml", "package.json"] {
            if candidate.join(marker).exists() {
                return candidate.to_path_buf();
            }
        }
    }

    seed
}

pub(crate) fn resolve_workspace_root(
    workspace_override: Option<&Path>,
    file: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf> {
    if let Some(path) = workspace_override {
        return canonicalize_path(path);
    }

    canonicalize_path(&detect_workspace_root(file, cwd))
}

pub(crate) fn resolve_workspace_root_for_target(target: &Path, cwd: &Path) -> Result<PathBuf> {
    let seed = if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(cwd)
    };

    canonicalize_path(&detect_workspace_root(None, seed))
}

pub(crate) fn locate_ty_binary(workspace_root: &Path) -> Result<PathBuf> {
    locate_workspace_binary(
        workspace_root,
        "ty",
        "LSPYX_TY_PATH",
        "unable to find ty; set LSPYX_TY_PATH, create .venv/bin/ty, or install ty on PATH",
    )
}

pub(crate) fn locate_ruff_binary(workspace_root: &Path) -> Result<PathBuf> {
    locate_workspace_binary(
        workspace_root,
        "ruff",
        "LSPYX_RUFF_PATH",
        "unable to find ruff; set LSPYX_RUFF_PATH, create .venv/bin/ruff, or install ruff on PATH",
    )
}

fn locate_workspace_binary(
    workspace_root: &Path,
    name: &str,
    override_variable: &str,
    missing_message: &str,
) -> Result<PathBuf> {
    // Allow an explicit override for debugging or alternate installs.
    if let Ok(path) = env::var(override_variable) {
        let resolved = PathBuf::from(path);
        if resolved.is_file() {
            return Ok(resolved);
        }
    }

    // Prefer the workspace virtualenv to match the active project environment.
    let local_binary = workspace_root.join(".venv").join("bin").join(name);
    if local_binary.is_file() {
        return Ok(local_binary);
    }

    // Fall back to a PATH lookup for globally installed toolchains.
    which::which(name).context(missing_message.to_string())
}

pub(crate) fn ty_server_configuration(workspace_root: &Path) -> Result<Value> {
    let roots = discover_ty_roots(workspace_root)?;

    Ok(json!({
        "environment": {
            "root": roots,
        }
    }))
}

fn discover_ty_roots(workspace_root: &Path) -> Result<Vec<String>> {
    let mut roots = BTreeSet::new();
    roots.insert(workspace_root.to_path_buf());

    // Add the common single-package layouts first.
    for relative in ["src", "python"] {
        let candidate = workspace_root.join(relative);
        if candidate.is_dir() {
            roots.insert(candidate);
        }
    }

    // Add nested `src` trees used by monorepos with many Python packages.
    collect_named_directories(workspace_root, "src", &mut roots)?;

    let python_root = workspace_root.join("python");
    if python_root.is_dir() {
        for entry in fs::read_dir(&python_root)
            .with_context(|| format!("failed to read {}", python_root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || is_ignored_directory(&path) || is_python_package_dir(&path) {
                continue;
            }

            // Include namespace/grouping directories like `python/examples`.
            if contains_python_project_hint(&path)? {
                roots.insert(path);
            }
        }
    }

    Ok(roots
        .into_iter()
        .map(|path| relative_root_string(workspace_root, &path))
        .collect())
}

fn collect_named_directories(
    current: &Path,
    target_name: &str,
    roots: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if is_ignored_directory(current) {
        return Ok(());
    }

    for entry in
        fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() || is_ignored_directory(&path) {
            continue;
        }

        if path.file_name().and_then(|value| value.to_str()) == Some(target_name) {
            roots.insert(path.clone());
        }

        collect_named_directories(&path, target_name, roots)?;
    }

    Ok(())
}

fn contains_python_project_hint(path: &Path) -> Result<bool> {
    if path.join("pyproject.toml").is_file() || path.join("src").is_dir() {
        return Ok(true);
    }

    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let child = entry.path();

        if child.is_file() {
            if matches!(
                child.extension().and_then(|value| value.to_str()),
                Some("py") | Some("pyi")
            ) {
                return Ok(true);
            }
            continue;
        }

        if !child.is_dir() || is_ignored_directory(&child) {
            continue;
        }

        if is_python_package_dir(&child) || child.join("src").is_dir() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn is_python_package_dir(path: &Path) -> bool {
    path.join("__init__.py").is_file() || path.join("__init__.pyi").is_file()
}

fn is_ignored_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some(".git")
            | Some(".hg")
            | Some(".mypy_cache")
            | Some(".pytest_cache")
            | Some(".ruff_cache")
            | Some(".tox")
            | Some(".venv")
            | Some("__pycache__")
            | Some("node_modules")
            | Some("target")
    )
}

fn relative_root_string(workspace_root: &Path, path: &Path) -> String {
    match path.strip_prefix(workspace_root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => format!("./{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::detect_workspace_root;

    #[test]
    fn workspace_root_prefers_pyproject() {
        let base = unique_temp_dir("lspyx-test");
        let nested = base.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(base.join("pyproject.toml"), "[project]\nname='x'\n").unwrap();

        let detected = detect_workspace_root(None, &nested);
        assert_eq!(detected, base);

        fs::remove_dir_all(&base).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{prefix}-{suffix}"))
    }
}
