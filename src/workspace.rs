use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{self, DirEntry};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub(crate) fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to resolve path {}", path.display()))
}

pub(crate) fn detect_workspace_root(file: Option<&Path>, cwd: &Path) -> PathBuf {
    let seed = file.and_then(Path::parent).unwrap_or(cwd);

    seed.ancestors()
        .find(|candidate| {
            ["pyproject.toml", ".git", "Cargo.toml", "package.json"]
                .iter()
                .any(|marker| candidate.join(marker).exists())
        })
        .unwrap_or(seed)
        .to_path_buf()
}

pub(crate) fn resolve_workspace_root(
    workspace_override: Option<&Path>,
    file: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf> {
    match workspace_override {
        Some(path) => canonicalize_path(path),
        None => canonicalize_path(&detect_workspace_root(file, cwd)),
    }
}

pub(crate) fn resolve_workspace_root_for_target(target: &Path, cwd: &Path) -> Result<PathBuf> {
    let seed = if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(cwd)
    };

    canonicalize_path(&detect_workspace_root(None, seed))
}

/// Return the lspyx cache directory, or `None` when HOME is unset.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache").join("lspyx"))
}

/// Return a stable per-workspace key used to name cache entries.
pub(crate) fn workspace_cache_key(workspace_root: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
    missing_message: &'static str,
) -> Result<PathBuf> {
    // Allow an explicit override for debugging or alternate installs.
    if let Some(path) = env::var_os(override_variable).map(PathBuf::from)
        && path.is_file()
    {
        return Ok(path);
    }

    // Prefer the workspace virtualenv to match the active project environment.
    let local_binary = workspace_root.join(".venv").join("bin").join(name);
    if local_binary.is_file() {
        return Ok(local_binary);
    }

    // Fall back to a PATH lookup for globally installed toolchains.
    which::which(name).context(missing_message)
}

pub(crate) fn ty_server_configuration(workspace_root: &Path) -> Result<Value> {
    Ok(json!({
        "environment": {
            "root": discover_ty_roots(workspace_root)?,
        }
    }))
}

fn discover_ty_roots(workspace_root: &Path) -> Result<Vec<String>> {
    let mut roots = BTreeSet::from([workspace_root.to_path_buf()]);

    // Add `src` trees, including nested ones used by monorepos with many Python packages.
    collect_named_directories(workspace_root, "src", &mut roots)?;

    let python_root = workspace_root.join("python");
    if python_root.is_dir() {
        for entry in read_dir(&python_root)? {
            let entry = entry?;
            let path = entry.path();
            if !is_walkable_dir(&entry)? || is_python_package_dir(&path) {
                continue;
            }

            // Include namespace/grouping directories like `python/examples`.
            if contains_python_project_hint(&path)? {
                roots.insert(path);
            }
        }
        roots.insert(python_root);
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
    for entry in read_dir(current)? {
        let entry = entry?;
        if !is_walkable_dir(&entry)? {
            continue;
        }

        let path = entry.path();
        collect_named_directories(&path, target_name, roots)?;
        if entry.file_name() == target_name {
            roots.insert(path);
        }
    }

    Ok(())
}

fn contains_python_project_hint(path: &Path) -> Result<bool> {
    if path.join("pyproject.toml").is_file() || path.join("src").is_dir() {
        return Ok(true);
    }

    for entry in read_dir(path)? {
        let entry = entry?;
        let child = entry.path();

        if entry.file_type()?.is_file() {
            if is_python_file(&child) {
                return Ok(true);
            }
        } else if is_walkable_dir(&entry)?
            && (is_python_package_dir(&child) || child.join("src").is_dir())
        {
            return Ok(true);
        }
    }

    Ok(false)
}

/// List a directory with a path-aware error message.
pub(crate) fn read_dir(path: &Path) -> Result<fs::ReadDir> {
    fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))
}

/// Return whether a directory entry is a real (non-symlink) directory worth scanning.
pub(crate) fn is_walkable_dir(entry: &DirEntry) -> Result<bool> {
    // `file_type` does not follow symlinks, which avoids an extra stat and symlink cycles.
    Ok(entry.file_type()?.is_dir()
        && !matches!(
            entry.file_name().to_str(),
            Some(
                ".git"
                    | ".hg"
                    | ".mypy_cache"
                    | ".pytest_cache"
                    | ".ruff_cache"
                    | ".tox"
                    | ".venv"
                    | "__pycache__"
                    | "node_modules"
                    | "target"
            )
        ))
}

pub(crate) fn is_python_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("py" | "pyi")
    )
}

fn is_python_package_dir(path: &Path) -> bool {
    path.join("__init__.py").is_file() || path.join("__init__.pyi").is_file()
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

    use super::{detect_workspace_root, discover_ty_roots};

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

    #[test]
    fn ty_roots_include_src_trees_and_python_groups() {
        let base = unique_temp_dir("lspyx-roots");
        for dir in [
            "src",
            "packages/api/src",
            "python/examples",
            "python/pkg",
            ".venv/lib/src",
        ] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        fs::write(base.join("python/examples/demo.py"), "").unwrap();
        fs::write(base.join("python/pkg/__init__.py"), "").unwrap();

        let roots = discover_ty_roots(&base).unwrap();
        assert_eq!(
            roots,
            vec![
                ".",
                "./packages/api/src",
                "./python",
                "./python/examples",
                "./src",
            ]
        );

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
