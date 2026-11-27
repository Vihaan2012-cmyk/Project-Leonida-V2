mod cli;
mod cmd_check;
mod cmd_exe;
mod cmd_package;
mod cmd_run;
mod cmd_shaders;
mod cmd_systems;
mod cmd_verify;
mod game;
mod shader_build;

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
            ShadersCommand::Disasm(args) => cmd_shaders::disasm_command(args),
            ShadersCommand::Report(args) => cmd_shaders::report_command(args),
            ShadersCommand::Verify(args) => cmd_verify::verify_command(args),
        },
        Some(Command::Exe(exe)) => cmd_exe::exe_command(exe),
        Some(Command::Check(args)) => cmd_check::check_command(args),
        Some(Command::Run(args)) => cmd_run::run_command(args),
        Some(Command::Triage(args)) => cmd_package::triage_command(args),
        Some(Command::Systems(systems)) => cmd_systems::systems_command(systems),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}
