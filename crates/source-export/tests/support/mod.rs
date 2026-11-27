//! A small PE image built from bytes: four functions in the exception table, one leaf function without
//! unwind data, an import thunk, two imports, a string and one export.

pub const BASE: u64 = 0x1_4000_0000;
pub const F1: u32 = 0x1000;
pub const F2: u32 = 0x1040;
pub const F3: u32 = 0x1080;
pub const LEAF: u32 = 0x10C0;
pub const THUNK: u32 = 0x1100;
pub const IAT_FIRST: u32 = 0x2060;
pub const IAT_SECOND: u32 = 0x2068;
pub const STRING: u32 = 0x2100;

const TEXT: u32 = 0x1000;
const DATA: u32 = 0x2000;
const RAW: u32 = 0x400;

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn rel(target: u32, next: u32) -> [u8; 4] {
    (i64::from(target) - i64::from(next)).to_le_bytes()[..4]
        .try_into()
        .expect("four bytes")
}

pub fn image() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x2400];
    let at = |rva: u32| (rva - TEXT + RAW) as usize;
    bytes[..2].copy_from_slice(b"MZ");
    put32(&mut bytes, 0x3C, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    let file = 0x84;
    put16(&mut bytes, file, 0x8664);
    put16(&mut bytes, file + 2, 2);
    put16(&mut bytes, file + 16, 240);
    let optional = file + 20;
    put16(&mut bytes, optional, 0x20B);
    put32(&mut bytes, optional + 16, F1);
    put64(&mut bytes, optional + 24, BASE);
    put32(&mut bytes, optional + 56, 0x4000);
    put32(&mut bytes, optional + 108, 16);
    let directory = |index: usize| optional + 112 + 8 * index;
    put32(&mut bytes, directory(0), 0x2400);
    put32(&mut bytes, directory(0) + 4, 0x90);
    put32(&mut bytes, directory(1), 0x2000);
    put32(&mut bytes, directory(1) + 4, 40);
    put32(&mut bytes, directory(3), 0x2200);
    put32(&mut bytes, directory(3) + 4, 36);
    let sections = optional + 240;
    for (index, (name, rva, flags)) in [
        (&b".text"[..], TEXT, 0x6000_0020u32),
        (&b".rdata"[..], DATA, 0x4000_0040u32),
    ]
    .into_iter()
    .enumerate()
    {
        let section = sections + 40 * index;
        bytes[section..section + name.len()].copy_from_slice(name);
        put32(&mut bytes, section + 8, 0x1000);
        put32(&mut bytes, section + 12, rva);
        put32(&mut bytes, section + 16, 0x1000);
        put32(&mut bytes, section + 20, rva - TEXT + RAW);
        put32(&mut bytes, section + 36, flags);
    }

    bytes[at(TEXT)..at(TEXT) + 0x1000].fill(0xCC);
    let mut code = |rva: u32, parts: &[&[u8]]| {
        let mut cursor = at(rva);
        for part in parts {
            bytes[cursor..cursor + part.len()].copy_from_slice(part);
            cursor += part.len();
        }
    };
    let sub = [0x48, 0x83, 0xEC, 0x28];
    let add = [0x48, 0x83, 0xC4, 0x28];
    code(
        F1,
        &[
            &sub,
            &[0x48, 0x8D, 0x0D],
            &rel(STRING, F1 + 11),
            &[0xFF, 0x15],
            &rel(IAT_FIRST, F1 + 17),
            &[0xE8],
            &rel(F2, F1 + 22),
            &add,
            &[0xC3],
        ],
    );
    code(F2, &[&sub, &[0xE8], &rel(THUNK, F2 + 9), &add, &[0xC3]]);
    code(
        F3,
        &[
            &sub,
            &[0xE8],
            &rel(LEAF, F3 + 9),
            &[0x48, 0x8D, 0x05],
            &rel(F2, F3 + 16),
            &add,
            &[0xC3],
        ],
    );
    code(LEAF, &[&[0x31, 0xC0, 0xC3]]);
    code(THUNK, &[&[0xFF, 0x25], &rel(IAT_SECOND, THUNK + 6)]);

    put32(&mut bytes, at(DATA), 0x2040);
    put32(&mut bytes, at(DATA) + 12, 0x2080);
    put32(&mut bytes, at(DATA) + 16, IAT_FIRST);
    for base in [0x2040u32, IAT_FIRST] {
        put64(&mut bytes, at(base), 0x20A0);
        put64(&mut bytes, at(base) + 8, 0x20C0);
    }
    bytes[at(0x2080)..at(0x2080) + 13].copy_from_slice(b"kernel32.dll\0");
    bytes[at(0x20A2)..at(0x20A2) + 12].copy_from_slice(b"CreateThing\0");
    bytes[at(0x20C2)..at(0x20C2) + 11].copy_from_slice(b"OtherThing\0");
    bytes[at(STRING)..at(STRING) + 12].copy_from_slice(b"hello world\0");

    for (index, (begin, end)) in [(F1, 0x1020), (F2, 0x1060), (F3, 0x10A0)]
        .into_iter()
        .enumerate()
    {
        let entry = at(0x2200) + 12 * index;
        put32(&mut bytes, entry, begin);
        put32(&mut bytes, entry + 4, end);
        put32(&mut bytes, entry + 8, 0x2300);
    }
    bytes[at(0x2300)] = 1;

    put32(&mut bytes, at(0x2400) + 12, 0x2470);
    put32(&mut bytes, at(0x2400) + 16, 1);
    put32(&mut bytes, at(0x2400) + 20, 1);
    put32(&mut bytes, at(0x2400) + 24, 1);
    put32(&mut bytes, at(0x2400) + 28, 0x2440);
    put32(&mut bytes, at(0x2400) + 32, 0x2450);
    put32(&mut bytes, at(0x2400) + 36, 0x2460);
    put32(&mut bytes, at(0x2440), F1);
    put32(&mut bytes, at(0x2450), 0x2490);
    bytes[at(0x2470)..at(0x2470) + 9].copy_from_slice(b"game.dll\0");
    bytes[at(0x2490)..at(0x2490) + 14].copy_from_slice(b"ExportedStart\0");
    bytes
}
