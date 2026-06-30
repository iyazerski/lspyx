use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::lsp::{LspSession, column_to_utf16_offset, path_to_file_uri, read_line_text};
use crate::model::{
    DocumentSymbolNode, LocationOutput, LocationRecord, OutlineOutput, RangeRecord,
    ResolvedPosition, SymbolAtOutput, WorkspaceSymbolOutput, WorkspaceSymbolRecord,
};
use crate::parse::{
    apply_document_symbol_metadata, build_symbol_hierarchy, extract_symbol_at,
    find_document_symbol, parse_document_symbols, parse_hover_contents, parse_location_response,
    parse_workspace_symbols, prune_outline_depth,
};
use crate::render::{
    render_location_output, render_outline_output, render_symbol_at_output,
    render_workspace_symbol_output,
};
use crate::workspace::{
    adapter_status, canonicalize_path, locate_ty_binary, resolve_workspace_root,
};

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

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonWireResponse {
    pub ok: bool,
    pub payload: Option<Value>,
    pub text: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub enum DaemonRequest {
    Ping,
    Shutdown,
    Goto {
        file: PathBuf,
        line: usize,
        column: usize,
        target: GotoTarget,
        limit: Option<usize>,
    },
    Usages {
        file: PathBuf,
        line: usize,
        column: usize,
        include_declaration: bool,
        limit: Option<usize>,
    },
    FindSymbol {
        query: String,
        kind: Option<SymbolKindFilter>,
        limit: Option<usize>,
    },
    Inspect {
        file: PathBuf,
        line: usize,
        column: usize,
    },
    Outline {
        file: PathBuf,
        depth: Option<usize>,
        limit: Option<usize>,
    },
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
    // Reuse an already-running daemon directly to avoid an extra ping roundtrip.
    if let Some(response) = send_request(workspace_root, &request)? {
        return render_daemon_response(response);
    }

    ensure_daemon(workspace_root, DEFAULT_IDLE_SECONDS)?;

    let response = send_request(workspace_root, &request)?.ok_or_else(|| {
        anyhow!(
            "daemon started for workspace {} but did not accept the request",
            workspace_root.display()
        )
    })?;

    render_daemon_response(response)
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

fn dispatch_request(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    request: DaemonRequest,
) -> Result<DispatchResult> {
    let response = match request {
        DaemonRequest::Ping => DaemonWireResponse {
            ok: true,
            payload: Some(json!({
                "pid": std::process::id(),
                "workspace_root": workspace_root,
            })),
            text: Some(format!("daemon alive: {}", workspace_root.display())),
            error: None,
        },
        DaemonRequest::Shutdown => {
            return Ok(DispatchResult::Shutdown(DaemonWireResponse {
                ok: true,
                payload: Some(json!({
                    "pid": std::process::id(),
                    "workspace_root": workspace_root,
                    "stopped": true,
                })),
                text: Some("daemon shutting down".to_string()),
                error: None,
            }));
        }
        DaemonRequest::Goto {
            file,
            line,
            column,
            target,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;

            // Route each goto target through the corresponding LSP request.
            let locations = match target {
                GotoTarget::Definition => {
                    adapter.definition_locations(&file, line, position.requested_column)?
                }
                GotoTarget::Declaration => adapter.request_locations(
                    "textDocument/declaration",
                    &file,
                    line,
                    position.requested_column,
                    false,
                )?,
                GotoTarget::Type => adapter.request_locations(
                    "textDocument/typeDefinition",
                    &file,
                    line,
                    position.requested_column,
                    false,
                )?,
            };

            build_location_response(
                LocationOutput {
                    ok: true,
                    workspace_root: workspace_root.to_path_buf(),
                    position,
                    target: Some(target),
                    locations,
                },
                limit,
            )?
        }
        DaemonRequest::Usages {
            file,
            line,
            column,
            include_declaration,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let locations = adapter.reference_locations(
                workspace_root,
                &file,
                line,
                position.requested_column,
                include_declaration,
            )?;

            build_location_response(
                LocationOutput {
                    ok: true,
                    workspace_root: workspace_root.to_path_buf(),
                    position,
                    target: None,
                    locations,
                },
                limit,
            )?
        }
        DaemonRequest::FindSymbol { query, kind, limit } => {
            let symbols = adapter.workspace_symbol(&query)?;
            // Enrich symbols with source snippets for context.
            let symbols = symbols
                .into_iter()
                .map(|mut symbol| {
                    symbol.snippet = read_line_text(&symbol.file, symbol.range.start.line)
                        .ok()
                        .map(|value| value.trim().to_string());
                    symbol
                })
                .collect();
            let payload = WorkspaceSymbolOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                query,
                symbols,
            };
            build_rendered_response(render_workspace_symbol_output(limit, &payload, kind)?)?
        }
        DaemonRequest::Inspect { file, line, column } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let hover = Some(adapter.hover(&file, line, position.requested_column)?);
            let payload = SymbolAtOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                symbol: position.symbol.clone(),
                position,
                hover,
            };

            build_rendered_response(render_symbol_at_output(&payload)?)?
        }
        DaemonRequest::Outline { file, depth, limit } => {
            let symbols = adapter.document_symbols(&file)?;
            // Preserve the full tree for --full and prune only when a depth limit is requested.
            let hierarchy = if let Some(depth) = depth {
                prune_outline_depth(build_symbol_hierarchy(symbols), depth)
            } else {
                build_symbol_hierarchy(symbols)
            };
            let payload = OutlineOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                file,
                depth,
                symbols: hierarchy,
            };

            build_rendered_response(render_outline_output(limit, &payload)?)?
        }
    };

    Ok(DispatchResult::Respond(response))
}

