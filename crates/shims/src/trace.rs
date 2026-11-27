//! What `leonida run --trace` records: how often each import is called and which was called last, every file
//! name the program hands to Windows, notes from other parts of the runtime (such as unknown graphics
//! methods), and the stops: crashes, failed checks, C++ exceptions and exits, with a summary of how far the
//! program got.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pe_loader::ImportRef;

#[cfg(windows)]
mod crash;

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

    /// Writes `imports.txt`, `files.txt`, `summary.txt` and `notes.txt` into `folder`.
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

/// What the crash handler knows about a stop, for an explainer to turn into words.
#[derive(Debug, Clone, Default)]
pub struct CrashFacts {
    pub code: u32,
    pub address: usize,
    /// The address as an offset in the image, when it lies inside.
    pub rva: Option<u32>,
    /// The start of the function containing the address, as an image offset.
    pub function_rva: Option<u32>,
    pub registers: Vec<(&'static str, u64)>,
    pub frames: Vec<usize>,
    pub image_base: usize,
    pub image_size: usize,
    /// The 48 bytes before the address and the 16 at it, when readable.
    pub code_before: Vec<u8>,
    pub code_at: Vec<u8>,
}

impl CrashFacts {
    /// The offset of `address` in the image, when it lies inside.
    pub fn offset(&self, address: usize) -> Option<u32> {
        let image = self.image_base..self.image_base + self.image_size;
        (self.image_size != 0 && image.contains(&address))
            .then(|| (address - self.image_base) as u32)
    }

    pub fn describe(&self, address: usize) -> String {
        match self.offset(address) {
            Some(offset) => format!("{address:#x} (image offset {offset:#x})"),
            None => format!("{address:#x} (outside the image)"),
        }
    }

    pub fn register(&self, name: &str) -> Option<u64> {
        self.registers.iter().find(|r| r.0 == name).map(|r| r.1)
    }
}

/// An explainer's answer: lines for the report and the part of the program a stop belongs to.
#[derive(Debug, Clone, Default)]
pub struct Explanation {
    /// Replaces the generic headline of the stop.
    pub title: Option<String>,
    pub lines: Vec<String>,
    pub subsystem: Option<String>,
}

pub type Explainer = fn(&CrashFacts) -> Explanation;

/// One recorded stop.
#[derive(Debug, Clone)]
pub struct Stop {
    pub headline: String,
    pub lines: Vec<String>,
    pub subsystem: Option<String>,
    /// Image offsets of the first frames in the image.
    pub frames: Vec<u32>,
}

static STOPS: Mutex<Vec<Stop>> = Mutex::new(Vec::new());
static CXX: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CXX_COUNT: AtomicU64 = AtomicU64::new(0);
static VIEW: Mutex<Option<View>> = Mutex::new(None);
static WATCHED_FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

struct View {
    names: Arc<Vec<String>>,
    counters: &'static [AtomicU64],
    last: &'static AtomicU64,
}

/// Stops so far, in order.
pub fn stops() -> Vec<Stop> {
    STOPS.lock().map(|s| s.clone()).unwrap_or_default()
}

/// The most recent C++ exceptions the program threw (often caught again), oldest first.
pub fn cxx_exceptions() -> Vec<String> {
    CXX.lock().map(|c| c.clone()).unwrap_or_default()
}

#[cfg(windows)]
pub(crate) fn push_stop(stop: Stop) {
    if let Ok(mut stops) = STOPS.lock()
        && stops.len() < 64
    {
        stops.push(stop);
    }
}

#[cfg(windows)]
pub(crate) fn push_cxx(what: String, facts: &CrashFacts) {
    let count = CXX_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let at: Vec<String> = facts
        .frames
        .iter()
        .filter_map(|f| facts.offset(*f))
        .take(3)
        .map(|o| format!("{o:#x}"))
        .collect();
    let line = format!("#{count} {what:?} from image offsets [{}]", at.join(", "));
    if let Ok(mut list) = CXX.lock() {
        if list.len() >= 12 {
            list.remove(0);
        }
        list.push(line);
    }
}

#[cfg(windows)]
pub(crate) fn push_exit() {
    push_stop(Stop {
        headline: "the process is exiting (exit() or a normal return)".into(),
        lines: Vec::new(),
        subsystem: None,
        frames: Vec::new(),
    });
    if let Some(folder) = WATCHED_FOLDER.lock().ok().and_then(|f| f.clone()) {
        write_summary(&folder);
    }
}

/// The part of the program an import's DLL belongs to.
pub fn subsystem_of_dll(dll: &str) -> &'static str {
    let dll = dll.to_ascii_lowercase();
    let has = |part: &str| dll.contains(part);
    if has("d3d12") || has("xg_xs") || has("xboxbvh") || has("fidelityfx") {
        "graphics"
    } else if has("xapu") || has("acphal") || has("bink") || has("xaudio") || has("mf") {
        "audio and video"
    } else if has("gameinput") {
        "input"
    } else if has("httpclient") || has("winhttp") || has("ws2_32") {
        "online services (blocked offline)"
    } else if has("xmem") {
        "memory"
    } else if has("ntuser") {
        "window and messages"
    } else if has("api-ms-win-crt") || has("msvcp") || has("vcruntime") {
        "C runtime"
    } else {
        "system services"
    }
}

