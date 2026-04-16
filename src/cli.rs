use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::daemon;
use crate::workspace::canonicalize_path;

#[derive(Parser, Debug)]
#[command(
    name = "lspyx",
    version,
    about = "Read-only Python semantic lookups for Codex"
)]
pub(crate) struct Cli {
    /// Override the inferred workspace root when targeting a different repo.
    #[arg(long, global = true)]
    pub(crate) workspace: Option<PathBuf>,

    /// Limit the number of results returned (does not affect --format count).
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
    /// Choose a compact output format for agent and script consumption.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct UsagesArgs {
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Exclude the declaration site from results.
    #[arg(long)]
    pub(crate) no_declaration: bool,
    /// Choose a compact output format for agent and script consumption.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
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
    /// Search text for the symbol name.
    #[arg(value_name = "QUERY")]
    pub(crate) query: String,
    /// Restrict results to a symbol kind.
    #[arg(long, value_enum)]
    pub(crate) kind: Option<SymbolKindFilter>,
    /// Choose a compact output format for agent and script consumption.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
}

#[derive(Args, Debug)]
pub(crate) struct InspectArgs {
    #[command(flatten)]
    pub(crate) position: PositionArgs,
    /// Choose a compact output format for agent and script consumption.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
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
    /// Choose a compact output format for agent and script consumption.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
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

        if line == 0 || column == 0 {
            bail!("line and column must be 1-based values");
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
    let mut parts = input.rsplitn(3, ':');
    let column_str = parts.next().unwrap_or("");
    let line_str = parts.next().unwrap_or("");
    let file_str = parts.next().unwrap_or("");

    if file_str.is_empty() {
        bail!("expected FILE:LINE:COLUMN format, got: {input}");
    }

    let line = line_str
        .parse::<usize>()
        .with_context(|| format!("expected FILE:LINE:COLUMN format, got: {input}"))?;
    let column = column_str
        .parse::<usize>()
        .with_context(|| format!("expected FILE:LINE:COLUMN format, got: {input}"))?;

    Ok((PathBuf::from(file_str), line, column))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputFormat {
    Text,
    Paths,
    Count,
}

impl OutputFormat {
    pub(crate) fn is_paths(self) -> bool {
        self == Self::Paths
    }

    pub(crate) fn is_count(self) -> bool {
        self == Self::Count
    }
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
