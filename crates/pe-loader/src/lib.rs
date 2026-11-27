//! Maps a 64-bit PE executable into this process and readies it to run: sections, base relocations, imports,
//! the exception table and page protections. What each import becomes is up to a [`Resolver`].
#![allow(unsafe_code)]

use std::path::PathBuf;

use pe_image::{DIRECTORY_EXCEPTION, DIRECTORY_TLS, Image, PeError};

mod tls;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Pe(#[from] PeError),
    #[error("not an x64 executable (machine {0:#x})")]
    Machine(u16),
    #[error("the image uses thread-local storage but no folder for its helper was given")]
    NoHelperFolder,
    #[error("thread-local storage: {0}")]
    ThreadLocalStorage(String),
    #[error("could not reserve {0} bytes for the image")]
    Reserve(usize),
    #[error("the image cannot move from its preferred address: it has no relocations")]
    Fixed,
    #[error("relocation type {kind} at {rva:#x} is not supported")]
    Relocation { rva: u32, kind: u8 },
    #[error("section {0} is both writable and executable")]
    WritableCode(String),
    #[error("section {0} lies outside the image")]
    Section(String),
    #[error("setting page protection failed at {0:#x}")]
    Protect(u32),
    #[error("registering the exception table failed")]
    ExceptionTable,
    #[error("only Windows can run executables")]
    Unsupported,
}

/// One imported function: by name or by ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportRef<'a> {
    pub dll: &'a str,
    pub name: Option<&'a str>,
    pub ordinal: Option<u16>,
}

/// Gives each import the address its IAT slot will hold. It must always give one: a stand-in that fails is
/// better than a null the program jumps to.
pub trait Resolver {
    fn resolve(&mut self, import: &ImportRef<'_>) -> usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// At the image's preferred address if it is free, anywhere otherwise.
    Preferred,
    /// Anywhere, so relocations always apply.
    Anywhere,
}

/// How to load an image.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub placement: Placement,
    /// Where helper DLLs for thread-local storage are written; needed only by images that use it.
    pub helpers: Option<PathBuf>,
}

/// A mapped image. Dropping it unloads its TLS helper, removes its exception table and frees its memory.
pub struct Module {
    base: usize,
    size: usize,
    entry: u32,
    exception_table: Option<usize>,
    tls_helper: Option<(usize, PathBuf)>,
    /// The helper's TLS slot and our executable's index variable, when the two swapped slots.
    tls_swap: Option<(u32, usize)>,
}

