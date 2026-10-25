mod adapter;
mod dispatch;
mod protocol;

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde_json::Value;

use self::adapter::PersistentAdapter;
use self::dispatch::{DispatchResult, dispatch_request};
pub(crate) use self::protocol::{DaemonReply, DaemonRequest, Declaration};
use self::protocol::{DaemonResponse, read_request, write_frame};
use crate::debug;
use crate::workspace::{cache_dir, locate_ty_binary, workspace_cache_key};

const DEFAULT_IDLE_SECONDS: u64 = 1800;
const DAEMON_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(25);
const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const ENSURE_AFTER_HELP: &str = "Example:\n  lspyx daemon ensure --idle-seconds 900";
const SERVE_AFTER_HELP: &str = "Example:\n  lspyx daemon serve --idle-seconds 900";
const STATUS_AFTER_HELP: &str = "Example:\n  lspyx daemon status";
const STOP_AFTER_HELP: &str = "Example:\n  lspyx daemon stop";

#[derive(Args, Debug)]
pub(crate) struct DaemonArgs {
    /// Optional override for a different repo; omit in the current workspace.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: DaemonSubcommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum DaemonSubcommand {
    /// Start the daemon in the background unless it is already running.
    #[command(after_help = ENSURE_AFTER_HELP)]
    Ensure(DaemonLifecycleArgs),
    /// Run the daemon in the foreground.
    #[command(after_help = SERVE_AFTER_HELP)]
    Serve(DaemonLifecycleArgs),
    /// Report whether the daemon is running.
    #[command(after_help = STATUS_AFTER_HELP)]
    Status,
    /// Stop the running daemon.
    #[command(after_help = STOP_AFTER_HELP)]
    Stop,
}

#[derive(Args, Debug)]
pub(crate) struct DaemonLifecycleArgs {
    /// Exit after this many seconds without requests.
    #[arg(long, default_value_t = DEFAULT_IDLE_SECONDS)]
    pub(crate) idle_seconds: u64,
}

#[derive(Debug)]
pub(crate) struct DaemonStatus {
    pub(crate) socket_path: PathBuf,
    pub(crate) pid: Option<u32>,
}

pub(crate) fn run_daemon_command(
    workspace_root: &Path,
    command: DaemonSubcommand,
) -> Result<String> {
    match command {
        DaemonSubcommand::Ensure(lifecycle) => Ok(render_status(&ensure_daemon(
            workspace_root,
            lifecycle.idle_seconds,
        )?)),
        DaemonSubcommand::Serve(lifecycle) => {
            serve_daemon(workspace_root, lifecycle.idle_seconds)?;
            Ok("summary: daemon exited".to_string())
        }
        DaemonSubcommand::Status => Ok(render_status(&daemon_status(workspace_root)?)),
        DaemonSubcommand::Stop => {
            let stopped = stop_daemon(workspace_root)?;
            let summary = if stopped {
                "daemon stopped"
            } else {
                "daemon was not running"
            };
            Ok(format!("summary: {summary}\nstopped: {stopped}"))
        }
    }
}

/// Send `request` to the workspace daemon, starting it if needed, and return the rendered text.
///
/// # Errors
///
/// Returns an error when the daemon cannot be reached or the request fails.
pub(crate) fn run_via_daemon(workspace_root: &Path, request: DaemonRequest) -> Result<String> {
    Ok(run_via_daemon_reply(workspace_root, request)?.text)
}

/// Send `request` to the workspace daemon, starting it if needed, and return the full reply.
///
/// # Errors
///
/// Returns an error when the daemon cannot be reached or the request fails.
pub(crate) fn run_via_daemon_reply(
    workspace_root: &Path,
    request: DaemonRequest,
) -> Result<DaemonReply> {
    let socket_path = socket_path(workspace_root)?;

    // Reuse an already-running daemon directly to avoid an extra ping roundtrip.
    let response = match protocol::send_request(&socket_path, &request)? {
        Some(response) => response,
        None => {
            ensure_daemon(workspace_root, DEFAULT_IDLE_SECONDS)?;
            protocol::send_request(&socket_path, &request)?.ok_or_else(|| {
                anyhow!(
                    "daemon started for workspace {} but did not accept the request",
                    workspace_root.display()
                )
            })?
        }
    };

    response.map_err(|error| anyhow!(error))
}

