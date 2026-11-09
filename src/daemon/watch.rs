//! Watch the workspace for the file changes the daemon must forward to ty.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
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

/// What the watcher thread passes on; irrelevant events are dropped there, so the queue only
/// grows with changes ty needs.
#[derive(Debug)]
enum WatchMessage {
    Change { path: PathBuf, created: bool },
    Cookie(PathBuf),
    Failed(String),
}

/// A recursive watcher over the workspace that queues changes until the next request.
#[derive(Debug)]
pub(super) struct WorkspaceWatcher {
    /// Prefix for cookie files in a watched directory outside the workspace.
    cookie_prefix: PathBuf,
    cookies_written: u64,
    // Dropping the watcher stops the events.
    _watcher: RecommendedWatcher,
    messages: Receiver<WatchMessage>,
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

        let (sender, messages) = mpsc::channel();
        let filter = EventFilter {
            workspace_root: workspace_root.to_path_buf(),
            cookie_dir: cookie_dir.clone(),
            sender,
        };
        let mut watcher = notify::recommended_watcher(move |event| filter.forward(event))
            .context("failed to create file watcher")?;
        for (path, mode) in [
            (workspace_root, RecursiveMode::Recursive),
            (cookie_dir.as_path(), RecursiveMode::NonRecursive),
        ] {
            watcher
                .watch(path, mode)
                .with_context(|| format!("failed to watch {}", path.display()))?;
        }
        Ok(Self {
            cookie_prefix,
            cookies_written: 0,
            _watcher: watcher,
            messages,
        })
    }

    /// Collect every change made before this call, with the current change type of each path.
    ///
    /// Paths are Python and config files, or directories that were created, moved, or deleted.
    ///
    /// # Errors
    ///
    /// Returns an error when the watcher failed, dropped events, or stopped reporting.
    pub(super) fn take_changes(&mut self) -> Result<BTreeMap<PathBuf, FileChangeType>> {
        // Once a fresh cookie file shows up, earlier workspace changes have been reported:
        // inotify keeps events in order, and FSEvents does in practice, though it doesn't
        // promise it. Each cookie gets a new name, so leftover events from older cookies
        // can't end the wait early.
        self.cookies_written += 1;
        let mut cookie = self.cookie_prefix.clone().into_os_string();
        cookie.push(format!("-{}", self.cookies_written));
        let cookie = PathBuf::from(cookie);
        fs::write(&cookie, b"").with_context(|| format!("failed to write {}", cookie.display()))?;
        let collected = self.collect_until(&cookie);
        let _ = fs::remove_file(&cookie);
        let (changed, created) = collected?;

        // Events coalesce and arrive late, so the path's current state decides the change type.
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

    /// Read messages until `cookie` shows up, returning the changed and created paths.
    fn collect_until(&self, cookie: &Path) -> Result<(HashSet<PathBuf>, HashSet<PathBuf>)> {
        let deadline = Instant::now() + COOKIE_TIMEOUT;
        let mut changed = HashSet::new();
        let mut created = HashSet::new();
        loop {
            match self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(WatchMessage::Change {
                    path,
                    created: is_creation,
                }) => {
                    if is_creation {
                        created.insert(path.clone());
                    }
                    changed.insert(path);
                }
                Ok(WatchMessage::Cookie(path)) if path == cookie => {
                    return Ok((changed, created));
                }
                Ok(WatchMessage::Cookie(_)) => {}
                Ok(WatchMessage::Failed(error)) => bail!("file watcher failed: {error}"),
                Err(RecvTimeoutError::Timeout) => bail!("file watcher stopped reporting changes"),
                Err(RecvTimeoutError::Disconnected) => bail!("file watcher stopped"),
            }
        }
    }
}

/// Runs on the watcher thread and passes on only the events the daemon needs.
struct EventFilter {
    workspace_root: PathBuf,
    cookie_dir: PathBuf,
    sender: Sender<WatchMessage>,
}

impl EventFilter {
    fn forward(&self, event: notify::Result<Event>) {
        // A closed channel means the daemon is shutting down, so send results are ignored.
        let event = match event {
            Ok(event) if event.need_rescan() => {
                let _ = self
                    .sender
                    .send(WatchMessage::Failed("dropped events".to_string()));
                return;
            }
            Ok(event) => event,
            Err(error) => {
                let _ = self.sender.send(WatchMessage::Failed(error.to_string()));
                return;
            }
        };
        let (created, structural) = match event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => (true, true),
            EventKind::Remove(_) => (false, true),
            EventKind::Modify(_) | EventKind::Any => (false, false),
            EventKind::Access(_) | EventKind::Other => return,
        };
        for path in event.paths {
            let message = if path.parent() == Some(self.cookie_dir.as_path()) {
                WatchMessage::Cookie(path)
            } else if self.is_relevant(&path, structural) {
                WatchMessage::Change { path, created }
            } else {
                continue;
            };
            let _ = self.sender.send(message);
        }
    }

    /// Return whether ty needs to hear about `path`: a Python or config file, or a directory
    /// that appeared or disappeared, outside ignored directories.
    fn is_relevant(&self, path: &Path, structural: bool) -> bool {
        let Ok(relative) = path.strip_prefix(&self.workspace_root) else {
            return false;
        };
        if relative.iter().any(is_ignored_dir_name) {
            return false;
        }
        let is_config = relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| CONFIG_FILE_NAMES.contains(&name));
        // Moving a directory reports only the directory, so ty must rescan it. A deleted
        // path can't be checked, so one without an extension is taken for a directory.
        let is_directory = || path.is_dir() || (!path.exists() && relative.extension().is_none());
        is_python_file(relative) || is_config || (structural && is_directory())
    }
}
