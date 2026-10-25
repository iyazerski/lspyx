use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon::{self, Declaration};
use crate::file_position::FilePosition;
use crate::lsp::model::{OutlineDepth, SymbolKind};
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
#[command(
    name = "lspyx",
    version,
    about = "Python semantic navigation and diagnostics"
)]
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

impl UsagesArgs {
    /// Return whether usages should include the declaration site.
    pub(crate) fn declaration(&self) -> Declaration {
        if self.exclude_declaration {
            Declaration::Exclude
        } else {
            Declaration::Include
        }
    }
}

#[derive(Args, Debug)]
pub(crate) struct PositionArgs {
    /// File position as file:line:column (1-based).
    #[arg(value_name = "FILE:LINE:COLUMN")]
    pub(crate) location: String,
}

impl PositionArgs {
    /// Parse the location and canonicalize its file.
    ///
    /// # Errors
    ///
    /// Returns an error when the location is malformed or the file cannot be resolved.
    pub(crate) fn resolve(&self) -> Result<FilePosition> {
        let mut position: FilePosition = self.location.parse()?;
        position.file = canonicalize_path(&position.file)?;
        Ok(position)
    }
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
    #[arg(long, conflicts_with = "full")]
    pub(crate) depth: Option<usize>,
    /// Show the full document symbol tree without pruning.
    #[arg(long)]
    pub(crate) full: bool,
}

impl OutlineArgs {
    /// Return the requested outline depth.
    pub(crate) fn outline_depth(&self) -> OutlineDepth {
        if self.full {
            OutlineDepth::Full
        } else {
            self.depth
                .map_or_else(OutlineDepth::default, OutlineDepth::Limited)
        }
    }
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SymbolKindFilter {
    Class,
    Function,
    Method,
}

impl SymbolKindFilter {
    /// Return the LSP symbol kind this filter selects.
    pub(crate) fn lsp_kind(self) -> SymbolKind {
        match self {
            Self::Class => SymbolKind::CLASS,
            Self::Method => SymbolKind::METHOD,
            Self::Function => SymbolKind::FUNCTION,
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
    fn outline_rejects_depth_with_full() {
        assert!(
            Cli::try_parse_from(["lspyx", "outline", "a.py", "--full", "--depth", "1"]).is_err()
        );
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
