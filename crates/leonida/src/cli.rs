use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Builds and runs the Leonida PC port from your own copy of the game. With no command, opens the window.
#[derive(Debug, Parser)]
#[command(name = "leonida", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the Leonida window.
    Ui,
    /// Work with the game's shaders.
    Shaders(ShadersArgs),
    /// Read the game executable's structure and code without running it.
    Exe(ExeArgs),
    /// Check that every game file is there and the right size; with --full, that its contents are right too.
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Also read every file and compare it with the known copy. Slow: it reads the whole game.
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct ExeArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long, global = true)]
    pub game: Option<PathBuf>,
    #[command(subcommand)]
    pub command: ExeCommand,
}

#[derive(Debug, Subcommand)]
pub enum ExeCommand {
    /// Headers, sections and table sizes.
    Info,
    /// Every imported function, by DLL.
    Imports {
        /// Only DLLs whose name contains this text.
        #[arg(long)]
        dll: Option<String>,
    },
    /// Everything the executable could use to reach the network. Leonida blocks all of it.
    Network,
    /// Functions from the exception table, largest first.
    Functions {
        /// How many to list.
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
    /// Disassemble the function containing an address (hex RVA or virtual address).
    Disasm {
        address: String,
        /// Most instructions to print.
        #[arg(long, default_value_t = 400)]
        limit: usize,
    },
    /// C-like pseudocode for the function containing an address (hex RVA or virtual address).
    Decompile {
        address: String,
        /// Also decompile the functions it calls, this many levels deep.
        #[arg(long, default_value_t = 0)]
        depth: usize,
    },
    /// Code that calls, jumps to or references an address, an import or a string.
    Xrefs {
        /// A hex RVA or virtual address.
        address: Option<String>,
        /// An imported function's name, e.g. CreateFileW.
        #[arg(long)]
        import: Option<String>,
        /// Text of a string in the data sections.
        #[arg(long)]
        string: Option<String>,
        /// Most references to print.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Readable text in the executable's data sections.
    Strings {
        /// Only strings containing this text.
        #[arg(long)]
        find: Option<String>,
        /// Shortest string to report.
        #[arg(long, default_value_t = 8)]
        min: usize,
        /// Most strings to print.
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
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
    /// Print one shader's GPU instructions.
    Disasm(DisasmArgs),
    /// Check how much of every shader the translator handles, and list what it can't yet. Writes nothing.
    Report(ReportArgs),
    /// Translate every shader and have the graphics driver build it into a pipeline. Writes nothing.
    Verify(VerifyArgs),
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Stop after this many shader containers.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Only containers of this stage: ps, vs, gs, ms or cs.
    #[arg(long)]
    pub stage: Option<String>,
    /// Only containers whose name contains this text.
    #[arg(long)]
    pub name: Option<String>,
    /// Write each translated program into this folder.
    #[arg(long)]
    pub save_spirv: Option<PathBuf>,
    /// Build this SPIR-V file in place of each translated program.
    #[arg(long)]
    pub use_spirv: Option<PathBuf>,
    /// Only translate; build no pipelines.
    #[arg(long)]
    pub no_build: bool,
    /// Print each shader with its size and how long the driver took.
    #[arg(long)]
    pub each: bool,
    /// How many shaders the driver builds at once. Each large shader can take several gigabytes to build.
    #[arg(long, default_value_t = 1)]
    pub jobs: usize,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// How many missing instructions to list.
    #[arg(long, default_value_t = 40)]
    pub top: usize,
    /// Also print one example of each missing instruction.
    #[arg(long)]
    pub examples: bool,
}

#[derive(Debug, Args)]
pub struct DisasmArgs {
    /// The shader's name, as shown by the scan or the shader report.
    pub name: String,
    /// Which of the containers with this name, counting from 1.
    #[arg(long, default_value_t = 1)]
    pub nth: usize,
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
}
