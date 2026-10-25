use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cli::{
    Cli, CommandKind, GotoArgs, InspectArgs, OutlineArgs, UsagesArgs, WorkspaceSymbolArgs,
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
        CommandKind::Doctor(args) => run_doctor(args.workspace.as_deref())?,
        CommandKind::Goto(args) => run_goto(args)?,
        CommandKind::Usages(args) => run_usages(args)?,
        CommandKind::FindSymbol(args) => run_find_symbol(args)?,
        CommandKind::Inspect(args) => run_inspect(args)?,
        CommandKind::Outline(args) => run_outline(args)?,
        CommandKind::Daemon(args) => daemon::run_daemon_command(
            &workspace_root(args.workspace.as_deref(), None)?,
            args.command,
        )?,
    };
    println!("{output}");
    Ok(())
}

fn run_doctor(workspace: Option<&Path>) -> Result<String> {
    let workspace_root = workspace_root(workspace, None)?;
    let ty = locate_ty_binary(&workspace_root).ok();
    let ruff = locate_ruff_binary(&workspace_root).ok();
    let daemon_pid = daemon::daemon_status(&workspace_root)?.pid;

    let availability = |path: Option<&Path>| match path {
        Some(_) => "available",
        None => "unavailable",
    };
    let tools_summary = if ty.is_some() == ruff.is_some() {
        format!("ty and ruff {}", availability(ty.as_deref()))
    } else {
        format!(
            "ty {}, ruff {}",
            availability(ty.as_deref()),
            availability(ruff.as_deref())
        )
    };
    let tool_line = |name: &str, path: Option<&Path>| match path {
        Some(path) => format!("{name}: found at {}", path.display()),
        None => format!("{name}: not found"),
    };
    let (daemon_summary, daemon_line) = match daemon_pid {
        Some(pid) => ("daemon running", format!("daemon: running (pid {pid})")),
        None => ("daemon not running", "daemon: not running".to_string()),
    };

    Ok(format!(
        "summary: {tools_summary}, {daemon_summary}\nversion: {}\n{}\n{}\n{daemon_line}",
        env!("CARGO_PKG_VERSION"),
        tool_line("ty", ty.as_deref()),
        tool_line("ruff", ruff.as_deref()),
    ))
}

fn run_goto(args: GotoArgs) -> Result<String> {
    let position = args.position.resolve()?;
    let workspace_root = workspace_root(args.common.workspace.as_deref(), Some(&position.file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Goto {
            position,
            target: args.kind,
            limit: args.common.limit,
        },
    )
}

fn run_usages(args: UsagesArgs) -> Result<String> {
    let position = args.position.resolve()?;
    let workspace_root = workspace_root(args.common.workspace.as_deref(), Some(&position.file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Usages {
            position,
            declaration: args.declaration(),
            limit: args.common.limit,
        },
    )
}

fn run_find_symbol(args: WorkspaceSymbolArgs) -> Result<String> {
    let workspace_root = workspace_root(args.common.workspace.as_deref(), None)?;

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
    let position = args.position.resolve()?;
    let workspace_root = workspace_root(args.common.workspace.as_deref(), Some(&position.file))?;

    daemon::run_via_daemon(&workspace_root, DaemonRequest::Inspect { position })
}

fn run_outline(args: OutlineArgs) -> Result<String> {
    let file = canonicalize_path(&args.file)?;
    let workspace_root = workspace_root(args.common.workspace.as_deref(), Some(&file))?;

    daemon::run_via_daemon(
        &workspace_root,
        DaemonRequest::Outline {
            file,
            depth: args.outline_depth(),
            limit: args.common.limit,
        },
    )
}

/// Resolve the workspace root from an explicit override, a target file, or the current directory.
fn workspace_root(workspace_override: Option<&Path>, file: Option<&Path>) -> Result<PathBuf> {
    let cwd = env::current_dir().context("failed to determine current directory")?;
    resolve_workspace_root(workspace_override, file, &cwd)
}
