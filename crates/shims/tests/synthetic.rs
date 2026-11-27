//! Small executables built byte by byte, loaded and run through the shims.
#![cfg(windows)]
#![allow(unsafe_code)]

use pe_loader::{ImportRef, LoadError, LoadOptions, Placement, load};

fn anywhere() -> LoadOptions {
    LoadOptions {
        placement: Placement::Anywhere,
        helpers: None,
    }
}
use shims::{Binder, Binding, Shim, Trap, take_calls};

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const RDATA: u32 = 0x2000;
const DATA: u32 = 0x3000;
const PDATA: u32 = 0x4000;
const RELOC: u32 = 0x5000;

const IMPORTS: [(&str, &str); 5] = [
    ("test.dll", "Triple"),
    ("missing.dll", "Nothing"),
    ("WS2_32.dll", "connect"),
    ("kernel32.dll", "GetCurrentProcessId"),
    ("kernel32.dll", "GetFileAttributesW"),
];

const PROBE_FILE: &str = "leonida-trace-probe.txt";
const PROBE_AT: u32 = DATA + 0x40;

fn iat(index: usize) -> u32 {
    RDATA + 0x200 + 0x20 * index as u32
}

struct Code(Vec<u8>);

impl Code {
    fn at(&self) -> u32 {
        TEXT + self.0.len() as u32
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// An instruction ending in a rip-relative displacement to `target`, with `tail` bytes after it.
    fn relative(&mut self, opcode: &[u8], target: u32, tail: &[u8]) {
        let next = self.at() + (opcode.len() + 4 + tail.len()) as u32;
        self.bytes(opcode);
        self.bytes(&(target.wrapping_sub(next) as i32).to_le_bytes());
        self.bytes(tail);
    }
}

/// Asks for the attributes of a file that does not exist, reads a pointer to 7 that needs relocating, triples
/// it through a shim, calls a missing import and a blocked one (both give 0), stores the process id from the
/// host at DATA + 0x20, and returns the sum.
fn program() -> Vec<u8> {
    let mut code = Code(Vec::new());
    code.bytes(&[0x48, 0x83, 0xEC, 0x28]);
    code.relative(&[0x48, 0x8D, 0x0D], PROBE_AT, &[]);
    code.relative(&[0xFF, 0x15], iat(4), &[]);
    code.relative(&[0x48, 0x8B, 0x05], DATA, &[]);
    code.bytes(&[0x8B, 0x08]);
    code.relative(&[0xFF, 0x15], iat(0), &[]);
    for import in [1, 2] {
        code.bytes(&[0x48, 0x89, 0x44, 0x24, 0x20]);
        code.relative(&[0xFF, 0x15], iat(import), &[]);
        code.bytes(&[0x48, 0x03, 0x44, 0x24, 0x20]);
    }
    code.bytes(&[0x48, 0x89, 0x44, 0x24, 0x20]);
    code.relative(&[0xFF, 0x15], iat(3), &[]);
    code.relative(&[0x89, 0x05], DATA + 0x20, &[]);
    code.bytes(&[0x48, 0x8B, 0x44, 0x24, 0x20]);
    code.bytes(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
    code.0
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn image(text_characteristics: u32) -> Vec<u8> {
    let mut file = vec![0u8; 0x1200];
    put(&mut file, 0, b"MZ");
    put(&mut file, 0x3C, &0x40u32.to_le_bytes());
    put(&mut file, 0x40, b"PE\0\0");
    let header = [
        0x8664u16.to_le_bytes().as_slice(),
        &5u16.to_le_bytes(),
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
    put(&mut file, optional + 56, &0x6000u32.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x160u16.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    for (index, (rva, size)) in [(1, (RDATA, 120)), (3, (PDATA, 12)), (5, (RELOC, 12))] {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(
            &mut file,
            optional + 116 + 8 * index,
            &(size as u32).to_le_bytes(),
        );
    }
    let sections = [
        (".text", TEXT, 0x400u32, text_characteristics),
        (".rdata", RDATA, 0x600, 0x4000_0040),
        (".data", DATA, 0xC00, 0xC000_0040),
        (".pdata", PDATA, 0xE00, 0x4000_0040),
        (".reloc", RELOC, 0x1000, 0x4200_0040),
    ];
    for (index, (name, rva, raw, characteristics)) in sections.iter().enumerate() {
        let at = 0x148 + 40 * index;
        let mut short = [0u8; 8];
        short[..name.len()].copy_from_slice(name.as_bytes());
        put(&mut file, at, &short);
        let size = if *name == ".rdata" { 0x600u32 } else { 0x200 };
        put(&mut file, at + 8, &size.to_le_bytes());
        put(&mut file, at + 12, &rva.to_le_bytes());
        put(&mut file, at + 16, &size.to_le_bytes());
        put(&mut file, at + 20, &raw.to_le_bytes());
        put(&mut file, at + 36, &characteristics.to_le_bytes());
    }
    let code = program();
    put(&mut file, 0x400, &code);
    let file_of = |rva: u32| (rva - RDATA + 0x600) as usize;
    for (index, (dll, name)) in IMPORTS.iter().enumerate() {
        let lookup = RDATA + 0x100 + 0x20 * index as u32;
        let hint = RDATA + 0x300 + 0x40 * index as u32;
        let dll_name = RDATA + 0x500 + 0x20 * index as u32;
        let descriptor = [lookup, 0, 0, dll_name, iat(index)]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>();
        put(&mut file, file_of(RDATA) + 20 * index, &descriptor);
        put(&mut file, file_of(lookup), &u64::from(hint).to_le_bytes());
        put(
            &mut file,
            file_of(iat(index)),
            &u64::from(hint).to_le_bytes(),
        );
        put(&mut file, file_of(hint) + 2, name.as_bytes());
        put(&mut file, file_of(dll_name), dll.as_bytes());
    }
    put(
        &mut file,
        0xC00,
        &(BASE + u64::from(DATA) + 0x10).to_le_bytes(),
    );
    put(&mut file, 0xC10, &7u32.to_le_bytes());
    let wide: Vec<u8> = PROBE_FILE
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    put(&mut file, 0xC00 + (PROBE_AT - DATA) as usize, &wide);
    let function = [TEXT, TEXT + code.len() as u32, PDATA + 0x10];
    put(
        &mut file,
        0xE00,
        &function
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    put(
        &mut file,
        0xE10,
        &[0x01, 0x04, 0x01, 0x00, 0x04, 0x42, 0x00, 0x00],
    );
    put(&mut file, 0x1000, &DATA.to_le_bytes());
    put(&mut file, 0x1004, &12u32.to_le_bytes());
    put(&mut file, 0x1008, &(0xA000u16).to_le_bytes());
    file
}

extern "win64" fn triple(value: u32) -> u64 {
    u64::from(value) * 3
}

fn test_shims() -> Vec<Shim> {
    vec![Shim {
        dll: "test.dll",
        name: "Triple",
        address: triple as *const () as usize,
    }]
}

fn run(mut binder: Binder) -> Binder {
    let bytes = image(0x6000_0020);
    let module = load(&bytes, &mut binder, &anywhere()).expect("loads");
    let watched = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("trace");
    shims::trace::watch_crashes(watched, module.base(), module.size());
    assert_ne!(
        module.base() as u64,
        BASE,
        "placed elsewhere, so the pointer was relocated"
    );
    binder.seal().expect("stand-ins sealed");
    take_calls();
    // SAFETY: the image is the program above: it only calls its imports and returns.
    let result = unsafe { module.call_entry() };
    assert_eq!(result, 21);
    // SAFETY: inside the loaded .data section.
    let pid = unsafe { std::ptr::read((module.base() + DATA as usize + 0x20) as *const u32) };
    assert_eq!(pid, std::process::id());
    assert_eq!(
        take_calls(),
        [
            Trap {
                dll: "missing.dll".into(),
                function: "Nothing".into(),
                blocked: None,
            },
            Trap {
                dll: "WS2_32.dll".into(),
                function: "connect".into(),
                blocked: Some(offline::Reason::Sockets),
            },
        ]
    );
    binder
}

#[test]
fn a_synthetic_executable_runs_through_shims_host_functions_and_stand_ins() {
    let plain = run(Binder::new(&test_shims()));
    assert!(plain.trace().is_none());
    let traced = run(Binder::new(&test_shims()).traced());
    let trace = traced.trace().expect("tracing");
    assert_eq!(trace.calls("test.dll!Triple"), 1);
    assert_eq!(trace.calls("missing.dll!Nothing"), 1);
    assert_eq!(trace.last(), Some("kernel32.dll!GetCurrentProcessId"));
    assert!(
        shims::trace::files().iter().any(|f| f == PROBE_FILE),
        "{:?}",
        shims::trace::files()
    );
    assert!(trace.report().contains("5 of 5 imports called"));
}

#[test]
fn writable_code_is_refused() {
    let bytes = image(0xE000_0020);
    let error = load(&bytes, &mut Binder::new(&[]), &anywhere()).err();
    assert!(matches!(error, Some(LoadError::WritableCode(name)) if name == ".text"));
}

#[test]
fn imports_are_classified_without_loading() {
    let binder = Binder::new(&test_shims());
    let classify = |dll, name| {
        binder.classify(&ImportRef {
            dll,
            name: Some(name),
            ordinal: None,
        })
    };
    assert_eq!(classify("test.dll", "Triple"), Binding::Shim);
    assert_eq!(classify("xmem.dll", "XMemAlloc"), Binding::Shim);
    assert_eq!(
        classify("kernel32.dll", "GetCurrentProcessId"),
        Binding::Host
    );
    assert_eq!(
        classify("api-ms-win-core-synch-l1-1-0.dll", "WaitForSingleObject"),
        Binding::Host
    );
    assert_eq!(
        classify("api-ms-win-core-synch-l1-2-0.dll", "Sleep"),
        Binding::Shim
    );
    assert_eq!(
        classify("WINHTTP.dll", "WinHttpOpen"),
        Binding::Blocked(offline::Reason::WebRequests)
    );
    assert_eq!(classify("missing.dll", "Nothing"), Binding::Missing);
    assert_eq!(classify("kernel32.dll", "NoSuchFunction"), Binding::Missing);
}
