mod adapter;
mod dispatch;
mod protocol;

use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};

use self::adapter::PersistentAdapter;
use self::dispatch::{DispatchResult, dispatch_request};
pub(crate) use self::protocol::{DaemonRequest, DaemonWireResponse};
use self::protocol::{read_request, write_response};
use crate::workspace::{adapter_status, locate_ty_binary, resolve_workspace_root};

const DEFAULT_IDLE_SECONDS: u64 = 1800;
const DAEMON_POLL_INTERVAL_MILLIS: u64 = 25;
const DAEMON_STARTUP_TIMEOUT_SECONDS: u64 = 5;
const ENSURE_AFTER_HELP: &str = "Example:\n  lspyx daemon ensure --idle-seconds 900";
const SERVE_AFTER_HELP: &str = "Example:\n  lspyx daemon serve --idle-seconds 900";
const STATUS_AFTER_HELP: &str = "Example:\n  lspyx daemon status";
const STOP_AFTER_HELP: &str = "Example:\n  lspyx daemon stop";

#[derive(Args, Debug)]
pub struct DaemonArgs {
    /// Optional override for a different repo; omit in the current workspace.
    #[arg(long)]
    pub workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub command: DaemonSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum DaemonSubcommand {
    #[command(after_help = ENSURE_AFTER_HELP)]
    Ensure(DaemonLifecycleArgs),
    #[command(after_help = SERVE_AFTER_HELP)]
    Serve(DaemonLifecycleArgs),
    #[command(after_help = STATUS_AFTER_HELP)]
    Status,
    #[command(after_help = STOP_AFTER_HELP)]
    Stop,
}

#[derive(Args, Debug)]
pub struct DaemonLifecycleArgs {
    #[arg(long, default_value_t = DEFAULT_IDLE_SECONDS)]
    pub idle_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub socket_path: PathBuf,
    pub workspace_root: PathBuf,
    pub pid: Option<u32>,
}

pub fn run_daemon_command(args: DaemonArgs) -> Result<String> {
    let cwd = env::current_dir().context("failed to determine current directory")?;
    let workspace_root = resolve_workspace_root(args.workspace.as_deref(), None, &cwd)?;

    match args.command {
        DaemonSubcommand::Ensure(lifecycle) => {
            let status = ensure_daemon(&workspace_root, lifecycle.idle_seconds)?;
            render_status(status)
        }
        DaemonSubcommand::Serve(lifecycle) => {
            serve_daemon(&workspace_root, lifecycle.idle_seconds)?;
            Ok("summary: daemon exited".to_string())
        }
        DaemonSubcommand::Status => {
            let status = daemon_status(&workspace_root)?;
            render_status(status)
        }
        DaemonSubcommand::Stop => {
            let stopped = stop_daemon(&workspace_root)?;
            Ok(format!(
                "summary: {}\nstopped: {stopped}",
                stop_summary(stopped)
            ))
        }
    }
}

pub fn run_via_daemon(workspace_root: &Path, request: DaemonRequest) -> Result<String> {
    render_daemon_response(run_via_daemon_response(workspace_root, request)?)
}

pub fn run_via_daemon_response(
    workspace_root: &Path,
    request: DaemonRequest,
) -> Result<DaemonWireResponse> {
    // Reuse an already-running daemon directly to avoid an extra ping roundtrip.
    if let Some(response) = send_request(workspace_root, &request)? {
        return validate_daemon_response(response);
    }

    ensure_daemon(workspace_root, DEFAULT_IDLE_SECONDS)?;

    let response = send_request(workspace_root, &request)?.ok_or_else(|| {
        anyhow!(
            "daemon started for workspace {} but did not accept the request",
            workspace_root.display()
        )
    })?;

    validate_daemon_response(response)
}

pub fn daemon_status(workspace_root: &Path) -> Result<DaemonStatus> {
    let socket_path = socket_path(workspace_root)?;
    let response = send_request(workspace_root, &DaemonRequest::Ping)?;

    let pid = response
        .and_then(|value| value.payload)
        .and_then(|payload| payload.get("pid").and_then(Value::as_u64))
        .map(|value| value as u32);

    Ok(DaemonStatus {
        running: pid.is_some(),
        socket_path,
        workspace_root: workspace_root.to_path_buf(),
        pid,
    })
}

pub fn ensure_daemon(workspace_root: &Path, idle_seconds: u64) -> Result<DaemonStatus> {
    // Serialize cold starts so concurrent clients cannot stomp the same socket.
    let _startup_lock = acquire_startup_lock(workspace_root)?;
    let status = daemon_status(workspace_root)?;
    if status.running {
        return Ok(status);
    }

    let socket = status.socket_path.clone();
    if socket.exists() {
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to remove stale socket {}", socket.display())
                });
            }
        }
    }

    spawn_daemon_process(workspace_root, idle_seconds)?;

    let deadline = Instant::now() + Duration::from_secs(DAEMON_STARTUP_TIMEOUT_SECONDS);
    while Instant::now() < deadline {
        let status = daemon_status(workspace_root)?;
        if status.running {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(DAEMON_POLL_INTERVAL_MILLIS));
    }

    bail!(
        "daemon did not become ready for workspace {}",
        workspace_root.display()
    )
}