pub(crate) fn daemon_status(workspace_root: &Path) -> Result<DaemonStatus> {
    let socket_path = socket_path(workspace_root)?;
    let pid = protocol::send_request(&socket_path, &DaemonRequest::Ping)?
        .as_ref()
        .and_then(response_pid);

    Ok(DaemonStatus { socket_path, pid })
}

fn ensure_daemon(workspace_root: &Path, idle_seconds: u64) -> Result<DaemonStatus> {
    // Serialize cold starts so concurrent clients cannot stomp the same socket.
    let _startup_lock = acquire_startup_lock(workspace_root)?;
    let status = daemon_status(workspace_root)?;
    if status.pid.is_some() {
        return Ok(status);
    }

    if let Err(error) = fs::remove_file(&status.socket_path)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error).with_context(|| {
            format!(
                "failed to remove stale socket {}",
                status.socket_path.display()
            )
        });
    }

    spawn_daemon_process(workspace_root, idle_seconds)?;

    let deadline = Instant::now() + DAEMON_STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        let status = daemon_status(workspace_root)?;
        if status.pid.is_some() {
            return Ok(status);
        }
        thread::sleep(DAEMON_STARTUP_POLL_INTERVAL);
    }

    bail!(
        "daemon did not become ready for workspace {}",
        workspace_root.display()
    )
}

/// Start a detached `lspyx daemon serve` process for the workspace.
fn spawn_daemon_process(workspace_root: &Path, idle_seconds: u64) -> Result<()> {
    let current_exe = env::current_exe().context("failed to resolve current lspyx binary")?;
    let mut command = Command::new(current_exe);
    command
        .args(daemon_serve_args(workspace_root, idle_seconds))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(debug::child_stderr());
    // SAFETY: the hook runs in the forked child before exec and only calls `setsid`, which is
    // async-signal-safe and touches no memory shared with the parent.
    unsafe {
        // Start a new session so the daemon outlives the caller and ignores its terminal signals.
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("failed to spawn daemon process")?;

    // Reap the daemon when it exits so a long-lived caller, such as the MCP server, never keeps
    // a zombie. The thread dies with short-lived callers, and the daemon is re-parented.
    thread::spawn(move || child.wait());
    Ok(())
}

fn daemon_serve_args(workspace_root: &Path, idle_seconds: u64) -> Vec<OsString> {
    vec![
        OsString::from("daemon"),
        OsString::from("--workspace"),
        workspace_root.as_os_str().to_os_string(),
        OsString::from("serve"),
        OsString::from("--idle-seconds"),
        OsString::from(idle_seconds.to_string()),
    ]
}

fn stop_daemon(workspace_root: &Path) -> Result<bool> {
    let response = protocol::send_request(&socket_path(workspace_root)?, &DaemonRequest::Shutdown)?;
    Ok(response.is_some())
}

fn render_status(status: &DaemonStatus) -> String {
    let running = status.pid.is_some();
    let summary = if running {
        "daemon running"
    } else {
        "daemon not running"
    };
    let pid = status
        .pid
        .map_or_else(|| "none".to_string(), |pid| pid.to_string());

    format!(
        "summary: {summary}\nrunning: {running}\npid: {pid}\nsocket: {}",
        status.socket_path.display()
    )
}

fn serve_daemon(workspace_root: &Path, idle_seconds: u64) -> Result<()> {
    let socket_path = socket_path(workspace_root)?;
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Refuse to take over a live socket; the startup lock handles stale cleanup before spawn.
    if let Some(response) = protocol::send_request(&socket_path, &DaemonRequest::Ping)? {
        let pid =
            response_pid(&response).map_or_else(|| "unknown".to_string(), |pid| pid.to_string());
        bail!(
            "daemon already running for workspace {} (pid {pid})",
            workspace_root.display()
        );
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind {}", socket_path.display()))?;
    listener
        .set_nonblocking(true)
        .context("failed to configure daemon listener")?;

    let ty_binary = locate_ty_binary(workspace_root)?;
    let mut adapter = PersistentAdapter::new(workspace_root, &ty_binary)?;
    let idle_timeout = Duration::from_secs(idle_seconds.max(1));
    let mut last_activity = Instant::now();

    loop {
        let remaining = idle_timeout.saturating_sub(last_activity.elapsed());
        if remaining.is_zero() {
            break;
        }

        // Block until a client connects or the idle timeout elapses instead of busy polling.
        let accepted = match wait_readable(&listener, remaining) {
            Ok(true) => listener.accept(),
            Ok(false) => continue,
            Err(error) => Err(error),
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => {
                let _ = fs::remove_file(&socket_path);
                return Err(error).with_context(|| {
                    format!("daemon listener failed for {}", workspace_root.display())
                });
            }
        };

        last_activity = Instant::now();
        if handle_connection(workspace_root, &mut adapter, stream) == Connection::Shutdown {
            break;
        }
    }

    adapter.shutdown();
    let _ = fs::remove_file(&socket_path);
    Ok(())
}

/// What the serve loop does after handling a connection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Connection {
    Continue,
    Shutdown,
}

/// Answer one client request; failures are logged so the daemon outlives bad clients.
fn handle_connection(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    mut stream: UnixStream,
) -> Connection {
    if let Err(error) = stream.set_nonblocking(false) {
        debug::log(&format!("failed to configure daemon connection: {error}"));
        return Connection::Continue;
    }

    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            debug::log(&format!(
                "failed to read daemon request for {}: {error:#}",
                workspace_root.display()
            ));
            return Connection::Continue;
        }
    };

    let (response, next): (DaemonResponse, Connection) =
        match dispatch_request(workspace_root, adapter, request) {
            Ok(DispatchResult::Respond(reply)) => (Ok(reply), Connection::Continue),
            Ok(DispatchResult::Shutdown(reply)) => (Ok(reply), Connection::Shutdown),
            Err(error) => (Err(format!("{error:#}")), Connection::Continue),
        };

    if let Err(error) = write_frame(&mut stream, &response) {
        debug::log(&format!(
            "failed to write daemon response for {}: {error:#}",
            workspace_root.display()
        ));
    }
    next
}

