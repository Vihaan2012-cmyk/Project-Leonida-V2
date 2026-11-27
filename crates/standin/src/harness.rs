//! Runs a built image the way `leonida run` runs the game: the shims are told where the package and `Output/`
//! are, the image is loaded with its imports bound and sealed, the loader shims learn which module is the
//! program, and the entry point is called. Here the entry runs on its own thread so a hang is a timeout.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use pe_loader::{LoadError, LoadOptions, Placement};
use shims::Binder;
use shims::audio::OutputMode;
use shims::gameinput::RawInput;

use crate::emit::BuildError;
use crate::image::Built;
use crate::script::{FAILED, PASSED, Stage};
use crate::steps::{TITLE_ID, package_file_bytes};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("loading the image: {0}")]
    Load(#[from] LoadError),
    #[error("setting up the runtime: {0}")]
    Setup(String),
    #[error("the entry point did not return within {0:?}")]
    Timeout(Duration),
    #[error("the entry point's thread panicked")]
    Panicked,
}

/// The folders the program sees: a package with `MicrosoftGame.Config` and one data file, and `Output/`.
pub struct Environment {
    _root: tempfile::TempDir,
    pub package: PathBuf,
    pub output: PathBuf,
}

impl Environment {
    /// Makes the folders below `under`, which must be on a drive with a letter.
    pub fn new(under: &Path) -> std::io::Result<Environment> {
        std::fs::create_dir_all(under)?;
        let root = tempfile::tempdir_in(under)?;
        let top = std::path::absolute(root.path())?;
        let package = top.join("Package");
        let output = top.join("Output");
        std::fs::create_dir_all(package.join("data"))?;
        std::fs::create_dir_all(&output)?;
        std::fs::write(
            package.join("MicrosoftGame.Config"),
            format!(
                "<Game><Identity Name=\"Standin.Title\" Version=\"1.0.0.0\" />\
                 <TitleId>{TITLE_ID:08X}</TitleId></Game>"
            ),
        )?;
        std::fs::write(package.join("data").join("boot.bin"), package_file_bytes())?;
        Ok(Environment {
            _root: root,
            package,
            output,
        })
    }
}

/// How to run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub timeout: Duration,
    pub placement: Placement,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            timeout: Duration::from_secs(60),
            placement: Placement::Anywhere,
        }
    }
}

/// The step that failed and what it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub id: u32,
    pub stage: Option<Stage>,
    pub name: Option<&'static str>,
    /// `rax` when the check failed: the value that was wrong.
    pub detail: u64,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "step {:#x} ({} stage: {}) failed with {:#x}",
            self.id,
            self.stage.map_or("?", Stage::name),
            self.name.unwrap_or("?"),
            self.detail
        )
    }
}

/// What a run left behind: the entry's return value and a copy of the image's data.
pub struct Report {
    pub exit: u32,
    pub completed: u32,
    pub failed: Option<Failure>,
    /// Where the image was loaded.
    pub base: usize,
    data_start: u32,
    data: Vec<u8>,
    symbols: HashMap<String, u32>,
}

impl Report {
    /// Whether the entry reported that every step passed.
    pub fn passed(&self) -> bool {
        self.exit == PASSED | self.completed && self.failed.is_none()
    }

    fn at(&self, name: &str) -> usize {
        let rva = self
            .symbols
            .get(name)
            .unwrap_or_else(|| panic!("the image declares no data named {name}"));
        (*rva - self.data_start) as usize
    }

    pub fn u32(&self, name: &str) -> u32 {
        let at = self.at(name);
        u32::from_le_bytes(self.data[at..at + 4].try_into().expect("four bytes"))
    }

    pub fn u64(&self, name: &str) -> u64 {
        let at = self.at(name);
        u64::from_le_bytes(self.data[at..at + 8].try_into().expect("eight bytes"))
    }

    pub fn bytes(&self, name: &str, length: usize) -> &[u8] {
        let at = self.at(name);
        &self.data[at..at + length]
    }
}

static RUNNING: Mutex<()> = Mutex::new(());

/// Sets the runtime up as `leonida run` does, loads the image and calls its entry.
pub fn run(
    built: &Built,
    environment: &Environment,
    options: &RunOptions,
) -> Result<Report, RunError> {
    let _one_at_a_time = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    shims::loader::configure(
        Some(environment.package.clone()),
        environment.output.clone(),
    );
    if !shims::files::configure(environment.package.clone(), environment.output.clone()) {
        return Err(RunError::Setup(
            "the package and output folders need drive letters".into(),
        ));
    }
    shims::files::set_redirect(None);
    shims::presentation::set(shims::presentation::Presentation::default());
    shims::audio::set_output_mode(OutputMode::Off);
    shims::gameinput::set_config(shims::gameinput::InputConfig::default());
    shims::gameinput::submit_input(RawInput::Focus(true));
    shims::gameinput::submit_input(RawInput::Key {
        virtual_key: 0x0D,
        scan_code: 28,
        down: true,
    });

    let mut binder = Binder::new(&[]);
    let load = LoadOptions {
        placement: options.placement,
        helpers: Some(environment.output.join("runtime")),
    };
    let module = pe_loader::load(&built.bytes, &mut binder, &load)?;
    binder.seal().map_err(RunError::Setup)?;
    shims::trace::set_image(module.base(), module.size());
    shims::loader::set_main_module(module.base());

    let entry = module.entry();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("standin-entry".into())
        .stack_size(8 << 20)
        .spawn(move || {
            // SAFETY: the entry point of the image just loaded, a function taking no arguments; the module outlives
            // the thread unless the wait below times out, in which case the module is leaked.
            let call: extern "win64" fn() -> usize = unsafe { std::mem::transmute(entry) };
            let _ = send.send(call() as u32);
        })
        .map_err(|e| RunError::Setup(e.to_string()))?;
    let exit = match receive.recv_timeout(options.timeout) {
        Ok(exit) => exit,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            std::mem::forget(module);
            std::mem::forget(binder);
            return Err(RunError::Timeout(options.timeout));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = thread.join();
            std::mem::forget(module);
            std::mem::forget(binder);
            return Err(RunError::Panicked);
        }
    };
    let _ = thread.join();

    let (start, length) = built.data_span;
    // SAFETY: the data section of the loaded image: `length` readable bytes at this offset.
    let data = unsafe {
        std::slice::from_raw_parts(
            (module.base() + start as usize) as *const u8,
            length as usize,
        )
    }
    .to_vec();
    let symbols: HashMap<String, u32> = built
        .symbol_names()
        .map(|name| (name.to_string(), built.symbol(name).expect("listed")))
        .collect();
    let mut report = Report {
        exit,
        completed: 0,
        failed: None,
        base: module.base(),
        data_start: start,
        data,
        symbols,
    };
    report.completed = report.u32("completed");
    let failed = report.u32("failed");
    if failed != 0 {
        let info = built.step(failed);
        report.failed = Some(Failure {
            id: failed,
            stage: info.map(|s| s.stage),
            name: info.map(|s| s.name),
            detail: report.u64("detail"),
        });
    }
    debug_assert!(report.failed.is_none() || report.exit == FAILED | failed);
    drop(module);
    drop(binder);
    Ok(report)
}