fn spawn_daemon_process(workspace_root: &Path, idle_seconds: u64) -> Result<()> {
    let current_exe = env::current_exe().context("failed to resolve current lspyx binary")?;

    // Double-fork the daemon so it is re-parented before `daemon ensure` exits.
    let child_pid = unsafe { libc::fork() };
    if child_pid < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to fork daemon launcher");
    }

    if child_pid == 0 {
        if unsafe { libc::setsid() } == -1 {
            unsafe { libc::_exit(1) };
        }

        let grandchild_pid = unsafe { libc::fork() };
        if grandchild_pid < 0 {
            unsafe { libc::_exit(1) };
        }

        if grandchild_pid > 0 {
            unsafe { libc::_exit(0) };
        }

        let stderr = if env::var_os("LSPYX_DEBUG").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };
        let mut command = Command::new(current_exe);
        let error = command
            .args(daemon_serve_args(workspace_root, idle_seconds))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .exec();
        debug_log(format!("failed to exec daemon process: {error}"));
        unsafe { libc::_exit(1) };
    }

    let _ = unsafe { libc::waitpid(child_pid, std::ptr::null_mut(), 0) };
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

pub fn stop_daemon(workspace_root: &Path) -> Result<bool> {
    let response = send_request(workspace_root, &DaemonRequest::Shutdown)?;
    Ok(response.is_some())
}

pub fn adapter_status_with_daemon(workspace_root: &Path) -> Result<Value> {
    let daemon = daemon_status(workspace_root)?;
    let adapter = adapter_status(workspace_root);

    Ok(json!({
        "adapter": "ty",
        "available": adapter.ty.found,
        "ty": adapter.ty,
        "ruff": adapter.ruff,
        "daemon": {
            "running": daemon.running,
            "socket_path": daemon.socket_path,
            "pid": daemon.pid,
        }
    }))
}

fn render_status(status: DaemonStatus) -> Result<String> {
    let pid = status
        .pid
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());

    Ok(format!(
        "summary: {}\nrunning: {}\npid: {}\nsocket: {}",
        daemon_status_summary(status.running),
        status.running,
        pid,
        status.socket_path.display()
    ))
}

fn daemon_status_summary(running: bool) -> &'static str {
    if running {
        "daemon running"
    } else {
        "daemon not running"
    }
}

fn stop_summary(stopped: bool) -> &'static str {
    if stopped {
        "daemon stopped"
    } else {
        "daemon was not running"
    }
}

