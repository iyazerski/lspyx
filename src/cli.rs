use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::daemon;
use crate::workspace::canonicalize_path;

#[derive(Parser, Debug)]
#[command(name = "lspyx", version, about = "Python semantic navigation")]
pub(crate) struct Cli {
    /// Override the inferred workspace root when targeting a different repo.
    #[arg(long, global = true)]
    pub(crate) workspace: Option<PathBuf>,

    /// Limit the number of results returned.
    #[arg(long, global = true)]
    pub(crate) limit: Option<usize>,

    #[command(subcommand)]
    pub(crate) command: CommandKind,
}

#[derive(Subcommand, Debug)]
pub(crate) enum CommandKind {
    /// Check workspace resolution, adapter discovery, and daemon status.
    Doctor,
    /// Jump to the definition, declaration, or type of the symbol at a file position.
    Goto(GotoArgs),
    /// Find usages of the symbol at a file position.
    Usages(UsagesArgs),
    /// Search workspace symbols by name.
    FindSymbol(WorkspaceSymbolArgs),
    /// Identify the symbol at a file position and show hover details.
    Inspect(InspectArgs),
    /// Build a bounded outline or full symbol tree for a file.
    Outline(OutlineArgs),
    /// Manage the background daemon for a workspace.
    Daemon(daemon::DaemonArgs),
}

#[derive(Args, Debug)]
pub(crate) struct GotoArgs {
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Choose which semantic target to resolve.
    #[arg(long, value_enum, default_value_t = GotoTarget::Definition)]
    pub(crate) kind: GotoTarget,
}

#[derive(Args, Debug)]
pub(crate) struct UsagesArgs {
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Exclude the declaration site from results.
    #[arg(long)]
    pub(crate) exclude_declaration: bool,
}

#[derive(Args, Debug)]
pub(crate) struct PositionArgs {
    /// File position as file:line (1-based).
    #[arg(value_name = "FILE:LINE")]
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
    pub(crate) position: PositionArgs,
}

#[derive(Args, Debug)]
pub(crate) struct OutlineArgs {
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

#[derive(Debug)]
pub(crate) struct CommandInput {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl CommandInput {
    pub(crate) fn from_position_args(args: PositionArgs) -> Result<Self> {
        let (file, line) = parse_colon_location(&args.location)?;

        if line == 0 {
            bail!("line must be a 1-based value");
        }

        Ok(Self {
            file: canonicalize_path(&file)?,
            line,
            column: 1,
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

/// Parse a `file:line` string, splitting from the right to preserve colons in paths.
fn parse_colon_location(input: &str) -> Result<(PathBuf, usize)> {
    let mut parts = input.rsplitn(2, ':');
    let line_str = parts.next().unwrap_or("");
    let file_str = parts.next().unwrap_or("");

    if file_str.is_empty() {
        bail!("expected FILE:LINE format, got: {input}");
    }

    let line = line_str
        .parse::<usize>()
        .with_context(|| format!("expected FILE:LINE format, got: {input}"))?;

    Ok((PathBuf::from(file_str), line))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ValueEnum)]
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
