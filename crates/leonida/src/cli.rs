use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Builds and runs the Leonida PC port from your own copy of the game.
#[derive(Debug, Parser)]
#[command(name = "leonida", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Work with the game's shaders.
    Shaders(ShadersArgs),
}

#[derive(Debug, Args)]
pub struct ShadersArgs {
    #[command(subcommand)]
    pub command: ShadersCommand,
}

#[derive(Debug, Subcommand)]
pub enum ShadersCommand {
    /// Read every shader container in shaders.rpf and report what it holds. Writes nothing.
    Scan(ScanArgs),
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
}
