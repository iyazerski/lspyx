use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cli::{
    Cli, CommandInput, CommandKind, GotoArgs, InspectArgs, OutlineArgs, UsagesArgs,
    WorkspaceSymbolArgs, outline_depth,
};
use crate::daemon::{self, DaemonRequest};
use crate::mcp;
use crate::workspace::{
    canonicalize_path, locate_ruff_binary, locate_ty_binary, resolve_workspace_root,
};

pub(crate) fn run(cli: Cli) -> Result<()> {
    let output = match cli.command {
        // MCP owns stdout for its protocol stream, so it never prints a CLI result.
        CommandKind::Mcp(args) => return mcp::run_mcp_command(args),
        CommandKind::Doctor(args) => run_doctor(args.workspace)?,
        CommandKind::Goto(args) => run_goto(args)?,
        CommandKind::Usages(args) => run_usages(args)?,
        CommandKind::FindSymbol(args) => run_find_symbol(args)?,
        CommandKind::Inspect(args) => run_inspect(args)?,
        CommandKind::Outline(args) => run_outline(args)?,
        CommandKind::Daemon(args) => {
            daemon::run_daemon_command(&workspace_root(args.workspace, None)?, args.command)?
        }
    };
    println!("{output}");
    Ok(())
}

fn run_doctor(workspace: Option<PathBuf>) -> Result<String> {
    let workspace_root = workspace_root(workspace, None)?;
    let ty = locate_ty_binary(&workspace_root).ok();
    let ruff = locate_ruff_binary(&workspace_root).ok();
    let daemon_pid = daemon::daemon_status(&workspace_root)?.pid;

    let tool_line = |name: &str, path: &Option<PathBuf>| match path {
        Some(path) => format!("{name}: found at {}", path.display()),
        None => format!("{name}: not found"),
    };
    let daemon_line = match daemon_pid {
        Some(pid) => format!("daemon: running (pid {pid})"),
        None => "daemon: not running".to_string(),
    };

    Ok(format!(
        "summary: {}\nversion: {}\n{}\n{}\n{}",
        doctor_summary(ty.is_some(), ruff.is_some(), daemon_pid.is_some()),
        env!("CARGO_PKG_VERSION"),
        tool_line("ty", &ty),
        tool_line("ruff", &ruff),
        daemon_line
    ))
}

fn doctor_summary(ty_found: bool, ruff_found: bool, daemon_running: bool) -> String {
    let tools = match (ty_found, ruff_found) {
        (true, true) => "ty and ruff available",
        (true, false) => "ty available, ruff unavailable",
        (false, true) => "ty unavailable, ruff available",
        (false, false) => "ty and ruff unavailable",
    };
    let daemon = if daemon_running {
        "daemon running"
    } else {
        "daemon not running"
    };
    format!("{tools}, {daemon}")
}

fn run_goto(args: GotoArgs) -> Result<String> {
    let input = CommandInput::from_position_args(args.position)?;
    let workspace_root = workspace_root(args.common.workspace, Some(&input.file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Goto {
            file: input.file,
            line: input.line,
            column: input.column,
            target: args.kind,
            limit: args.common.limit,
        },
    )
}

fn run_usages(args: UsagesArgs) -> Result<String> {
    let input = CommandInput::from_position_args(args.position)?;
    let workspace_root = workspace_root(args.common.workspace, Some(&input.file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Usages {
            file: input.file,
            line: input.line,
            column: input.column,
            include_declaration: !args.exclude_declaration,
            limit: args.common.limit,
        },
    )
}

fn run_find_symbol(args: WorkspaceSymbolArgs) -> Result<String> {
    let workspace_root = workspace_root(args.common.workspace, None)?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::FindSymbol {
            query: args.query,
            kind: args.kind,
            limit: args.common.limit,
        },
    )
}

fn run_inspect(args: InspectArgs) -> Result<String> {
    let input = CommandInput::from_position_args(args.position)?;
    let workspace_root = workspace_root(args.common.workspace, Some(&input.file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Inspect {
            file: input.file,
            line: input.line,
            column: input.column,
        },
    )
}

fn run_outline(args: OutlineArgs) -> Result<String> {
    let file = canonicalize_path(&args.file)?;
    let workspace_root = workspace_root(args.common.workspace, Some(&file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Outline {
            file,
            depth: outline_depth(args.depth, args.full),
            limit: args.common.limit,
        },
    )
}

/// Resolve the workspace root from an explicit override, a target file, or the current directory.
fn workspace_root(workspace_override: Option<PathBuf>, file: Option<&Path>) -> Result<PathBuf> {
    let cwd = env::current_dir().context("failed to determine current directory")?;
    resolve_workspace_root(workspace_override.as_deref(), file, &cwd)
}
