use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon;
use crate::mcp;
use crate::workspace::canonicalize_path;

const DOCTOR_AFTER_HELP: &str = "Example:\n  lspyx doctor";
const GOTO_AFTER_HELP: &str =
    "Examples:\n  lspyx goto src/app.py:42:17\n  lspyx goto src/app.py:42:17 --kind type";
const USAGES_AFTER_HELP: &str = "Examples:\n  lspyx usages src/app.py:42:17\n  lspyx usages src/app.py:42:17 --exclude-declaration";
const FIND_SYMBOL_AFTER_HELP: &str =
    "Examples:\n  lspyx find-symbol User\n  lspyx find-symbol main --kind function --limit 5";
const INSPECT_AFTER_HELP: &str =
    "Examples:\n  lspyx inspect src/app.py:42:17\n  lspyx inspect src/app.py:84:9";
const OUTLINE_AFTER_HELP: &str =
    "Examples:\n  lspyx outline src/app.py\n  lspyx outline src/app.py --full";
const DAEMON_AFTER_HELP: &str =
    "Examples:\n  lspyx daemon status\n  lspyx daemon ensure --idle-seconds 900";
const MCP_AFTER_HELP: &str = "Example:\n  lspyx mcp serve";

#[derive(Parser, Debug)]
#[command(name = "lspyx", version, about = "Python semantic navigation")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: CommandKind,
}

#[derive(Subcommand, Debug)]
pub(crate) enum CommandKind {
    /// Check workspace resolution, adapter discovery, and daemon status.
    #[command(after_help = DOCTOR_AFTER_HELP)]
    Doctor(WorkspaceArgs),
    /// Jump to the definition, declaration, or type of the symbol at a file position.
    #[command(after_help = GOTO_AFTER_HELP)]
    Goto(GotoArgs),
    /// Find usages of the symbol at a file position.
    #[command(after_help = USAGES_AFTER_HELP)]
    Usages(UsagesArgs),
    /// Search workspace symbols by name.
    #[command(after_help = FIND_SYMBOL_AFTER_HELP)]
    FindSymbol(WorkspaceSymbolArgs),
    /// Identify the symbol at a file position and show hover details.
    #[command(after_help = INSPECT_AFTER_HELP)]
    Inspect(InspectArgs),
    /// Build a bounded outline or full symbol tree for a file.
    #[command(after_help = OUTLINE_AFTER_HELP)]
    Outline(OutlineArgs),
    /// Manage the background daemon for a workspace.
    #[command(after_help = DAEMON_AFTER_HELP)]
    Daemon(daemon::DaemonArgs),
    /// Run lspyx as an MCP server.
    #[command(after_help = MCP_AFTER_HELP)]
    Mcp(mcp::McpArgs),
}

#[derive(Args, Debug)]
pub(crate) struct GotoArgs {
    #[command(flatten)]
    pub(crate) common: WorkspaceLimitArgs,
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Choose which semantic target to resolve.
    #[arg(long, value_enum, default_value_t = GotoTarget::Definition)]
    pub(crate) kind: GotoTarget,
}

#[derive(Args, Debug)]
pub(crate) struct UsagesArgs {
    #[command(flatten)]
    pub(crate) common: WorkspaceLimitArgs,
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Exclude the declaration site from results.
    #[arg(long)]
    pub(crate) exclude_declaration: bool,
}

#[derive(Args, Debug)]
pub(crate) struct PositionArgs {
    /// File position as file:line:column (1-based).
    #[arg(value_name = "FILE:LINE:COLUMN")]
    pub(crate) location: String,
}

#[derive(Args, Debug)]
pub(crate) struct FileArgs {
    /// File to inspect.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,
}

