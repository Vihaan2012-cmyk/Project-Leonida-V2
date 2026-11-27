//! What each import of a loaded executable is bound to: one of our shims, the same function in an allowed
//! Windows DLL, or a stand-in that records the call and fails, for everything the offline policy blocks and
//! everything nobody provides yet.
#![allow(unsafe_code)]

use std::sync::Mutex;

use pe_loader::{ImportRef, Resolver};

mod xmem;

/// A function we provide in place of a console one.
#[derive(Debug, Clone, Copy)]
pub struct Shim {
    pub dll: &'static str,
    pub name: &'static str,
    pub address: usize,
}

/// The shims built into Leonida.
pub fn builtin() -> Vec<Shim> {
    xmem::shims()
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

/// Binds imports for one loaded image.
pub struct Binder {
    shims: Vec<Shim>,
    #[cfg(windows)]
    stubs: host::Stubs,
}

impl Binder {
    pub fn new(extra: &[Shim]) -> Binder {
        let mut shims = builtin();
        shims.extend_from_slice(extra);
        Binder {
            shims,
            #[cfg(windows)]
            stubs: host::Stubs::default(),
        }
    }

    fn shim(&self, import: &ImportRef<'_>) -> Option<usize> {
        let dll = lower(import.dll);
        let name = import.name?;
        self.shims
            .iter()
            .find(|s| lower(s.dll) == dll && s.name == name)
            .map(|s| s.address)
    }

    /// How an import would be bound, without binding it.
    pub fn classify(&self, import: &ImportRef<'_>) -> Binding {
        if self.shim(import).is_some() {
            return Binding::Shim;
        }
        if let Some(reason) = offline::import(import.dll, import.name) {
            return Binding::Blocked(reason);
        }
        #[cfg(windows)]
        if host_allowed(import.dll) && host::find(import).is_some() {
            return Binding::Host;
        }
        Binding::Missing
    }

    /// Makes the stand-ins callable. Call after loading and before running anything.
    pub fn seal(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.stubs.seal()
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }
}

impl Resolver for Binder {
    fn resolve(&mut self, import: &ImportRef<'_>) -> usize {
        if let Some(address) = self.shim(import) {
            return address;
        }
        let blocked = offline::import(import.dll, import.name);
        #[cfg(windows)]
        if blocked.is_none()
            && host_allowed(import.dll)
            && let Some(address) = host::find(import)
        {
            return address;
        }
        let function = match (import.name, import.ordinal) {
            (Some(name), _) => name.to_string(),
            (None, Some(ordinal)) => format!("#{ordinal}"),
            (None, None) => "?".to_string(),
        };
        let mut traps = TRAPS.lock().expect("no panic while held");
        let index = traps.len();
        traps.push(Trap {
            dll: import.dll.to_string(),
            function,
            blocked,
        });
        drop(traps);
        #[cfg(windows)]
        {
            self.stubs.stub(index).unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            let _ = index;
            0
        }
    }
}

#[cfg(windows)]
mod host {
    use std::ffi::c_void;

    use pe_loader::ImportRef;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualProtect,
    };

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

    const STUB_BYTES: usize = 22;
    const PAGE_BYTES: usize = 64 * 1024;

    /// Pages of `mov rcx, index; mov rax, trapped; jmp rax`, written while read-write and then sealed
    /// read-execute, never both.
    #[derive(Default)]
    pub struct Stubs {
        pages: Vec<(usize, usize, bool)>,
    }

    impl Stubs {
        pub fn stub(&mut self, index: usize) -> Option<usize> {
            let needs_page = self
                .pages
                .last()
                .is_none_or(|(_, used, sealed)| *sealed || used + STUB_BYTES > PAGE_BYTES);
            if needs_page {
                // SAFETY: a fresh read-write allocation.
                let page = unsafe {
                    VirtualAlloc(
                        std::ptr::null(),
                        PAGE_BYTES,
                        MEM_RESERVE | MEM_COMMIT,
                        PAGE_READWRITE,
                    )
                };
                if page.is_null() {
                    return None;
                }
                self.pages.push((page as usize, 0, false));
            }
            let (page, used, _) = self.pages.last_mut()?;
            let at = *page + *used;
            let mut code = Vec::with_capacity(STUB_BYTES);
            code.extend_from_slice(&[0x48, 0xB9]);
            code.extend_from_slice(&(index as u64).to_le_bytes());
            code.extend_from_slice(&[0x48, 0xB8]);
            code.extend_from_slice(&(super::trapped as *const () as usize as u64).to_le_bytes());
            code.extend_from_slice(&[0xFF, 0xE0]);
            // SAFETY: `at..at + STUB_BYTES` is inside an unsealed read-write page.
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len()) };
            *used += STUB_BYTES;
            Some(at)
        }

        pub fn seal(&mut self) -> Result<(), String> {
            for (page, _, sealed) in self.pages.iter_mut().filter(|p| !p.2) {
                let mut old = 0;
                // SAFETY: a page this struct allocated.
                let done = unsafe {
                    VirtualProtect(
                        *page as *const c_void,
                        PAGE_BYTES,
                        PAGE_EXECUTE_READ,
                        &mut old,
                    )
                };
                if done == 0 {
                    return Err("could not make the stand-in calls executable".into());
                }
                *sealed = true;
            }
            Ok(())
        }
    }
}