/// Wait until `listener` has a pending connection, returning `false` on timeout.
fn wait_readable(listener: &UnixListener, timeout: Duration) -> io::Result<bool> {
    let mut poll_fd = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Round up so sub-millisecond remainders do not spin with a zero timeout.
    let timeout_millis =
        libc::c_int::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(libc::c_int::MAX);
    // SAFETY: `poll_fd` is a valid, exclusively borrowed `pollfd` array of length 1, and its
    // descriptor stays open for the call because `listener` is borrowed.
    match unsafe { libc::poll(&mut poll_fd, 1, timeout_millis) } {
        -1 => Err(io::Error::last_os_error()),
        ready => Ok(ready > 0),
    }
}

fn socket_path(workspace_root: &Path) -> Result<PathBuf> {
    daemon_file(workspace_root, "sock")
}

/// Return the per-workspace daemon file with the given extension under the lspyx cache.
fn daemon_file(workspace_root: &Path, extension: &str) -> Result<PathBuf> {
    let cache_dir =
        cache_dir().context("HOME is not set; unable to derive daemon cache directory")?;
    Ok(cache_dir.join(format!(
        "{}.{extension}",
        workspace_cache_key(workspace_root)
    )))
}

/// Open and exclusively lock the startup lock file; the lock is released when it is dropped.
fn acquire_startup_lock(workspace_root: &Path) -> Result<fs::File> {
    let lock_path = daemon_file(workspace_root, "lock")?;
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open daemon lock {}", lock_path.display()))?;

    // Hold an exclusive lock until the daemon is confirmed responsive.
    file.lock()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(file)
}

/// Return the daemon pid reported in a successful ping or shutdown reply.
fn response_pid(response: &DaemonResponse) -> Option<u32> {
    response
        .as_ref()
        .ok()?
        .payload
        .as_ref()?
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::daemon_serve_args;

    #[test]
    fn daemon_serve_args_place_workspace_under_daemon_subcommand() {
        let args = daemon_serve_args(Path::new("/tmp/example"), 900);

        assert_eq!(
            args,
            vec![
                OsString::from("daemon"),
                OsString::from("--workspace"),
                OsString::from("/tmp/example"),
                OsString::from("serve"),
                OsString::from("--idle-seconds"),
                OsString::from("900"),
            ]
        );
    }
}
