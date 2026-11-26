mod cli;
mod cmd_shaders;
mod game;

use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;

use cli::{Cli, Command, ShadersCommand};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        None | Some(Command::Ui) => leonida_ui::run()
            .map(|()| ExitCode::SUCCESS)
            .map_err(|error| anyhow!("could not open the window: {error}")),
        Some(Command::Shaders(shaders)) => match &shaders.command {
            ShadersCommand::Scan(args) => cmd_shaders::scan_command(args),
        },
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}
