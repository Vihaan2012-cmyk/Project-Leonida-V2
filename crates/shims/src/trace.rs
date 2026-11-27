//! What `leonida run --trace` records: how often each import is called and which was called last, every file
//! name the program hands to Windows, notes from other parts of the runtime (such as unknown graphics
//! methods), and the address of a crash.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use pe_loader::ImportRef;

const CAPACITY: usize = 1 << 14;

static FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static IMAGE: Mutex<(usize, usize)> = Mutex::new((0, 0));

/// Where the program's image lies, so addresses can be given relative to it.
pub fn set_image(base: usize, size: usize) {
    if let Ok(mut image) = IMAGE.lock() {
        *image = (base, size);
    }
}

/// An address, with its offset in the image when it lies inside.
pub fn image_offset(address: usize) -> String {
    let (base, size) = IMAGE.lock().map(|i| *i).unwrap_or((0, 0));
    if size != 0 && (base..base + size).contains(&address) {
        format!("{address:#x} (image offset {:#x})", address - base)
    } else {
        format!("{address:#x}")
    }
}
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Call counts for one image's imports. The counters live as long as the process: generated code points at
/// them.
pub struct Trace {
    counters: &'static [AtomicU64],
    last: &'static AtomicU64,
    names: Vec<String>,
}

impl Default for Trace {
    fn default() -> Self {
        Self::new()
    }
}

impl Trace {
    pub fn new() -> Trace {
        let counters: Vec<AtomicU64> = (0..CAPACITY).map(|_| AtomicU64::new(0)).collect();
        Trace {
            counters: Box::leak(counters.into_boxed_slice()),
            last: Box::leak(Box::new(AtomicU64::new(u64::MAX))),
            names: Vec::new(),
        }
    }

    /// Registers an import; gives its index and the addresses its counter code writes.
    pub fn add(&mut self, import: &ImportRef<'_>) -> Option<(u32, usize, usize)> {
        let index = self.names.len();
        let counter = self.counters.get(index)?;
        self.names
            .push(format!("{}!{}", import.dll, super::function_name(import)));
        Some((
            index as u32,
            counter as *const AtomicU64 as usize,
            self.last as *const AtomicU64 as usize,
        ))
    }

    pub fn calls(&self, name: &str) -> u64 {
        self.names
            .iter()
            .position(|n| n == name)
            .map_or(0, |i| self.counters[i].load(Ordering::Relaxed))
    }

    pub fn last(&self) -> Option<&str> {
        let index = self.last.load(Ordering::Relaxed);
        self.names
            .get(usize::try_from(index).ok()?)
            .map(String::as_str)
    }

    /// Every import that was called, most calls first, then the last one called.
    pub fn report(&self) -> String {
        let mut called: Vec<(u64, &str)> = self
            .names
            .iter()
            .enumerate()
            .map(|(i, name)| (self.counters[i].load(Ordering::Relaxed), name.as_str()))
            .filter(|(count, _)| *count > 0)
            .collect();
        called.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        let mut text = String::new();
        for (count, name) in &called {
            let _ = writeln!(text, "{count:>10}  {name}");
        }
        let _ = writeln!(
            text,
            "{} of {} imports called; last: {}",
            called.len(),
            self.names.len(),
            self.last().unwrap_or("none")
        );
        text
    }

    /// Writes `imports.txt`, `files.txt` and `notes.txt` into `folder`.
    pub fn write(&self, folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        std::fs::write(folder.join("imports.txt"), self.report())?;
        std::fs::write(folder.join("files.txt"), files().join("\n"))?;
        std::fs::write(folder.join("notes.txt"), notes().join("\n"))
    }
}

const WIDE_PATHS: &[&str] = &[
    "CreateFileW",
    "CreateFile2",
    "CreateDirectoryW",
    "DeleteFileW",
    "FindFirstFileW",
    "FindFirstFileExW",
    "GetFileAttributesW",
    "GetFileAttributesExW",
    "RemoveDirectoryW",
    "SetFileAttributesW",
];

const NARROW_PATHS: &[&str] = &[
    "CreateFileA",
    "GetFileAttributesA",
    "GetFileAttributesExA",
    "FindFirstFileA",
];

/// Whether an import's first argument is a file name: `Some(true)` for UTF-16, `Some(false)` for 8-bit.
pub fn path_argument(import: &ImportRef<'_>) -> Option<bool> {
    let name = import.name?;
    if WIDE_PATHS.contains(&name) {
        Some(true)
    } else if NARROW_PATHS.contains(&name) {
        Some(false)
    } else {
        None
    }
}

/// Called from generated code with the file name argument of an import.
/// # Safety
/// `path` is null or a terminated string, UTF-16 when `wide` is not 0.
pub(crate) unsafe extern "win64" fn record_path(path: *const u8, wide: u32) {
    if path.is_null() {
        return;
    }
    let text = if wide != 0 {
        let path = path as *const u16;
        // SAFETY: the program passes a terminated UTF-16 string, as the import requires; reading stops at
        // the terminator or 32767 characters.
        let length = (0..32767)
            .take_while(|i| unsafe { *path.add(*i) } != 0)
            .count();
        // SAFETY: as above, `length` characters are readable.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, length) })
    } else {
        // SAFETY: as above, for an 8-bit string.
        let length = (0..32767)
            .take_while(|i| unsafe { *path.add(*i) } != 0)
            .count();
        // SAFETY: as above.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(path, length) }).into_owned()
    };
    if let Ok(mut files) = FILES.lock() {
        files.push(text);
    }
}

/// File names the program has handed to Windows so far, in order.
pub fn files() -> Vec<String> {
    FILES.lock().map(|f| f.clone()).unwrap_or_default()
}

/// Records something worth knowing, such as a graphics method nobody implements.
pub fn note(text: String) {
    if let Ok(mut notes) = NOTES.lock() {
        notes.push(text);
    }
}

pub fn notes() -> Vec<String> {
    NOTES.lock().map(|n| n.clone()).unwrap_or_default()
}

#[cfg(windows)]
mod crash {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };

    pub(super) static WATCH: Mutex<Option<(PathBuf, usize, usize)>> = Mutex::new(None);

    const FATAL: [u32; 6] = [
        0xC000_0005,
        0xC000_001D,
        0xC000_0096,
        0xC000_00FD,
        0xC000_0094,
        0xC000_0409,
    ];

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows passes valid exception pointers to vectored handlers.
        let record = unsafe { &*(*info).ExceptionRecord };
        let code = record.ExceptionCode as u32;
        if !FATAL.contains(&code) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let address = record.ExceptionAddress as usize;
        if let Ok(watch) = WATCH.try_lock()
            && let Some((folder, base, size)) = watch.as_ref()
        {
            let place = if (*base..base + size).contains(&address) {
                format!("{address:#x} (image offset {:#x})", address - base)
            } else {
                format!("{address:#x} (outside the image)")
            };
            let text = format!("exception {code:#010x} at {place}\n");
            let _ = std::fs::write(folder.join("crash.txt"), text);
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    pub fn install() {
        // SAFETY: registers a handler that only reads the exception record and writes a file.
        unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    }
}

/// Writes `crash.txt` into `folder` with the exception and its address, relative to the image at
/// `base..base + size` when it lies inside, if the program crashes.
pub fn watch_crashes(folder: PathBuf, base: usize, size: usize) {
    #[cfg(windows)]
    {
        let first = crash::WATCH
            .lock()
            .map(|mut watch| watch.replace((folder, base, size)).is_none())
            .unwrap_or(false);
        if first {
            crash::install();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (folder, base, size);
    }
}