impl Module {
    pub fn base(&self) -> usize {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn entry(&self) -> usize {
        self.base + self.entry as usize
    }

    /// Calls the entry point with no arguments and returns what it leaves in `rax`.
    ///
    /// # Safety
    /// Runs the image's own code with this process's rights.
    pub unsafe fn call_entry(&self) -> usize {
        // SAFETY: the caller accepts running the image; `entry` lies in a read-execute section of it.
        unsafe {
            let entry: extern "win64" fn() -> usize = std::mem::transmute(self.entry());
            entry()
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FlushInstructionCache, IMAGE_RUNTIME_FUNCTION_ENTRY, RtlAddFunctionTable,
        RtlDeleteFunctionTable,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS,
        PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    const RELOCATION_PADDING: u8 = 0;
    const RELOCATION_DIR64: u8 = 10;
    const RELOCATIONS_STRIPPED: u16 = 0x1;

    pub fn load(
        bytes: &[u8],
        resolver: &mut dyn Resolver,
        options: &LoadOptions,
    ) -> Result<Module, LoadError> {
        let image = Image::parse(bytes)?;
        if image.machine != 0x8664 {
            return Err(LoadError::Machine(image.machine));
        }
        if image.directory(DIRECTORY_TLS).is_some() && options.helpers.is_none() {
            return Err(LoadError::NoHelperFolder);
        }
        let placement = options.placement;
        for section in &image.sections {
            if section.executable() && section.writable() {
                return Err(LoadError::WritableCode(section.name.clone()));
            }
            let end = u64::from(section.virtual_address)
                + u64::from(section.virtual_size.max(section.raw_size));
            if end > u64::from(image.image_size) {
                return Err(LoadError::Section(section.name.clone()));
            }
        }
        let size = image.image_size as usize;
        let mut module = reserve(&image, size, placement)?;
        // SAFETY: `module.base` is `size` fresh read-write bytes; every copy and write below stays inside
        // them, which the section bounds checked above and `write_u64` ensure.
        unsafe {
            let base = module.base as *mut u8;
            let headers = (image.headers_size as usize).min(bytes.len()).min(size);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), base, headers);
            for section in &image.sections {
                let start = section.raw_offset as usize;
                let length = section
                    .raw_size
                    .min(section.virtual_size.max(section.raw_size))
                    as usize;
                let Some(raw) = bytes.get(start..start + length) else {
                    return Err(LoadError::Section(section.name.clone()));
                };
                std::ptr::copy_nonoverlapping(
                    raw.as_ptr(),
                    base.add(section.virtual_address as usize),
                    raw.len(),
                );
            }
            let delta = (module.base as u64).wrapping_sub(image.image_base);
            if delta != 0 {
                for (rva, kind) in image.relocations() {
                    match kind {
                        RELOCATION_PADDING => {}
                        RELOCATION_DIR64 => {
                            let value = read_u64(&module, rva)
                                .ok_or(LoadError::Relocation { rva, kind })?;
                            write_u64(&module, rva, value.wrapping_add(delta))
                                .ok_or(LoadError::Relocation { rva, kind })?;
                        }
                        _ => return Err(LoadError::Relocation { rva, kind }),
                    }
                }
            }
            for dll in image.imports() {
                for entry in &dll.entries {
                    let import = ImportRef {
                        dll: &dll.name,
                        name: entry.name.as_deref(),
                        ordinal: entry.ordinal,
                    };
                    let address = resolver.resolve(&import);
                    write_u64(&module, entry.iat_rva, address as u64)
                        .ok_or(LoadError::Section(".idata".into()))?;
                }
            }
            if let Some((rva, length)) = image.directory(DIRECTORY_EXCEPTION) {
                let table = module.base + rva as usize;
                let count = length / std::mem::size_of::<IMAGE_RUNTIME_FUNCTION_ENTRY>() as u32;
                if !RtlAddFunctionTable(table as *const _, count, module.base as u64) {
                    return Err(LoadError::ExceptionTable);
                }
                module.exception_table = Some(table);
            }
            protect(&module, 0, image.headers_size as usize, PAGE_READONLY, 0)?;
            for section in &image.sections {
                let flags = if section.executable() {
                    PAGE_EXECUTE_READ
                } else if section.writable() {
                    PAGE_READWRITE
                } else {
                    PAGE_READONLY
                };
                let length = section.virtual_size.max(section.raw_size) as usize;
                protect(
                    &module,
                    section.virtual_address as usize,
                    length,
                    flags,
                    section.virtual_address,
                )?;
            }
            FlushInstructionCache(GetCurrentProcess(), base as *const c_void, size);
        }
        if let (Some((rva, _)), Some(folder)) = (image.directory(DIRECTORY_TLS), &options.helpers) {
            let host = host_tls_index();
            module.tls_helper = Some(thread_storage(&module, &image, rva, folder, host)?);
            if let Some(host) = host {
                // SAFETY: our executable's index variable, which the helper's callback has just written.
                let slot = unsafe { std::ptr::read_volatile(host as *const u32) };
                if slot != 0 {
                    module.tls_swap = Some((slot, host as usize));
                }
            }
        }
        Ok(module)
    }

    static HELPERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Our executable's TLS index variable, when it has static TLS and holds slot 0.
    fn host_tls_index() -> Option<u64> {
        // SAFETY: reads the headers of our own executable as Windows mapped them.
        unsafe {
            let base = GetModuleHandleW(std::ptr::null()) as usize;
            if base == 0 {
                return None;
            }
            let header = base + std::ptr::read_unaligned((base + 0x3C) as *const u32) as usize;
            let optional = header + 24;
            if std::ptr::read_unaligned(optional as *const u16) != 0x20B
                || std::ptr::read_unaligned((optional + 108) as *const u32) <= 9
            {
                return None;
            }
            let rva = std::ptr::read_unaligned((optional + 112 + 72) as *const u32) as usize;
            if rva == 0 {
                return None;
            }
            let index = std::ptr::read_unaligned((base + rva + 16) as *const u64);
            (index != 0 && std::ptr::read_volatile(index as *const u32) == 0).then_some(index)
        }
    }

    /// Puts our executable's TLS back in slot 0 for the calling thread, undoing the helper's swap.
    ///
    /// # Safety
    /// `slot` is the helper's slot, swapped with slot 0 in this thread, and `host` our index variable.
    unsafe fn unswap(slot: u32, host: usize) {
        // SAFETY: one instruction sequence, so no code of ours reads TLS between the swap and the index write.
        unsafe {
            std::arch::asm!(
                "mov rcx, gs:[0x58]",
                "mov rdx, [rcx]",
                "mov r8, [rcx + r9 * 8]",
                "mov [rcx], r8",
                "mov [rcx + r9 * 8], rdx",
                "mov dword ptr [r10], 0",
                in("r9") u64::from(slot),
                in("r10") host,
                out("rcx") _,
                out("rdx") _,
                out("r8") _,
                options(nostack),
            );
        }
    }

    /// Loads a helper DLL carrying the image's TLS directory; see the `tls` module.
    fn thread_storage(
        module: &Module,
        image: &Image<'_>,
        rva: u32,
        folder: &std::path::Path,
        host_index: Option<u64>,
    ) -> Result<(usize, PathBuf), LoadError> {
        let fail = |what: &str| LoadError::ThreadLocalStorage(what.to_string());
        let field = |k: u32| {
            read_u64(module, rva + 8 * k).ok_or_else(|| fail("directory outside the image"))
        };
        let (start, end, index, callbacks) = (field(0)?, field(1)?, field(2)?, field(3)?);
        let sizes = field(4)?;
        let inside = |va: u64| {
            va.checked_sub(module.base as u64)
                .and_then(|at| u32::try_from(at).ok())
                .filter(|at| (*at as usize) < module.size)
        };
        let writable = inside(index)
            .and_then(|at| image.section_of(at))
            .is_some_and(|section| section.writable());
        if !writable {
            return Err(fail(
                "the index variable is not in a writable section of the image",
            ));
        }
        let template = match (inside(start), end.checked_sub(start)) {
            (Some(at), Some(length)) if (at as u64 + length) as usize <= module.size => {
                // SAFETY: a range of the mapped image, checked above.
                unsafe {
                    std::slice::from_raw_parts(
                        (module.base + at as usize) as *const u8,
                        length as usize,
                    )
                }
                .to_vec()
            }
            _ => return Err(fail("the template lies outside the image")),
        };
        let mut list = Vec::new();
        if let Some(mut at) = inside(callbacks) {
            while let Some(callback) = read_u64(module, at).filter(|c| *c != 0) {
                if inside(callback).is_none() || list.len() == 64 {
                    return Err(fail("a callback lies outside the image"));
                }
                list.push(callback);
                at += 8;
            }
        }
        let directory = tls::Directory {
            template,
            zero_fill: sizes as u32,
            characteristics: (sizes >> 32) as u32,
            index,
            host_index: host_index.unwrap_or(0),
            callbacks: list,
        };
        std::fs::create_dir_all(folder).map_err(|e| fail(&e.to_string()))?;
        let number = HELPERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = folder.join(format!("tls-{}-{number}.dll", std::process::id()));
        std::fs::write(&path, tls::helper(&directory)).map_err(|e| fail(&e.to_string()))?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: the helper has no code of its own; loading it runs only the image's TLS callbacks, mapped
        // read-execute by now.
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            let _ = std::fs::remove_file(&path);
            return Err(fail("Windows did not load the helper DLL"));
        }
        Ok((handle as usize, path))
    }