/// `summary.txt`: how the run ended and the last part of the program it reached.
pub fn summary_text() -> String {
    let mut text = String::new();
    let stops = stops();
    match stops.last() {
        Some(stop) => {
            let _ = writeln!(text, "stopped: {}", stop.headline);
            for line in &stop.lines {
                let _ = writeln!(text, "  {line}");
            }
            if let Some(subsystem) = &stop.subsystem {
                let _ = writeln!(text, "subsystem of the stop: {subsystem}");
            }
            if !stop.frames.is_empty() {
                let frames: Vec<String> = stop.frames.iter().map(|f| format!("{f:#x}")).collect();
                let _ = writeln!(text, "image offsets on the stack: {}", frames.join(" "));
            }
        }
        None => {
            let _ = writeln!(
                text,
                "no stop recorded: still running, or ended without a trace"
            );
        }
    }
    if stops.len() > 1 {
        let _ = writeln!(text, "{} stops in all, see crash.txt", stops.len());
    }
    if let Ok(view) = VIEW.lock()
        && let Some(view) = view.as_ref()
    {
        let called = view
            .counters
            .iter()
            .take(view.names.len())
            .filter(|c| c.load(Ordering::Relaxed) > 0)
            .count();
        let last = usize::try_from(view.last.load(Ordering::Relaxed))
            .ok()
            .and_then(|i| view.names.get(i));
        match last {
            Some(name) => {
                let dll = name.split('!').next().unwrap_or("");
                let _ = writeln!(
                    text,
                    "last import called: {name} (reached: {})",
                    subsystem_of_dll(dll)
                );
            }
            None => {
                let _ = writeln!(text, "no import called yet");
            }
        }
        let _ = writeln!(text, "{called} of {} imports called", view.names.len());
    }
    let reports: Vec<String> = notes()
        .into_iter()
        .filter(|n| n.starts_with("XErrorReport("))
        .collect();
    if let Some(last) = reports.last() {
        let _ = writeln!(text, "XErrorReport: {} noted, last {last}", reports.len());
    }
    if let Some(note) = notes().last() {
        let _ = writeln!(text, "last note: {note}");
    }
    let thrown = cxx_exceptions();
    if !thrown.is_empty() {
        let _ = writeln!(
            text,
            "C++ exceptions thrown ({} so far), latest last:",
            CXX_COUNT.load(Ordering::Relaxed)
        );
        for line in thrown.iter().rev().take(5).rev() {
            let _ = writeln!(text, "  {line}");
        }
    }
    text
}

pub fn write_summary(folder: &Path) {
    let _ = std::fs::create_dir_all(folder);
    let _ = std::fs::write(folder.join("summary.txt"), summary_text());
}

/// Gives the crash handler a way to explain stops inside the image.
pub fn set_explainer(explainer: Explainer) {
    #[cfg(windows)]
    if let Ok(mut slot) = crash::EXPLAINER.lock() {
        *slot = Some(explainer);
    }
    #[cfg(not(windows))]
    let _ = explainer;
}

/// Reads `length` bytes at `address` of this process, if all of them are readable.
pub fn read_memory(address: usize, length: usize) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        crash::read_memory(address, length)
    }
    #[cfg(not(windows))]
    {
        let _ = (address, length);
        None
    }
}

impl Trace {
    /// Lets `summary.txt` name the last import called.
    pub fn attach(&self) {
        if let Ok(mut view) = VIEW.lock() {
            *view = Some(View {
                names: Arc::new(self.names.clone()),
                counters: self.counters,
                last: self.last,
            });
        }
    }
}

/// Writes `crash.txt` and `summary.txt` into `folder` when the program stops: an exception, a failed check,
/// `abort` or `exit`. Addresses are given relative to the image at `base..base + size` when they lie inside.
pub fn watch_crashes(folder: PathBuf, base: usize, size: usize) {
    if let Ok(mut watched) = WATCHED_FOLDER.lock() {
        *watched = Some(folder.clone());
    }
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

/// The text of a crash report.
#[cfg(windows)]
pub fn format_report(facts: &CrashFacts, headline: &str, explanation: &[String]) -> String {
    crash::format_report(facts, headline, explanation)
}
