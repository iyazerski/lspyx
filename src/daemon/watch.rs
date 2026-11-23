use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::lsp::FileChangeType;
use crate::workspace::{is_ignored_dir_name, is_python_file};

const CONFIG_FILE_NAMES: [&str; 2] = ["pyproject.toml", "ty.toml"];
const COOKIE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
enum WatchMessage {
    Change { path: PathBuf, created: bool },
    Cookie(PathBuf),
    Failed(String),
}

#[derive(Debug)]
pub(super) struct WorkspaceWatcher {
    cookie_prefix: PathBuf,
    cookies_written: u64,
    _watcher: RecommendedWatcher,
    messages: Receiver<WatchMessage>,
}

impl WorkspaceWatcher {
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

        // Events coalesce and arrive late, so the path's current state decides the change type.
        Ok(collected?
            .into_iter()
            .map(|(path, created)| {
                let change = if !path.exists() {
                    FileChangeType::Deleted
                } else if created {
                    FileChangeType::Created
                } else {
                    FileChangeType::Changed
                };
                (path, change)
            })
            .collect())
    }

    fn collect_until(&self, cookie: &Path) -> Result<BTreeMap<PathBuf, bool>> {
        let deadline = Instant::now() + COOKIE_TIMEOUT;
        let mut changed = BTreeMap::new();
        loop {
            match self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(WatchMessage::Change { path, created }) => {
                    *changed.entry(path).or_insert(false) |= created;
                }
                Ok(WatchMessage::Cookie(path)) if path == cookie => {
                    return Ok(changed);
                }
                Ok(WatchMessage::Cookie(_)) => {}
                Ok(WatchMessage::Failed(error)) => bail!("file watcher failed: {error}"),
                Err(RecvTimeoutError::Timeout) => bail!("file watcher stopped reporting changes"),
                Err(RecvTimeoutError::Disconnected) => bail!("file watcher stopped"),
            }
        }
    }
}

// Runs on the watcher thread so irrelevant churn never piles up in the channel.
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
