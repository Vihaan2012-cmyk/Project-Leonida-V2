//! A small executable that stops on `int3` and on `int 29h` under the trace's crash handler, run in a child
//! process because the stop ends it.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::path::Path;
use std::process::Command;

use enhancements::diagnostics;
use enhancements::patch::Mapped;
use pe_loader::{LoadOptions, Placement, load};
use shims::Binder;

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const PDATA: u32 = 0x2000;
const RELOC: u32 = 0x3000;

const PROLOGUE: [u8; 4] = [0x48, 0x83, 0xEC, 0x28];
const EPILOGUE: [u8; 5] = [0x48, 0x83, 0xC4, 0x28, 0xC3];

/// `cmp ecx, 6` then a jump over an `int3`: a check that fails because `ecx` is 5.
fn failed_check() -> Vec<u8> {
    let mut code = PROLOGUE.to_vec();
    code.extend_from_slice(&[0xB9, 5, 0, 0, 0, 0x83, 0xF9, 6, 0x74, 1, 0xCC]);
    code.extend_from_slice(&EPILOGUE);
    code
}

/// `mov ecx, 2` and `int 29h`, the stack cookie failure of the C runtime.
fn fast_fail() -> Vec<u8> {
    let mut code = PROLOGUE.to_vec();
    code.extend_from_slice(&[0xB9, 2, 0, 0, 0, 0xCD, 0x29]);
    code.extend_from_slice(&EPILOGUE);
    code
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn image(code: &[u8]) -> Vec<u8> {
    let mut file = vec![0u8; 0xA00];
    put(&mut file, 0, b"MZ");
    put(&mut file, 0x3C, &0x40u32.to_le_bytes());
    put(&mut file, 0x40, b"PE\0\0");
    let header = [
        0x8664u16.to_le_bytes().as_slice(),
        &3u16.to_le_bytes(),
        &[0; 12],
        &0xF0u16.to_le_bytes(),
        &0x22u16.to_le_bytes(),
    ]
    .concat();
    put(&mut file, 0x44, &header);
    let optional = 0x58;
    put(&mut file, optional, &0x20Bu16.to_le_bytes());
    put(&mut file, optional + 16, &TEXT.to_le_bytes());
    put(&mut file, optional + 24, &BASE.to_le_bytes());
    put(&mut file, optional + 32, &0x1000u32.to_le_bytes());
    put(&mut file, optional + 36, &0x200u32.to_le_bytes());
    put(&mut file, optional + 56, &0x4000u32.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x160u16.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    for (index, (rva, size)) in [(3, (PDATA, 12u32)), (5, (RELOC, 12))] {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(&mut file, optional + 116 + 8 * index, &size.to_le_bytes());
    }
    let sections = [
        (".text", TEXT, 0x400u32, 0x6000_0020u32),
        (".pdata", PDATA, 0x600, 0x4000_0040),
        (".reloc", RELOC, 0x800, 0x4200_0040),
    ];
    for (index, (name, rva, raw, characteristics)) in sections.iter().enumerate() {
        let at = 0x148 + 40 * index;
        let mut short = [0u8; 8];
        short[..name.len()].copy_from_slice(name.as_bytes());
        put(&mut file, at, &short);
        put(&mut file, at + 8, &0x200u32.to_le_bytes());
        put(&mut file, at + 12, &rva.to_le_bytes());
        put(&mut file, at + 16, &0x200u32.to_le_bytes());
        put(&mut file, at + 20, &raw.to_le_bytes());
        put(&mut file, at + 36, &characteristics.to_le_bytes());
    }
    put(&mut file, 0x400, code);
    let function = [TEXT, TEXT + code.len() as u32, PDATA + 0x10];
    put(
        &mut file,
        0x600,
        &function
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    put(
        &mut file,
        0x610,
        &[0x01, 0x04, 0x01, 0x00, 0x04, 0x42, 0x00, 0x00],
    );
    put(&mut file, 0x800, &TEXT.to_le_bytes());
    put(&mut file, 0x804, &12u32.to_le_bytes());
    file
}

/// Loads the program of `mode`, watches for stops into `folder` and runs it. Never returns.
fn child(mode: &str, folder: &Path) {
    use windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode;
    // SAFETY: keeps Windows from showing an error dialog for the stop.
    unsafe { SetErrorMode(0x8003) };
    let code = if mode == "fastfail" {
        fast_fail()
    } else {
        failed_check()
    };
    let bytes = image(&code);
    let options = LoadOptions {
        placement: Placement::Anywhere,
        helpers: None,
    };
    let mut binder = Binder::new(&[]);
    let module = load(&bytes, &mut binder, &options).expect("loads");
    binder.seal().expect("sealed");
    shims::trace::set_image(module.base(), module.size());
    shims::trace::watch_crashes(folder.to_path_buf(), module.base(), module.size());
    // SAFETY: the mapping is the module just loaded.
    let mut memory = unsafe { Mapped::new(module.base(), module.size()) };
    let sites = [TEXT + PROLOGUE.len() as u32 + 5];
    diagnostics::enable(&mut memory, &sites).expect("patched");
    // SAFETY: the program only stops itself.
    unsafe { module.call_entry() };
}

fn stop(mode: &str) -> (String, String) {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stops-{mode}"));
    let _ = std::fs::remove_dir_all(&folder);
    let status = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "a_stop_is_explained", "--nocapture"])
        .env("LEONIDA_STOP_MODE", mode)
        .env("LEONIDA_STOP_FOLDER", &folder)
        .status()
        .expect("spawned");
    assert!(!status.success(), "the program should not survive its stop");
    let read = |name: &str| std::fs::read_to_string(folder.join(name)).unwrap_or_default();
    (read("crash.txt"), read("summary.txt"))
}

#[test]
fn a_stop_is_explained() {
    if let (Ok(mode), Ok(folder)) = (
        std::env::var("LEONIDA_STOP_MODE"),
        std::env::var("LEONIDA_STOP_FOLDER"),
    ) {
        child(&mode, Path::new(&folder));
        return;
    }
    let (crash, summary) = stop("check");
    assert!(
        crash.contains("a failed check (int3) inside the program"),
        "{crash}"
    );
    assert!(crash.contains("image offset 0x100e"), "{crash}");
    assert!(crash.contains("image offset 0x1000"), "{crash}");
    assert!(crash.contains("a check failed"), "{crash}");
    assert!(crash.contains("cmp ecx,6"), "{crash}");
    assert!(crash.contains("rcx=0x5"), "{crash}");
    assert!(crash.contains("stack:"), "{crash}");
    assert!(
        summary.starts_with("stopped: a failed check (int3) inside the program"),
        "{summary}"
    );

    let (crash, summary) = stop("fastfail");
    assert!(
        crash.contains("== __fastfail(2) = STACK_COOKIE_CHECK_FAILURE"),
        "{crash}"
    );
    assert!(summary.starts_with("stopped: __fastfail(2)"), "{summary}");
    assert!(crash.contains("image offset 0x1009"), "{crash}");
    assert!(
        summary.contains("subsystem of the stop: C runtime fast fail"),
        "{summary}"
    );
}
