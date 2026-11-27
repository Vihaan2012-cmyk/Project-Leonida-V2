//! An executable with thread-local storage, built byte by byte: its variable must read as its template in
//! every thread, and its TLS callback must run as the process and threads start.
#![cfg(windows)]
#![allow(unsafe_code)]

use pe_loader::{ImportRef, LoadOptions, Placement, Resolver, load};

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const CALLBACK: u32 = 0x1040;
const RDATA: u32 = 0x2000;
const DATA: u32 = 0x3000;
const INDEX: u32 = DATA;
const REASON: u32 = DATA + 0x10;
const CALLS: u32 = DATA + 0x20;
const TLS: u32 = 0x4000;
const RELOC: u32 = 0x5000;

struct NoImports;

impl Resolver for NoImports {
    fn resolve(&mut self, _: &ImportRef<'_>) -> usize {
        0
    }
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn relative(code: &mut Vec<u8>, at: u32, opcode: &[u8], target: u32) {
    let next = at + code.len() as u32 + opcode.len() as u32 + 4;
    code.extend_from_slice(opcode);
    code.extend_from_slice(&(target.wrapping_sub(next) as i32).to_le_bytes());
}

fn image() -> Vec<u8> {
    let mut file = vec![0u8; 0xE00];
    put(&mut file, 0, b"MZ");
    put(&mut file, 0x3C, &0x40u32.to_le_bytes());
    put(&mut file, 0x40, b"PE\0\0");
    put(&mut file, 0x44, &0x8664u16.to_le_bytes());
    put(&mut file, 0x46, &5u16.to_le_bytes());
    put(&mut file, 0x54, &0xF0u16.to_le_bytes());
    put(&mut file, 0x56, &0x22u16.to_le_bytes());
    let optional = 0x58;
    put(&mut file, optional, &0x20Bu16.to_le_bytes());
    put(&mut file, optional + 16, &TEXT.to_le_bytes());
    put(&mut file, optional + 24, &BASE.to_le_bytes());
    put(&mut file, optional + 32, &0x1000u32.to_le_bytes());
    put(&mut file, optional + 36, &0x200u32.to_le_bytes());
    put(&mut file, optional + 56, &0x6000u32.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x160u16.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    for (index, (rva, size)) in [(5, (RELOC, 20u32)), (9, (RDATA, 40))] {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(&mut file, optional + 116 + 8 * index, &size.to_le_bytes());
    }
    let sections = [
        (".text", TEXT, 0x400u32, 0x6000_0020u32),
        (".rdata", RDATA, 0x600, 0x4000_0040),
        (".data", DATA, 0x800, 0xC000_0040),
        (".tls", TLS, 0xA00, 0xC000_0040),
        (".reloc", RELOC, 0xC00, 0x4200_0040),
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
    let mut read = Vec::new();
    relative(&mut read, TEXT, &[0x8B, 0x05], INDEX);
    read.extend_from_slice(&[0x65, 0x48, 0x8B, 0x0C, 0x25, 0x58, 0, 0, 0]);
    read.extend_from_slice(&[0x48, 0x8B, 0x0C, 0xC1, 0x8B, 0x01, 0xC3]);
    put(&mut file, 0x400, &read);
    let mut callback = Vec::new();
    relative(&mut callback, CALLBACK, &[0x89, 0x15], REASON);
    relative(&mut callback, CALLBACK, &[0xF0, 0xFF, 0x05], CALLS);
    callback.push(0xC3);
    put(&mut file, 0x400 + (CALLBACK - TEXT) as usize, &callback);
    let va = |rva: u32| BASE + u64::from(rva);
    let directory = [va(TLS), va(TLS + 4), va(INDEX), va(RDATA + 0x40)];
    for (k, value) in directory.iter().enumerate() {
        put(&mut file, 0x600 + 8 * k, &value.to_le_bytes());
    }
    put(&mut file, 0x620, &0x10u32.to_le_bytes());
    put(&mut file, 0x640, &va(CALLBACK).to_le_bytes());
    put(&mut file, 0xA00, &0x1234u32.to_le_bytes());
    put(&mut file, 0xC00, &RDATA.to_le_bytes());
    put(&mut file, 0xC04, &20u32.to_le_bytes());
    for (k, offset) in [0u16, 8, 0x10, 0x18, 0x40, 0].into_iter().enumerate() {
        let entry = if k == 5 { 0 } else { 0xA000 | offset };
        put(&mut file, 0xC08 + 2 * k, &entry.to_le_bytes());
    }
    file
}

fn read_u32(base: usize, rva: u32) -> u32 {
    // SAFETY: inside the loaded image's .data section.
    unsafe { std::ptr::read_volatile((base + rva as usize) as *const u32) }
}

#[test]
fn thread_local_storage_reads_its_template_in_every_thread() {
    let options = LoadOptions {
        placement: Placement::Anywhere,
        helpers: Some(std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tls-helpers")),
    };
    let module = load(&image(), &mut NoImports, &options).expect("loads");
    let base = module.base();
    assert_ne!(
        read_u32(base, INDEX),
        0,
        "a slot of its own, not the test's"
    );
    assert_eq!(
        read_u32(base, REASON),
        1,
        "the callback ran for the process"
    );
    // SAFETY: the entry reads its TLS variable and returns it.
    assert_eq!(unsafe { module.call_entry() }, 0x1234);
    let entry = module.entry();
    let other = std::thread::spawn(move || {
        // SAFETY: as above, on a thread started after the image was loaded.
        let read: extern "win64" fn() -> usize = unsafe { std::mem::transmute(entry) };
        read()
    })
    .join()
    .expect("thread ran");
    assert_eq!(other, 0x1234);
    assert!(
        read_u32(base, CALLS) >= 3,
        "attach, thread attach and thread detach"
    );
}
