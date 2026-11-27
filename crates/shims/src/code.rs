//! Small pieces of machine code made at load time: stand-ins, call counters and file-name recorders. They are
//! written into read-write pages that are then sealed read-execute, never both at once.

use std::ffi::c_void;

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualProtect,
};

const PAGE_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct Code {
    /// Each page's address, bytes used, and whether it is sealed.
    pages: Vec<(usize, usize, bool)>,
}

fn mov_rax(code: &mut Vec<u8>, value: u64) {
    code.extend_from_slice(&[0x48, 0xB8]);
    code.extend_from_slice(&value.to_le_bytes());
}

fn jump_to(code: &mut Vec<u8>, target: usize) {
    mov_rax(code, target as u64);
    code.extend_from_slice(&[0xFF, 0xE0]);
}

impl Code {
    fn emit(&mut self, code: &[u8]) -> Option<usize> {
        let needs_page = self
            .pages
            .last()
            .is_none_or(|(_, used, sealed)| *sealed || used + code.len() > PAGE_BYTES);
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
        // SAFETY: `at..at + code.len()` lies in an unsealed read-write page, checked above.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len()) };
        *used += code.len().next_multiple_of(16);
        Some(at)
    }

    /// `trapped(index)`: records the call and returns 0.
    pub fn trap(&mut self, index: usize) -> Option<usize> {
        let mut code = vec![0x48, 0xB9];
        code.extend_from_slice(&(index as u64).to_le_bytes());
        jump_to(&mut code, super::trapped as *const () as usize);
        self.emit(&code)
    }

    /// Notes `index` as the last import called, counts the call, and goes on to `target`. Only `rax`, which no
    /// argument uses, is changed.
    pub fn counter(
        &mut self,
        index: u32,
        counter: usize,
        last: usize,
        target: usize,
    ) -> Option<usize> {
        let mut code = Vec::new();
        mov_rax(&mut code, last as u64);
        code.extend_from_slice(&[0x48, 0xC7, 0x00]);
        code.extend_from_slice(&index.to_le_bytes());
        mov_rax(&mut code, counter as u64);
        code.extend_from_slice(&[0xF0, 0x48, 0xFF, 0x00]);
        jump_to(&mut code, target);
        self.emit(&code)
    }

    /// Hands the first argument, a file name, to the trace before going on to `target` with every argument
    /// register as it was.
    pub fn recorder(&mut self, target: usize, wide: bool) -> Option<usize> {
        let mut code = vec![
            0x51, 0x52, 0x41, 0x50, 0x41, 0x51, 0x48, 0x83, 0xEC, 0x28, 0xBA,
        ];
        code.extend_from_slice(&u32::from(wide).to_le_bytes());
        mov_rax(&mut code, super::trace::record_path as *const () as u64);
        code.extend_from_slice(&[
            0xFF, 0xD0, 0x48, 0x83, 0xC4, 0x28, 0x41, 0x59, 0x41, 0x58, 0x5A, 0x59,
        ]);
        jump_to(&mut code, target);
        self.emit(&code)
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
                return Err("could not make the generated calls executable".into());
            }
            *sealed = true;
        }
        Ok(())
    }
}