fn serve_daemon(workspace_root: &Path, idle_seconds: u64) -> Result<()> {
    let socket_path = socket_path(workspace_root)?;
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Refuse to take over a live socket; the startup lock handles stale cleanup before spawn.
    if let Some(response) = send_request(workspace_root, &DaemonRequest::Ping)? {
        let pid = response_pid(&response)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        bail!(
            "daemon already running for workspace {} (pid {})",
            workspace_root.display(),
            pid
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
        match listener.accept() {
            Ok((mut stream, _)) => {
                last_activity = Instant::now();
                stream
                    .set_nonblocking(false)
                    .context("failed to configure daemon connection")?;

                // Keep the daemon alive across client disconnects and per-request failures.
                let request = match read_request(&mut stream) {
                    Ok(request) => request,
                    Err(error) => {
                        debug_log(format!(
                            "failed to read daemon request for {}: {error:#}",
                            workspace_root.display()
                        ));
                        continue;
                    }
                };

                let response = match dispatch_request(workspace_root, &mut adapter, request) {
                    Ok(DispatchResult::Respond(response)) => response,
                    Ok(DispatchResult::Shutdown(response)) => {
                        let _ = write_response(&mut stream, &response);
                        break;
                    }
                    Err(error) => error_response(error),
                };

                if let Err(error) = write_response(&mut stream, &response) {
                    debug_log(format!(
                        "failed to write daemon response for {}: {error:#}",
                        workspace_root.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if last_activity.elapsed() >= idle_timeout {
                    break;
                }
                thread::sleep(Duration::from_millis(DAEMON_POLL_INTERVAL_MILLIS));
            }
            Err(error) => {
                let _ = fs::remove_file(&socket_path);
                return Err(error).with_context(|| {
                    format!("daemon listener failed for {}", workspace_root.display())
                });
            }
        }
    }

    let _ = adapter.shutdown();
    let _ = fs::remove_file(&socket_path);
    Ok(())
}

fn render_daemon_response(response: DaemonWireResponse) -> Result<String> {
    Ok(response.text.unwrap_or_default())
}

fn validate_daemon_response(response: DaemonWireResponse) -> Result<DaemonWireResponse> {
    if response.ok {
        return Ok(response);
    }

    Err(anyhow!(
        response
            .error
            .unwrap_or_else(|| "daemon request failed".to_string())
    ))
}

fn error_response(error: anyhow::Error) -> DaemonWireResponse {
    DaemonWireResponse {
        ok: false,
        payload: None,
        text: None,
        error: Some(format!("{error:#}")),
    }
}

fn send_request(
    workspace_root: &Path,
    request: &DaemonRequest,
) -> Result<Option<DaemonWireResponse>> {
    let socket_path = socket_path(workspace_root)?;
    protocol::send_request(&socket_path, request)
}

fn debug_log(message: String) {
    if env::var_os("LSPYX_DEBUG").is_some() {
        eprintln!("{message}");
    }
}

fn socket_path(workspace_root: &Path) -> Result<PathBuf> {
    let cache_dir = daemon_cache_dir()?;
    let workspace_hash = workspace_hash(workspace_root);

    Ok(cache_dir.join(format!("{workspace_hash:016x}.sock")))
}

fn startup_lock_path(workspace_root: &Path) -> Result<PathBuf> {
    let cache_dir = daemon_cache_dir()?;
    let workspace_hash = workspace_hash(workspace_root);

    Ok(cache_dir.join(format!("{workspace_hash:016x}.lock")))
}

fn daemon_cache_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache").join("lspyx"))
        .context("HOME is not set; unable to derive daemon cache directory")
}

fn acquire_startup_lock(workspace_root: &Path) -> Result<DaemonStartupLock> {
    let lock_path = startup_lock_path(workspace_root)?;
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
    let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if status != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to lock {}", lock_path.display()));
    }

    Ok(DaemonStartupLock { file })
}

fn workspace_hash(workspace_root: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    hasher.finish()
}

fn response_pid(response: &DaemonWireResponse) -> Option<u32> {
    response
        .payload
        .as_ref()
        .and_then(|payload| payload.get("pid"))
        .and_then(Value::as_u64)
        .map(|value| value as u32)
}

struct DaemonStartupLock {
    file: fs::File,
}

impl Drop for DaemonStartupLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
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
