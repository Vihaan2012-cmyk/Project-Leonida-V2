//! A small executable image, built byte by byte, with the shape of the look controller update the mouse patch hooks.

use enhancements::camera_mouse::{HOOK_LEN, Layout};
use enhancements::widescreen::fnv1a64;
use pe_image::Image;

pub const TEXT: u32 = 0x1000;
pub const DATA: u32 = 0x2000;
pub const PDATA: u32 = 0x3000;
const RELOC: u32 = 0x4000;
pub const UPDATE: u32 = TEXT;
pub const HOOK: u32 = UPDATE + 9 + 24;

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn update() -> Vec<u8> {
    let mut code = vec![0x56, 0x48, 0x89, 0xCE, 0xB8, 0x5A, 0x5A, 0x00, 0x00];
    code.extend_from_slice(&[0xC5, 0xFA, 0x10, 0x86, 0x90, 0x03, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x58, 0x86, 0x88, 0x03, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x59, 0x86, 0x8C, 0x03, 0x00, 0x00]);
    assert_eq!(UPDATE + code.len() as u32, HOOK);
    code.extend_from_slice(&[0xC5, 0xFA, 0x11, 0x86, 0xA0, 0x1C, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x10, 0x86, 0x28, 0x05, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x58, 0x86, 0x20, 0x05, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x59, 0x86, 0x24, 0x05, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x11, 0x86, 0xA4, 0x1C, 0x00, 0x00]);
    code.extend_from_slice(&[0x5E, 0xC3]);
    code
}

/// The file and the layout that finds the hook in it.
pub fn image() -> (Vec<u8>, Layout) {
    let mut file = vec![0u8; 0xE00];
    put(&mut file, 0, b"MZ");
    put(&mut file, 0x3C, &0x40u32.to_le_bytes());
    put(&mut file, 0x40, b"PE\0\0");
    let header = [
        0x8664u16.to_le_bytes().as_slice(),
        &4u16.to_le_bytes(),
        &[0; 12],
        &0xF0u16.to_le_bytes(),
        &0x22u16.to_le_bytes(),
    ]
    .concat();
    put(&mut file, 0x44, &header);
    let optional = 0x58;
    put(&mut file, optional, &0x20Bu16.to_le_bytes());
    put(&mut file, optional + 16, &TEXT.to_le_bytes());
    put(&mut file, optional + 24, &0x1_4000_0000u64.to_le_bytes());
    put(&mut file, optional + 32, &0x1000u32.to_le_bytes());
    put(&mut file, optional + 36, &0x200u32.to_le_bytes());
    put(&mut file, optional + 56, &0x5000u32.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x160u16.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    for (index, (rva, size)) in [(3, (PDATA, 12)), (5, (RELOC, 12))] {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(
            &mut file,
            optional + 116 + 8 * index,
            &(size as u32).to_le_bytes(),
        );
    }
    let sections = [
        (".text", TEXT, 0x400u32, 0x400u32, 0x6000_0020u32),
        (".data", DATA, 0x200, 0x800, 0xC000_0040),
        (".pdata", PDATA, 0x200, 0xA00, 0x4000_0040),
        (".reloc", RELOC, 0x200, 0xC00, 0x4200_0040),
    ];
    for (index, (name, rva, size, raw, characteristics)) in sections.iter().enumerate() {
        let at = 0x148 + 40 * index;
        let mut short = [0u8; 8];
        short[..name.len()].copy_from_slice(name.as_bytes());
        put(&mut file, at, &short);
        put(&mut file, at + 8, &size.to_le_bytes());
        put(&mut file, at + 12, &rva.to_le_bytes());
        put(&mut file, at + 16, &size.to_le_bytes());
        put(&mut file, at + 20, &raw.to_le_bytes());
        put(&mut file, at + 36, &characteristics.to_le_bytes());
    }
    let code = update();
    put(&mut file, 0x400, &code);
    let entry = [UPDATE, UPDATE + code.len() as u32, PDATA + 0x100]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect::<Vec<u8>>();
    put(&mut file, 0xA00, &entry);
    put(&mut file, 0xA00 + 0x100, &[0x01, 0x00, 0x00, 0x00]);
    put(&mut file, 0xC00, &DATA.to_le_bytes());
    put(&mut file, 0xC04, &12u32.to_le_bytes());
    let parsed = Image::parse(&file).unwrap();
    let hash = fnv1a64(parsed.read(HOOK, HOOK_LEN).unwrap());
    let layout = Layout {
        update: UPDATE,
        hash,
    };
    (file, layout)
}
