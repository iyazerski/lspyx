//! Watch the workspace for the file changes the daemon must forward to ty.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::lsp::FileChangeType;
use crate::workspace::{is_ignored_dir_name, is_python_file};

/// Config files that change how ty resolves or checks the project.
const CONFIG_FILE_NAMES: [&str; 2] = ["pyproject.toml", "ty.toml"];

/// How long to wait for the watcher to report a cookie file; it normally takes milliseconds.
const COOKIE_TIMEOUT: Duration = Duration::from_secs(2);

/// A recursive watcher over the workspace that queues events until the next request.
#[derive(Debug)]
pub(super) struct WorkspaceWatcher {
    workspace_root: PathBuf,
    /// Prefix for cookie files in a watched directory outside the workspace.
    cookie_prefix: PathBuf,
    cookies_written: u64,
    // Dropping the watcher stops the events.
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
}

impl WorkspaceWatcher {
    /// Watch `workspace_root` recursively, and the directory of `cookie_prefix` for cookies.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform watcher cannot be created or attached.
    pub(super) fn new(workspace_root: &Path, cookie_prefix: &Path) -> Result<Self> {
        // Watchers report resolved paths, so cookies must be compared in resolved form.
        let cookie_dir = cookie_prefix
            .parent()
            .context("cookie path has no parent directory")?
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", cookie_prefix.display()))?;
        let cookie_prefix = cookie_dir.join(
            cookie_prefix
                .file_name()
                .context("cookie path has no file name")?,
        );

        let (sender, events) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(sender).context("failed to create file watcher")?;
        for (path, mode) in [
            (workspace_root, RecursiveMode::Recursive),
            (cookie_dir.as_path(), RecursiveMode::NonRecursive),
        ] {
            watcher
                .watch(path, mode)
                .with_context(|| format!("failed to watch {}", path.display()))?;
        }
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            cookie_prefix,
            cookies_written: 0,
            _watcher: watcher,
            events,
        })
    }

    /// Collect every change made before this call, with the current change type of each file.
    ///
    /// # Errors
    ///
    /// Returns an error when the watcher failed, dropped events, or stopped reporting.
    pub(super) fn take_changes(&mut self) -> Result<BTreeMap<PathBuf, FileChangeType>> {
        // Watchers deliver events late but in order, so once a fresh cookie file shows up,
        // every earlier workspace change has been seen. Each cookie gets a new name, so
        // leftover events from older cookies can't end the wait early.
        self.cookies_written += 1;
        let mut cookie = self.cookie_prefix.clone().into_os_string();
        cookie.push(format!("-{}", self.cookies_written));
        let cookie = PathBuf::from(cookie);
        fs::write(&cookie, b"").with_context(|| format!("failed to write {}", cookie.display()))?;
        let collected = self.collect_until(&cookie);
        let _ = fs::remove_file(&cookie);
        let (changed, created) = collected?;

        // Events coalesce and arrive late, so the file's current state decides the change type.
        Ok(changed
            .into_iter()
            .map(|path| {
                let change = if !path.exists() {
                    FileChangeType::Deleted
                } else if created.contains(&path) {
                    FileChangeType::Created
                } else {
                    FileChangeType::Changed
                };
                (path, change)
            })
            .collect())
    }

    /// Read events until one names `cookie`, returning the changed and created relevant files.
    fn collect_until(&self, cookie: &Path) -> Result<(HashSet<PathBuf>, HashSet<PathBuf>)> {
        let deadline = Instant::now() + COOKIE_TIMEOUT;
        let mut changed = HashSet::new();
        let mut created = HashSet::new();
        loop {
            let event = match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(event) => event.context("file watcher failed")?,
                Err(RecvTimeoutError::Timeout) => bail!("file watcher stopped reporting changes"),
                Err(RecvTimeoutError::Disconnected) => bail!("file watcher stopped"),
            };
            if event.need_rescan() {
                bail!("file watcher dropped events");
            }
            if event.paths.iter().any(|path| path == cookie) {
                return Ok((changed, created));
            }
            let is_creation = match event.kind {
                EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => true,
                EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any => false,
                EventKind::Access(_) | EventKind::Other => continue,
            };
            for path in event
                .paths
                .into_iter()
                .filter(|path| is_relevant(&self.workspace_root, path))
            {
                if is_creation {
                    created.insert(path.clone());
                }
                changed.insert(path);
            }
        }
    }
}

/// Return whether ty cares about `path`: Python sources and config outside ignored directories.
fn is_relevant(workspace_root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(workspace_root) else {
        return false;
    };
    let wanted = is_python_file(relative)
        || relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| CONFIG_FILE_NAMES.contains(&name));
    wanted
        && !relative
            .parent()
            .is_some_and(|parent| parent.iter().any(is_ignored_dir_name))
}
