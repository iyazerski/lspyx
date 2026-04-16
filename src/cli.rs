use std::path::PathBuf;

use anyhow::{Result, bail};
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

    /// Choose a compact output format for agent and script consumption.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

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
    Usages(PositionArgs),
    /// Search workspace symbols by name.
    FindSymbol(WorkspaceSymbolArgs),
    /// Identify the symbol at a file position and show hover details.
    Inspect(PositionArgs),
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
pub(crate) struct PositionArgs {
    /// File that contains the target position.
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,
    /// 1-based line number.
    #[arg(value_name = "LINE")]
    pub(crate) line: usize,
    /// 1-based column number.
    #[arg(value_name = "COLUMN")]
    pub(crate) column: usize,
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
        if args.line == 0 || args.column == 0 {
            bail!("line and column must be 1-based values");
        }

        Ok(Self {
            file: canonicalize_path(&args.file)?,
            line: args.line,
            column: args.column,
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputFormat {
    Json,
    Text,
    Paths,
    Count,
}

impl OutputFormat {
    pub(crate) fn is_json(self) -> bool {
        self == Self::Json
    }

    pub(crate) fn is_text(self) -> bool {
        self == Self::Text
    }

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
