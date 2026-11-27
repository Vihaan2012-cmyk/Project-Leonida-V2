mod cli;
mod cmd_assets;
mod cmd_check;
mod cmd_config;
mod cmd_content;
mod cmd_exe;
mod cmd_mods;
mod cmd_package;
mod cmd_pipelines;
mod cmd_run;
mod cmd_shaders;
mod cmd_systems;
mod cmd_verify;
mod game;
mod pipeline_work;

use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;

use cli::{Cli, Command, ContentCommand, ShadersCommand};

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
        Some(Command::Config(args)) => cmd_config::config_command(args),
        Some(Command::Run(args)) => cmd_run::run_command(args),
        Some(Command::Triage(args)) => cmd_package::triage_command(args),
        Some(Command::Systems(systems)) => cmd_systems::systems_command(systems),
        Some(Command::Pipelines(args)) => cmd_pipelines::pipelines_command(args),
        Some(Command::Content(content)) => match &content.command {
            ContentCommand::Scan(args) => cmd_content::scan_command(args),
        },
        Some(Command::Mods(args)) => cmd_mods::mods_command(args),
        Some(Command::Assets(assets)) => cmd_assets::assets_command(assets),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}