fn build_location_response(
    payload: LocationOutput,
    limit: Option<usize>,
) -> Result<DaemonWireResponse> {
    build_rendered_response(render_location_output(limit, &payload)?)
}

fn build_rendered_response(text: String) -> Result<DaemonWireResponse> {
    Ok(DaemonWireResponse {
        ok: true,
        payload: None,
        text: Some(text),
        error: None,
    })
}

fn render_daemon_response(response: DaemonWireResponse) -> Result<String> {
    if !response.ok {
        return Err(anyhow!(
            response
                .error
                .unwrap_or_else(|| "daemon request failed".to_string())
        ));
    }

    Ok(response.text.unwrap_or_default())
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
    if !socket_path.exists() {
        return Ok(None);
    }

    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            let _ = fs::remove_file(&socket_path);
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to connect to daemon {}", socket_path.display()));
        }
    };

    let body = serde_json::to_vec(request)?;
    write_frame(&mut stream, body.as_slice())?;
    stream.shutdown(Shutdown::Write)?;

    let response_body = match read_frame(&mut stream) {
        Ok(body) => body,
        Err(error) if is_unexpected_eof(&error) => return Ok(None),
        Err(error) => return Err(error),
    };

    Ok(Some(serde_json::from_slice(&response_body)?))
}

fn read_request(stream: &mut UnixStream) -> Result<DaemonRequest> {
    let body = read_frame(stream)?;
    serde_json::from_slice(&body).context("failed to parse daemon request")
}

fn write_response(stream: &mut UnixStream, response: &DaemonWireResponse) -> Result<()> {
    let body = serde_json::to_vec(response)?;
    write_frame(stream, body.as_slice())?;
    Ok(())
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut length_bytes = [0_u8; 8];
    stream.read_exact(&mut length_bytes)?;

    let length = u64::from_be_bytes(length_bytes);
    let length = usize::try_from(length).context("daemon frame length exceeded usize")?;
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    Ok(body)
}