    fn reserve(image: &Image<'_>, size: usize, placement: Placement) -> Result<Module, LoadError> {
        let commit = MEM_RESERVE | MEM_COMMIT;
        // SAFETY: plain allocations; a refused preferred address returns null and falls through.
        let base = unsafe {
            let wanted = match placement {
                Placement::Preferred => VirtualAlloc(
                    image.image_base as *const c_void,
                    size,
                    commit,
                    PAGE_READWRITE,
                ),
                Placement::Anywhere => std::ptr::null_mut(),
            };
            if wanted.is_null() {
                VirtualAlloc(std::ptr::null(), size, commit, PAGE_READWRITE)
            } else {
                wanted
            }
        };
        if base.is_null() {
            return Err(LoadError::Reserve(size));
        }
        let module = Module {
            base: base as usize,
            size,
            entry: image.entry,
            exception_table: None,
            tls_helper: None,
            tls_swap: None,
        };
        let moved = module.base as u64 != image.image_base;
        if moved
            && (image.characteristics & RELOCATIONS_STRIPPED != 0 || image.relocations().is_empty())
        {
            return Err(LoadError::Fixed);
        }
        Ok(module)
    }

    fn read_u64(module: &Module, rva: u32) -> Option<u64> {
        let at = rva as usize;
        if at.checked_add(8)? > module.size {
            return None;
        }
        // SAFETY: inside the module's read-write mapping, checked above.
        Some(unsafe { std::ptr::read_unaligned((module.base + at) as *const u64) })
    }

