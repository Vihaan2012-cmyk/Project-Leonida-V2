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
    /// Run the game executable, or with --dry-run only show how its imports would be provided.
    Run(RunArgs),
    /// Look at every file in the game folder for signs of tampering, without running or loading anything.
    Triage(TriageArgs),
    /// The nudity filter: find what to hide.
    Content(ContentArgs),
}

#[derive(Debug, Args)]
pub struct ContentArgs {
    #[command(subcommand)]
    pub command: ContentCommand,
}

#[derive(Debug, Subcommand)]
pub enum ContentCommand {
    /// Find the resources that show nudity and write the blocklist to OUTPUT. Reads archives, changes nothing.
    Scan(ContentScanArgs),
}

#[derive(Debug, Args)]
pub struct ContentScanArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// An archive to read, relative to the game folder. Repeat for several. Default: packs.rpf and
    /// pack/core_restofworld/core_restofworld.rpf (levels.rpf adds the mannequins and takes much longer).
    #[arg(long = "archive")]
    pub archives: Vec<String>,
    /// Start reading at this byte offset instead of the start of the data area, for a quick sample.
    #[arg(long)]
    pub start: Option<u64>,
    /// Only read this many bytes of each archive, for a quick sample.
    #[arg(long)]
    pub limit: Option<u64>,
    /// Decoded ped catalogues (JSON from a catalogue decoder) to derive replacements and texture rules from.
    #[arg(long)]
    pub catalogues: Option<PathBuf>,
    /// Where Leonida writes what it makes. The blocklist goes to <OUTPUT>/content-blocklist.json.
    #[arg(long, default_value = "Output")]
    pub output: PathBuf,
    /// Leonida.toml, to report whether nudity is set to block or allow.
    #[arg(long, default_value = "Config/leonida.toml")]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct TriageArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Write the report here instead of printing it.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Another file in the game folder to look at instead of the executable, such as one of its DLLs.
    #[arg(long)]
    pub file: Option<String>,
    /// List how every import would be provided, without running anything.
    #[arg(long)]
    pub dry_run: bool,
    /// With --dry-run, also list every function each DLL provides.
    #[arg(long)]
    pub all: bool,
    /// Record every import called, every file named, every graphics method missing and any crash, in
    /// OUTPUT/boot.
    #[arg(long)]
    pub trace: bool,
    /// Where Leonida writes what it makes while running.
    #[arg(long, default_value = "Output")]
    pub output: PathBuf,
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
