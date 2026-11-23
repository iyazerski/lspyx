mod cli;
mod commands;
mod daemon;
mod debug;
mod diagnostics;
mod file_position;
mod lsp;
mod mcp;
mod render;
mod update;
mod workspace;

use clap::Parser;

use crate::cli::Cli;

pub fn cli_main() {
    if let Err(error) = commands::run(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
