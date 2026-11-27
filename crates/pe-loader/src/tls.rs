//! Thread-local storage for a mapped image through a helper DLL. Windows only gives implicit TLS to modules
//! it loads itself, so a small DLL is written that carries the image's TLS template and index variable. When
//! Windows loads it, it reserves a slot, copies the template into every thread's block, existing and future,
//! and runs the helper's TLS callbacks: the first copies the slot number into the image's own index
//! variable, the rest are the image's callbacks, so they run as threads start and end. (Windows skips all
//! of this for a DLL that imports nothing or has no entry point, so the helper has both.)

const BASE: u64 = 0x1_8000_0000;
const TEXT: u32 = 0x1000;
const RDATA: u32 = 0x2000;
const DATA: u32 = 0x3000;
const TLS: u32 = 0x4000;
const FILE_ALIGN: usize = 0x200;
const PAGE: u32 = 0x1000;
/// Windows runs TLS callbacks only for DLLs with an entry point; this one returns TRUE.
const ENTRY: u32 = 0x20;
/// Windows sets up TLS only for DLLs that import something, so the helper imports `GetLastError`.
const IMPORTS: u32 = 0x800;

pub struct Directory {
    pub template: Vec<u8>,
    pub zero_fill: u32,
    pub characteristics: u32,
    /// Where the image keeps its slot number.
    pub index: u64,
    /// The image's callbacks.
    pub callbacks: Vec<u64>,
}

fn put(file: &mut Vec<u8>, at: usize, bytes: &[u8]) {
    if file.len() < at + bytes.len() {
        file.resize(at + bytes.len(), 0);
    }
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

pub fn helper(directory: &Directory) -> Vec<u8> {
    let template_size = directory.template.len().max(1);
    let tls_pages = align(template_size, PAGE as usize) as u32;
    let reloc = TLS + tls_pages;
    let image_size = reloc + PAGE;
    let va = |rva: u32| BASE + u64::from(rva);
    let mut stub = vec![0x8B, 0x05];
    stub.extend_from_slice(&(DATA.wrapping_sub(TEXT + 6) as i32).to_le_bytes());
    stub.extend_from_slice(&[0x48, 0xB9]);
    stub.extend_from_slice(&directory.index.to_le_bytes());
    stub.extend_from_slice(&[0x89, 0x01, 0xC3]);
    stub.resize(ENTRY as usize, 0xCC);
    stub.extend_from_slice(&[0xB8, 1, 0, 0, 0, 0xC3]);
    let callbacks_at = RDATA + 0x40;
    let mut table = vec![0u8; 0x40];
    put(&mut table, 0, &va(TLS).to_le_bytes());
    put(
        &mut table,
        8,
        &(va(TLS) + directory.template.len() as u64).to_le_bytes(),
    );
    put(&mut table, 16, &va(DATA).to_le_bytes());
    put(&mut table, 24, &va(callbacks_at).to_le_bytes());
    put(&mut table, 32, &directory.zero_fill.to_le_bytes());
    put(&mut table, 36, &directory.characteristics.to_le_bytes());
    let mut list = vec![va(TEXT)];
    list.extend_from_slice(&directory.callbacks);
    list.push(0);
    for callback in &list {
        table.extend_from_slice(&callback.to_le_bytes());
    }
    let imports = RDATA + IMPORTS;
    let (lookup, iat, hint, dll) = (
        imports + 0x40,
        imports + 0x50,
        imports + 0x60,
        imports + 0x80,
    );
    let at = |rva: u32| (rva - RDATA) as usize;
    let descriptor: Vec<u8> = [lookup, 0, 0, dll, iat]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    put(&mut table, at(imports), &descriptor);
    put(&mut table, at(lookup), &u64::from(hint).to_le_bytes());
    put(&mut table, at(iat), &u64::from(hint).to_le_bytes());
    put(&mut table, at(hint) + 2, b"GetLastError\0");
    put(&mut table, at(dll), b"kernel32.dll\0");
    let mut relocations = Vec::new();
    for (page, offsets) in [(RDATA, vec![0u16, 8, 16, 24, 0x40])] {
        let mut entries: Vec<u16> = offsets.iter().map(|o| 0xA000 | o).collect();
        if entries.len() % 2 == 1 {
            entries.push(0);
        }
        relocations.extend_from_slice(&page.to_le_bytes());
        relocations.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
        for entry in entries {
            relocations.extend_from_slice(&entry.to_le_bytes());
        }
    }
    let sections: [(&str, u32, Vec<u8>, u32, u32); 5] = [
        (".text", TEXT, stub, PAGE, 0x6000_0020),
        (".rdata", RDATA, table, PAGE, 0x4000_0040),
        (".data", DATA, vec![0; 4], PAGE, 0xC000_0040),
        (
            ".tls",
            TLS,
            directory.template.clone(),
            tls_pages,
            0xC000_0040,
        ),
        (".reloc", reloc, relocations.clone(), PAGE, 0x4200_0040),
    ];
    let mut file = Vec::new();
    put(&mut file, 0, b"MZ");
    put(&mut file, 0x3C, &0x40u32.to_le_bytes());
    put(&mut file, 0x40, b"PE\0\0");
    put(&mut file, 0x44, &0x8664u16.to_le_bytes());
    put(&mut file, 0x46, &(sections.len() as u16).to_le_bytes());
    put(&mut file, 0x54, &0xF0u16.to_le_bytes());
    put(&mut file, 0x56, &0x2022u16.to_le_bytes());
    let optional = 0x58;
    put(&mut file, optional, &0x20Bu16.to_le_bytes());
    put(&mut file, optional + 16, &(TEXT + ENTRY).to_le_bytes());
    put(&mut file, optional + 24, &BASE.to_le_bytes());
    put(&mut file, optional + 32, &PAGE.to_le_bytes());
    put(&mut file, optional + 36, &(FILE_ALIGN as u32).to_le_bytes());
    put(&mut file, optional + 40, &6u16.to_le_bytes());
    put(&mut file, optional + 48, &6u16.to_le_bytes());
    put(&mut file, optional + 56, &image_size.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x0160u16.to_le_bytes());
    put(&mut file, optional + 72, &0x10_0000u64.to_le_bytes());
    put(&mut file, optional + 80, &0x1000u64.to_le_bytes());
    put(&mut file, optional + 88, &0x10_0000u64.to_le_bytes());
    put(&mut file, optional + 96, &0x1000u64.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    let entries = [
        (1, (RDATA + IMPORTS, 40)),
        (5, (reloc, relocations.len() as u32)),
        (9, (RDATA, 40)),
    ];
    for (index, (rva, size)) in entries {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(&mut file, optional + 116 + 8 * index, &size.to_le_bytes());
    }
    let mut raw = 0x400;
    for (index, (name, rva, bytes, size, characteristics)) in sections.iter().enumerate() {
        let at = 0x148 + 40 * index;
        let mut short = [0u8; 8];
        short[..name.len()].copy_from_slice(name.as_bytes());
        put(&mut file, at, &short);
        let raw_size = align(bytes.len().max(1), FILE_ALIGN);
        put(&mut file, at + 8, &size.to_le_bytes());
        put(&mut file, at + 12, &rva.to_le_bytes());
        put(&mut file, at + 16, &(raw_size as u32).to_le_bytes());
        put(&mut file, at + 20, &(raw as u32).to_le_bytes());
        put(&mut file, at + 36, &characteristics.to_le_bytes());
        put(&mut file, raw, bytes);
        raw += raw_size;
        file.resize(raw, 0);
    }
    file
}
