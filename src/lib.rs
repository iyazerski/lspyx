mod cli;
mod commands;
mod daemon;
mod lsp;
mod model;
mod parse;
mod render;
mod workspace;

use clap::Parser;

use crate::cli::Cli;

pub fn cli_main() {
    let cli = Cli::parse();
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
