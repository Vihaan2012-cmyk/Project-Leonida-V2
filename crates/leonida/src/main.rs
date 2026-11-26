mod cli;
mod cmd_shaders;
mod game;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command, ShadersCommand};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Shaders(shaders) => match &shaders.command {
            ShadersCommand::Scan(args) => cmd_shaders::scan(args),
        },
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}
