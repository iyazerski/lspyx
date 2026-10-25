//! Agent-friendly Python semantic navigation and diagnostics backed by ty and Ruff.

mod cli;
mod commands;
mod daemon;
mod debug;
mod diagnostics;
mod file_position;
mod lsp;
mod mcp;
mod render;
mod workspace;

use clap::Parser;

use crate::cli::Cli;

/// Parse command-line arguments, run the command, and exit with status 1 on failure.
pub fn cli_main() {
    if let Err(error) = commands::run(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