#[derive(Args, Debug)]
pub(crate) struct WorkspaceSymbolArgs {
    #[command(flatten)]
    pub(crate) common: WorkspaceLimitArgs,
    /// Search text for the symbol name.
    #[arg(value_name = "QUERY")]
    pub(crate) query: String,
    /// Restrict results to a symbol kind.
    #[arg(long, value_enum)]
    pub(crate) kind: Option<SymbolKindFilter>,
}

#[derive(Args, Debug)]
pub(crate) struct InspectArgs {
    #[command(flatten)]
    pub(crate) common: WorkspaceArgs,
    #[command(flatten)]
    pub(crate) position: PositionArgs,
}

#[derive(Args, Debug)]
pub(crate) struct OutlineArgs {
    #[command(flatten)]
    pub(crate) common: WorkspaceLimitArgs,
    /// File to outline.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,
    /// Limit nesting depth in the rendered outline.
    #[arg(long)]
    pub(crate) depth: Option<usize>,
    /// Show the full document symbol tree without pruning.
    #[arg(long)]
    pub(crate) full: bool,
}

#[derive(Args, Debug)]
pub(crate) struct WorkspaceArgs {
    /// Optional override for a different repo; omit in the current workspace.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct WorkspaceLimitArgs {
    /// Optional override for a different repo; omit in the current workspace.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Limit the number of results returned.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct CommandInput {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl CommandInput {
    pub(crate) fn from_position_args(args: PositionArgs) -> Result<Self> {
        let (file, line, column) = parse_colon_location(&args.location)?;

        if line == 0 {
            bail!("line must be a 1-based value");
        }

        if column == 0 {
            bail!("column must be a 1-based value");
        }

        Ok(Self {
            file: canonicalize_path(&file)?,
            line,
            column,
        })
    }

    pub(crate) fn from_file_args(args: FileArgs) -> Result<Self> {
        Ok(Self {
            file: canonicalize_path(&args.file)?,
            line: 1,
            column: 1,
        })
    }
}

/// Parse a `file:line:column` string, splitting from the right to preserve colons in paths.
fn parse_colon_location(input: &str) -> Result<(PathBuf, usize, usize)> {
    let parts = input.rsplitn(3, ':').collect::<Vec<_>>();

    match parts.as_slice() {
        [column_str, line_str, file_str] if !file_str.is_empty() => Ok((
            PathBuf::from(file_str),
            line_str
                .parse::<usize>()
                .with_context(|| format!("expected FILE:LINE:COLUMN format, got: {input}"))?,
            column_str
                .parse::<usize>()
                .with_context(|| format!("expected FILE:LINE:COLUMN format, got: {input}"))?,
        )),
        _ => bail!("expected FILE:LINE:COLUMN format, got: {input}"),
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SymbolKindFilter {
    Class,
    Function,
    Method,
}

impl SymbolKindFilter {
    pub(crate) fn matches(self, kind: u64) -> bool {
        match self {
            Self::Class => kind == 5,
            Self::Function => kind == 12,
            Self::Method => kind == 6,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GotoTarget {
    Definition,
    Declaration,
    Type,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    #[test]
    fn existing_subcommands_still_parse() {
        for args in [
            vec!["lspyx", "doctor"],
            vec!["lspyx", "find-symbol", "--limit", "5", "User"],
            vec!["lspyx", "inspect", "src/app.py:1:1"],
            vec!["lspyx", "goto", "src/app.py:1:1"],
            vec!["lspyx", "usages", "src/app.py:1:1"],
            vec!["lspyx", "outline", "src/app.py"],
            vec!["lspyx", "daemon", "--workspace", ".", "status"],
        ] {
            Cli::try_parse_from(args).unwrap();
        }
    }

    #[test]
    fn mcp_serve_subcommand_parses() {
        Cli::try_parse_from(["lspyx", "mcp", "serve"]).unwrap();
    }

    #[test]
    fn mcp_serve_rejects_cli_only_options() {
        for args in [
            ["lspyx", "mcp", "--workspace", ".", "serve"],
            ["lspyx", "mcp", "--limit", "5", "serve"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
