//! The boot steps, one file per part of the runtime, in the order the game boots.

pub mod audio;
pub mod files;
pub mod graphics;
pub mod input;
pub mod loader;
pub mod memory;
pub mod threads;

use crate::script::{Script, Stage};

/// Appends the steps of one stage.
pub fn add(script: Script, stage: Stage) -> Script {
    match stage {
        Stage::Loader => loader::add(script),
        Stage::Memory => memory::add(script),
        Stage::Files => files::add(script),
        Stage::Threads => threads::add(script),
        Stage::Input => input::add(script),
        Stage::Audio => audio::add(script),
        Stage::Device => graphics::add_device(script),
        Stage::Present => graphics::add_present(script),
    }
}

/// The boot of one stage on its own. The present stage needs the device it presents from, so it includes the
/// device stage's steps.
pub fn stage(stage: Stage) -> Script {
    let first = Script::new();
    match stage {
        Stage::Present => add(add(first, Stage::Device), Stage::Present),
        other => add(first, other),
    }
}

/// The whole boot, in the game's order.
pub fn boot() -> Script {
    Stage::ALL
        .into_iter()
        .fold(Script::new(), |script, stage| add(script, stage))
}

pub const LIBRARY_LOADER: &str = "api-ms-win-core-libraryloader-l1-2-0.dll";
pub const LIBRARY_LOADER_W: &str = "api-ms-win-core-libraryloader-l1-2-1.dll";
pub const FILE: &str = "api-ms-win-core-file-l1-1-0.dll";
pub const HANDLE: &str = "api-ms-win-core-handle-l1-1-0.dll";
pub const MEMORY: &str = "api-ms-win-core-memory-l1-1-0.dll";
pub const SYNCH: &str = "api-ms-win-core-synch-l1-1-0.dll";
pub const SLEEP: &str = "api-ms-win-core-synch-l1-2-0.dll";
pub const PROCESS_THREADS: &str = "api-ms-win-core-processthreads-l1-1-0.dll";
pub const PROCESS_TOPOLOGY: &str = "api-ms-win-core-processtopology-l1-1-0.dll";
pub const RTL_SUPPORT: &str = "api-ms-win-core-rtlsupport-l1-1-0.dll";
pub const SYSINFO: &str = "api-ms-win-core-sysinfo-l1-1-0.dll";

/// Where the package folder's test file lies, as the game names it.
pub const PACKAGE_FILE: &str = r"G:\data\boot.bin";
/// What the file holds: a counter, so a read that lands anywhere else shows.
pub fn package_file_bytes() -> Vec<u8> {
    (0..4096u32)
        .flat_map(|i| (i ^ 0x5A5A_1234).to_le_bytes())
        .collect()
}

/// The `XSystemImpl` and `XGameImpl` class ids the GDK runtime serves.
pub const XSYSTEM_CLASS: &str = "e349bd1a-fc20-4e40-b99c-4178cc6b409f";
pub const XGAME_CLASS: &str = "973a344e-24bf-4d0f-8457-56c534892b29";
/// The title id the stand-in's `MicrosoftGame.Config` carries.
pub const TITLE_ID: u32 = 0x7952_987A;