    fn write_u64(module: &Module, rva: u32, value: u64) -> Option<()> {
        let at = rva as usize;
        if at.checked_add(8)? > module.size {
            return None;
        }
        // SAFETY: inside the module's read-write mapping, checked above.
        unsafe { std::ptr::write_unaligned((module.base + at) as *mut u64, value) };
        Some(())
    }

    fn protect(
        module: &Module,
        start: usize,
        length: usize,
        flags: PAGE_PROTECTION_FLAGS,
        rva: u32,
    ) -> Result<(), LoadError> {
        if length == 0 {
            return Ok(());
        }
        let mut old = 0;
        // SAFETY: a range of the module's own mapping.
        let done = unsafe {
            VirtualProtect(
                (module.base + start) as *const c_void,
                length.min(module.size - start),
                flags,
                &mut old,
            )
        };
        if done == 0 {
            return Err(LoadError::Protect(rva));
        }
        Ok(())
    }

    impl Drop for Module {
        fn drop(&mut self) {
            // SAFETY: the table and mapping were made by `load` and are released once.
            unsafe {
                if let Some((slot, host)) = self.tls_swap.take() {
                    unswap(slot, host);
                }
                if let Some((handle, path)) = self.tls_helper.take() {
                    FreeLibrary(handle as _);
                    let _ = std::fs::remove_file(path);
                }
                if let Some(table) = self.exception_table {
                    RtlDeleteFunctionTable(table as *const IMAGE_RUNTIME_FUNCTION_ENTRY);
                }
                VirtualFree(self.base as *mut c_void, 0, MEM_RELEASE);
            }
        }
    }
}

/// Maps `bytes` and binds every import through `resolver`.
pub fn load(
    bytes: &[u8],
    resolver: &mut dyn Resolver,
    options: &LoadOptions,
) -> Result<Module, LoadError> {
    #[cfg(windows)]
    {
        windows::load(bytes, resolver, options)
    }
    #[cfg(not(windows))]
    {
        let _ = (bytes, resolver, options);
        Err(LoadError::Unsupported)
    }
}
