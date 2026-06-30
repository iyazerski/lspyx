mod cli;
mod commands;
mod daemon;
mod lsp;
mod mcp;
mod model;
mod parse;
mod render;
mod workspace;

use clap::Parser;

use crate::cli::{Cli, CommandKind};

pub fn cli_main() {
    let cli = Cli::parse();

    if let CommandKind::Mcp(args) = cli.command {
        if let Err(error) = mcp::run_mcp_command(args) {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        return;
    }

    let result = commands::run(cli);

    match result {
        Ok(output) => {
            println!("{output}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
