//! What each import of a loaded executable is bound to: one of our shims, the same function in an allowed
//! Windows DLL, or a stand-in that records the call and fails, for everything the offline policy blocks and
//! everything nobody provides yet. With tracing on, every import also counts its calls.
#![allow(unsafe_code)]

use std::sync::Mutex;

use pe_loader::{ImportRef, Resolver};

pub mod audio;
pub mod bvh_builder;
#[cfg(windows)]
mod code;
#[cfg(windows)]
pub mod constants;
pub mod d3d12;
pub mod dstorage;
pub mod fidelityfx;
pub mod files;
pub mod gameinput;
pub mod loader;
pub mod media;
#[cfg(windows)]
pub mod network;
pub mod presentation;
pub mod threads;
pub mod trace;
pub mod xg;
pub mod xmem;

/// A function we provide in place of a console one.
#[derive(Debug, Clone, Copy)]
pub struct Shim {
    pub dll: &'static str,
    pub name: &'static str,
    pub address: usize,
}

/// The shims built into Leonida.
pub fn builtin() -> Vec<Shim> {
    let mut shims = xmem::shims();
    shims.extend(d3d12::shims());
    shims.extend(audio::shims());
    shims.extend(bvh_builder::shims());
    shims.extend(dstorage::shims());
    shims.extend(fidelityfx::shims());
    shims.extend(files::shims());
    shims.extend(gameinput::shims());
    shims.extend(loader::shims());
    shims.extend(media::shims());
    #[cfg(windows)]
    shims.extend(network::shims());
    shims.extend(threads::shims());
    shims.extend(xg::shims());
    shims
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Shim,
    Host,
    Blocked(offline::Reason),
    Missing,
}

const HOST_DLLS: &[&str] = &[
    "kernel32.dll",
    "kernelbase.dll",
    "ntdll.dll",
    "user32.dll",
    "advapi32.dll",
    "bcrypt.dll",
    "ole32.dll",
    "ucrtbase.dll",
    "mfplat.dll",
    "mfreadwrite.dll",
];

const HOST_PREFIXES: &[&str] = &[
    "api-ms-win-core-",
    "api-ms-win-crt-",
    "ext-ms-win-rtcore-ntuser-",
    "vcruntime140",
    "msvcp140",
];

fn lower(dll: &str) -> String {
    let name = dll
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(dll)
        .to_ascii_lowercase();
    if name.ends_with(".dll") {
        name
    } else {
        format!("{name}.dll")
    }
}

/// Whether a DLL may come from Windows itself.
pub fn host_allowed(dll: &str) -> bool {
    let name = lower(dll);
    HOST_DLLS.contains(&name.as_str()) || HOST_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// A call to an import nobody provides or the offline policy blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trap {
    pub dll: String,
    pub function: String,
    pub blocked: Option<offline::Reason>,
}

static TRAPS: Mutex<Vec<Trap>> = Mutex::new(Vec::new());
static CALLS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// The stand-ins called so far, in order, and forgets them.
pub fn take_calls() -> Vec<Trap> {
    let calls = std::mem::take(&mut *CALLS.lock().expect("no panic while held"));
    let traps = TRAPS.lock().expect("no panic while held");
    calls
        .iter()
        .filter_map(|i| traps.get(*i).cloned())
        .collect()
}

extern "win64" fn trapped(index: usize) -> usize {
    if let Ok(mut calls) = CALLS.lock() {
        calls.push(index);
    }
    0
}

fn function_name(import: &ImportRef<'_>) -> String {
    match (import.name, import.ordinal) {
        (Some(name), _) => name.to_string(),
        (None, Some(ordinal)) => format!("#{ordinal}"),
        (None, None) => "?".to_string(),
    }
}

/// Binds imports for one loaded image.
pub struct Binder {
    shims: Vec<Shim>,
    trace: Option<trace::Trace>,
    #[cfg(windows)]
    code: code::Code,
}

impl Binder {
    pub fn new(extra: &[Shim]) -> Binder {
        let mut shims = builtin();
        shims.extend_from_slice(extra);
        Binder {
            shims,
            trace: None,
            #[cfg(windows)]
            code: code::Code::default(),
        }
    }

    /// Counts every import's calls and records the files the program names; see [`trace`].
    pub fn traced(mut self) -> Binder {
        self.trace = Some(trace::Trace::new());
        self
    }

    pub fn trace(&self) -> Option<&trace::Trace> {
        self.trace.as_ref()
    }

    fn shim(&self, import: &ImportRef<'_>) -> Option<usize> {
        let dll = lower(import.dll);
        let name = function_name(import);
        self.shims
            .iter()
            .find(|s| lower(s.dll) == dll && s.name == name)
            .map(|s| s.address)
    }

    /// How an import would be bound, without binding it. An import the offline policy names is blocked even
    /// where a shim answers it: the shim only refuses.
    pub fn classify(&self, import: &ImportRef<'_>) -> Binding {
        if let Some(reason) = offline::import(import.dll, import.name) {
            return Binding::Blocked(reason);
        }
        if self.shim(import).is_some() {
            return Binding::Shim;
        }
        #[cfg(windows)]
        if host_allowed(import.dll) && host::find(import).is_some() {
            return Binding::Host;
        }
        Binding::Missing
    }

    /// Makes the stand-ins and trace counters callable. Call after loading and before running anything.
    pub fn seal(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.code.seal()
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }

    fn bind(&mut self, import: &ImportRef<'_>) -> usize {
        if let Some(address) = self.shim(import) {
            return address;
        }
        let blocked = offline::import(import.dll, import.name);
        #[cfg(windows)]
        if blocked.is_none()
            && host_allowed(import.dll)
            && let Some(address) = host::find(import)
        {
            return match (&self.trace, trace::path_argument(import)) {
                (Some(_), Some(wide)) => self.code.recorder(address, wide).unwrap_or(address),
                _ => address,
            };
        }
        let mut traps = TRAPS.lock().expect("no panic while held");
        let index = traps.len();
        traps.push(Trap {
            dll: import.dll.to_string(),
            function: function_name(import),
            blocked,
        });
        drop(traps);
        #[cfg(windows)]
        {
            self.code.trap(index).unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            let _ = index;
            0
        }
    }
}

impl Resolver for Binder {
    fn resolve(&mut self, import: &ImportRef<'_>) -> usize {
        let address = self.bind(import);
        #[cfg(windows)]
        if let Some(trace) = &mut self.trace
            && let Some((index, counter, last)) = trace.add(import)
        {
            return self
                .code
                .counter(index, counter, last, address)
                .unwrap_or(address);
        }
        address
    }
}

#[cfg(windows)]
mod host {
    use pe_loader::ImportRef;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    pub fn find(import: &ImportRef<'_>) -> Option<usize> {
        let wide: Vec<u16> = import.dll.encode_utf16().chain([0]).collect();
        // SAFETY: loads a Windows DLL the allow-list names and asks it for one export.
        unsafe {
            let module = LoadLibraryW(wide.as_ptr());
            if module.is_null() {
                return None;
            }
            let found = match (import.name, import.ordinal) {
                (Some(name), _) => {
                    let name: Vec<u8> = name.bytes().chain([0]).collect();
                    GetProcAddress(module, name.as_ptr())
                }
                (None, Some(ordinal)) => GetProcAddress(module, ordinal as usize as *const u8),
                (None, None) => None,
            };
            found.map(|f| f as usize)
        }
    }
}
