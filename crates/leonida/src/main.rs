mod cli;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let _cli = cli::Cli::parse();
    ExitCode::SUCCESS
}