fn write_frame(stream: &mut UnixStream, body: &[u8]) -> Result<()> {
    let length = u64::try_from(body.len()).context("daemon frame length exceeded u64")?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn is_unexpected_eof(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|source| source.kind() == std::io::ErrorKind::UnexpectedEof)
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

enum DispatchResult {
    Respond(DaemonWireResponse),
    Shutdown(DaemonWireResponse),
}

struct PersistentAdapter {
    session: LspSession,
    documents: HashMap<PathBuf, OpenDocument>,
}

struct OpenDocument {
    text: String,
    version: i32,
}

impl PersistentAdapter {
    fn new(workspace_root: &Path, ty_binary: &Path) -> Result<Self> {
        Ok(Self {
            session: LspSession::start(ty_binary, workspace_root)?,
            documents: HashMap::new(),
        })
    }

    fn shutdown(&mut self) -> Result<()> {
        self.session.shutdown()
    }

    fn resolve_position(
        &mut self,
        file: &Path,
        line: usize,
        requested_column: usize,
    ) -> Result<ResolvedPosition> {
        self.ensure_file_synced(file)?;

        let source_line = read_line_text(file, line)
            .ok()
            .map(|value| value.trim().to_string());
        let symbol = extract_symbol_at(file, line, requested_column)?;
        let resolved_column = symbol.as_ref().map(|value| value.start_column);
        let symbol = if let Some(symbol) = symbol {
            let document_symbols = self.document_symbols(file)?;
            let document_symbol =
                find_document_symbol(document_symbols.as_slice(), line, symbol.start_column)
                    .filter(|value| value.name == symbol.name);
            Some(apply_document_symbol_metadata(symbol, document_symbol))
        } else {
            None
        };

        Ok(ResolvedPosition {
            file: file.to_path_buf(),
            line,
            requested_column,
            resolved_column,
            source_line,
            symbol,
        })
    }

    fn definition_locations(
        &mut self,
        file: &Path,
        line: usize,
        column: usize,
    ) -> Result<Vec<LocationRecord>> {
        let locations =
            self.request_locations("textDocument/definition", file, line, column, false)?;
        self.resolve_imported_definition(file, line, column, locations)
    }

    fn reference_locations(
        &mut self,
        workspace_root: &Path,
        file: &Path,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LocationRecord>> {
        let locations = self.request_locations(
            "textDocument/references",
            file,
            line,
            column,
            include_declaration,
        )?;
        let unique_files = locations
            .iter()
            .map(|location| location.file.clone())
            .collect::<HashSet<_>>();
        if unique_files.len() > 1 {
            return Ok(locations);
        }

        let Some(symbol) = extract_symbol_at(file, line, column)? else {
            return Ok(locations);
        };
        let Some(canonical_symbol) = self.unique_workspace_symbol(symbol.name.as_str())? else {
            return Ok(locations);
        };

        let lexical_locations =
            collect_symbol_occurrences(workspace_root, canonical_symbol.name.as_str())?;
        Ok(merge_locations(locations, lexical_locations))
    }

    fn request_locations(
        &mut self,
        method: &str,
        file: &Path,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LocationRecord>> {
        self.ensure_file_synced(file)?;
        let utf16_character = self.utf16_offset(file, line, column)?;

        let params = if method == "textDocument/references" {
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                },
                "context": {
                    "includeDeclaration": include_declaration,
                }
            })
        } else {
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                }
            })
        };

        let response = self.session.request(method, params)?;
        let locations = parse_location_response(response)?;
        self.enrich_locations(locations)
    }

    fn resolve_imported_definition(
        &mut self,
        file: &Path,
        line: usize,
        column: usize,
        locations: Vec<LocationRecord>,
    ) -> Result<Vec<LocationRecord>> {
        let Some(symbol) = extract_symbol_at(file, line, column)? else {
            return Ok(locations);
        };
        let Some(unique_symbol) = self.unique_workspace_symbol(symbol.name.as_str())? else {
            return Ok(locations);
        };

        let redirected = locations
            .iter()
            .any(|location| is_import_location(location, symbol.name.as_str()));
        if !redirected {
            return Ok(locations);
        }

        Ok(vec![location_from_workspace_symbol(&unique_symbol)])
    }

    fn hover(&mut self, file: &Path, line: usize, column: usize) -> Result<String> {
        self.ensure_file_synced(file)?;
        let utf16_character = self.utf16_offset(file, line, column)?;
        let response = self.session.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                }
            }),
        )?;
        parse_hover_contents(response)
    }

    fn document_symbols(&mut self, file: &Path) -> Result<Vec<DocumentSymbolNode>> {
        self.ensure_file_synced(file)?;
        let response = self.session.request(
            "textDocument/documentSymbol",
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
            }),
        )?;
        parse_document_symbols(response)
    }

    fn workspace_symbol(&mut self, query: &str) -> Result<Vec<WorkspaceSymbolRecord>> {
        let response = self
            .session
            .request("workspace/symbol", json!({ "query": query }))?;
        parse_workspace_symbols(response)
    }

    fn unique_workspace_symbol(&mut self, query: &str) -> Result<Option<WorkspaceSymbolRecord>> {
        let symbols = self.workspace_symbol(query)?;
        let exact_case_sensitive = symbols
            .iter()
            .filter(|symbol| symbol.name == query)
            .cloned()
            .collect::<Vec<_>>();
        if exact_case_sensitive.len() == 1 {
            return Ok(exact_case_sensitive.into_iter().next());
        }

        let exact_case_insensitive = symbols
            .iter()
            .filter(|symbol| symbol.name.eq_ignore_ascii_case(query))
            .cloned()
            .collect::<Vec<_>>();
        if exact_case_insensitive.len() == 1 {
            return Ok(exact_case_insensitive.into_iter().next());
        }

        Ok(None)
    }

    fn ensure_file_synced(&mut self, file: &Path) -> Result<()> {
        let canonical = canonicalize_path(file)?;
        let text = fs::read_to_string(&canonical)
            .with_context(|| format!("failed to read {}", canonical.display()))?;

        match self.documents.get_mut(&canonical) {
            Some(document) if document.text != text => {
                document.version += 1;
                self.session
                    .change_file(&canonical, document.version, &text)?;
                document.text = text;
            }
            Some(_) => {}
            None => {
                self.session.open_file_with_text(&canonical, 1, &text)?;
                self.documents
                    .insert(canonical, OpenDocument { text, version: 1 });
            }
        }

        Ok(())
    }

    fn utf16_offset(&self, file: &Path, line: usize, column: usize) -> Result<usize> {
        let line_text = read_line_text(file, line)?;
        column_to_utf16_offset(&line_text, column)
    }

    fn enrich_locations(&self, locations: Vec<LocationRecord>) -> Result<Vec<LocationRecord>> {
        let mut enriched = Vec::with_capacity(locations.len());

        for mut location in locations {
            let snippet = read_line_text(&location.file, location.range.start.line).ok();
            location.snippet = snippet.map(|value| value.trim().to_string());
            enriched.push(location);
        }

        Ok(enriched)
    }
}

fn is_import_location(location: &LocationRecord, symbol_name: &str) -> bool {
    let Some(snippet) = location.snippet.as_deref() else {
        return false;
    };
    let trimmed = snippet.trim_start();

    (trimmed.starts_with("from ") || trimmed.starts_with("import "))
        && trimmed.contains(symbol_name)
}

fn location_from_workspace_symbol(symbol: &WorkspaceSymbolRecord) -> LocationRecord {
    LocationRecord {
        file: symbol.file.clone(),
        range: symbol.range.clone(),
        snippet: read_line_text(&symbol.file, symbol.range.start.line)
            .ok()
            .map(|value| value.trim().to_string()),
    }
}

fn collect_symbol_occurrences(
    workspace_root: &Path,
    symbol_name: &str,
) -> Result<Vec<LocationRecord>> {
    let mut locations = Vec::new();
    collect_symbol_occurrences_recursive(
        workspace_root,
        workspace_root,
        symbol_name,
        &mut locations,
    )?;
    Ok(locations)
}

fn collect_symbol_occurrences_recursive(
    workspace_root: &Path,
    current: &Path,
    symbol_name: &str,
    locations: &mut Vec<LocationRecord>,
) -> Result<()> {
    if is_ignored_directory(current) {
        return Ok(());
    }

    for entry in
        fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if !is_ignored_directory(&path) {
                collect_symbol_occurrences_recursive(
                    workspace_root,
                    &path,
                    symbol_name,
                    locations,
                )?;
            }
            continue;
        }

        if !is_python_file(&path) {
            continue;
        }

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        for (line_index, line) in text.lines().enumerate() {
            for column in symbol_columns(line, symbol_name) {
                locations.push(LocationRecord {
                    file: path.clone(),
                    range: RangeRecord {
                        start: crate::model::PositionRecord {
                            line: line_index + 1,
                            column,
                        },
                        end: crate::model::PositionRecord {
                            line: line_index + 1,
                            column: column + symbol_name.chars().count(),
                        },
                    },
                    snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    let _ = workspace_root;
    Ok(())
}

fn symbol_columns(line: &str, symbol_name: &str) -> Vec<usize> {
    let mut columns = Vec::new();

    for (byte_index, _) in line.match_indices(symbol_name) {
        let before = line[..byte_index].chars().next_back();
        let after = line[byte_index + symbol_name.len()..].chars().next();

        if before.is_some_and(is_symbol_char_for_search)
            || after.is_some_and(is_symbol_char_for_search)
        {
            continue;
        }

        columns.push(line[..byte_index].chars().count() + 1);
    }

    columns
}

fn is_symbol_char_for_search(value: char) -> bool {
    value == '_' || value.is_alphanumeric()
}

fn is_python_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("py") | Some("pyi")
    )
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

fn merge_locations(
    mut primary: Vec<LocationRecord>,
    additional: Vec<LocationRecord>,
) -> Vec<LocationRecord> {
    let mut seen = primary
        .iter()
        .map(|location| {
            (
                location.file.clone(),
                location.range.start.line,
                location.range.start.column,
            )
        })
        .collect::<HashSet<_>>();

    for location in additional {
        let key = (
            location.file.clone(),
            location.range.start.line,
            location.range.start.column,
        );
        if seen.insert(key) {
            primary.push(location);
        }
    }

    primary
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
