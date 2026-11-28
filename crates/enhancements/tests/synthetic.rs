//! Patches on a small executable built byte by byte: found, applied to a mapped copy, run, reverted.
#![cfg(windows)]
#![allow(unsafe_code)]

use enhancements::framerate::{self, CLOCK_FLOOR, STEP_FLOOR};
use enhancements::patch::{
    Buffer, CodeMemory, Finder, Mapped, PatchError, Pattern, apply, apply_all, revert_all, test,
};
use pe_image::Image;
use pe_loader::{ImportRef, LoadOptions, Placement, Resolver, load};
use windows_sys::Win32::System::Memory::{
    MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, VirtualQuery,
};

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const DATA: u32 = 0x2000;
const PDATA: u32 = 0x3000;
const RELOC: u32 = 0x4000;
const DECOY: u32 = TEXT + 0x80;
const POINTER: u32 = TEXT + 0x100;
const SETUP: u32 = TEXT + 0x140;
const SETTINGS: u32 = TEXT + 0x1A0;
const FLOOR_A: u32 = DATA + 0x30;
const LIMIT_FLOOR: u32 = DATA + 0x34;
const FLAG: u32 = DATA + 0x38;

const DT: f32 = 0.007;
const ONE_THIRTIETH: f32 = 1.0 / 30.0;
const ONE_THREE_THOUSANDTH: f32 = 1.0 / 3000.0;

struct Code(Vec<u8>);

impl Code {
    fn at(&self) -> u32 {
        TEXT + self.0.len() as u32
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn relative(&mut self, opcode: &[u8], target: u32) {
        let next = self.at() + opcode.len() as u32 + 4;
        self.bytes(opcode);
        self.bytes(&(target.wrapping_sub(next) as i32).to_le_bytes());
    }
}

/// The two-stage clamp of the step as the executable has it, fed with a 7 ms frame, returning the step's bits.
fn floor_function() -> Vec<u8> {
    let mut code = Code(Vec::new());
    code.relative(&[0xC5, 0xFA, 0x10, 0x3D], DATA + 0x28);
    code.relative(&[0xC5, 0xFA, 0x10, 0x05], DATA + 0x2C);
    code.bytes(&[0xC5, 0xFA, 0x5D, 0xD7]);
    code.relative(&[0xC5, 0xFA, 0x10, 0x0D], DATA + 0x20);
    code.bytes(&[0xC5, 0xC2, 0xC2, 0xD9, 0x01]);
    code.bytes(&[0xC4, 0xE3, 0x69, 0x4A, 0xD1, 0x30]);
    code.relative(&[0xC5, 0xFA, 0x10, 0x0D], DATA + 0x24);
    code.bytes(&[0xC5, 0xFA, 0x5D, 0xDA]);
    code.bytes(&[0xC5, 0xEA, 0xC2, 0xD1, 0x01]);
    code.bytes(&[0xC4, 0xE3, 0x61, 0x4A, 0xD1, 0x20]);
    code.bytes(&[0xC5, 0xF9, 0x7E, 0xD0, 0xC3]);
    code.0
}

/// `mov rax, imm64` whose immediate holds the bytes of `mov r8d, -1`, then the real `mov r8d, -1`.
fn decoy_function() -> Vec<u8> {
    let mut code = vec![0x48, 0xB8, 0x41, 0xB8, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00];
    code.extend_from_slice(&[0x41, 0xB8, 0xFF, 0xFF, 0xFF, 0xFF, 0xC3]);
    code
}

/// `mov rax, imm64` with an address in it that the loader relocates.
fn pointer_function() -> Vec<u8> {
    let mut code = vec![0x48, 0xB8];
    code.extend_from_slice(&(BASE + u64::from(DATA)).to_le_bytes());
    code.push(0xC3);
    code
}

fn disp(next: u32, target: u32) -> [u8; 4] {
    (target.wrapping_sub(next) as i32).to_le_bytes()
}

/// The floor setup of the timer initialiser: both floors stored, then the flag set from the flags of an earlier test.
fn setup_function() -> Vec<u8> {
    let mut code = vec![0xC7, 0x05];
    code.extend_from_slice(&disp(SETUP + 10, FLOOR_A));
    code.extend_from_slice(&0x39AE_C33Eu32.to_le_bytes());
    code.extend_from_slice(&[0xC5, 0xFA, 0x11, 0x05]);
    code.extend_from_slice(&disp(SETUP + 18, LIMIT_FLOOR));
    code.extend_from_slice(&[0x0F, 0x94, 0x05]);
    code.extend_from_slice(&disp(SETUP + 25, FLAG));
    code.push(0xC3);
    code
}

/// `1 / limit` computed after a settings change and stored.
fn settings_function() -> Vec<u8> {
    let mut code = vec![0xC5, 0xF2, 0x5E, 0xC0, 0xC5, 0xFA, 0x11, 0x05];
    code.extend_from_slice(&disp(SETTINGS + 12, LIMIT_FLOOR));
    code.push(0xC3);
    code
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn image() -> Vec<u8> {
    let mut file = vec![0u8; 0x1000];
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
    put(&mut file, optional + 24, &BASE.to_le_bytes());
    put(&mut file, optional + 32, &0x1000u32.to_le_bytes());
    put(&mut file, optional + 36, &0x200u32.to_le_bytes());
    put(&mut file, optional + 56, &0x5000u32.to_le_bytes());
    put(&mut file, optional + 60, &0x400u32.to_le_bytes());
    put(&mut file, optional + 68, &3u16.to_le_bytes());
    put(&mut file, optional + 70, &0x160u16.to_le_bytes());
    put(&mut file, optional + 108, &16u32.to_le_bytes());
    for (index, (rva, size)) in [(3, (PDATA, 60)), (5, (RELOC, 24))] {
        put(&mut file, optional + 112 + 8 * index, &rva.to_le_bytes());
        put(
            &mut file,
            optional + 116 + 8 * index,
            &(size as u32).to_le_bytes(),
        );
    }
    let sections = [
        (".text", TEXT, 0x400u32, 0x6000_0020u32),
        (".data", DATA, 0x600, 0xC000_0040),
        (".pdata", PDATA, 0x800, 0x4000_0040),
        (".reloc", RELOC, 0xA00, 0x4200_0040),
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
    file.resize(0xC00, 0);
    let floor = floor_function();
    let decoy = decoy_function();
    let pointer = pointer_function();
    put(&mut file, 0x400, &floor);
    put(&mut file, 0x400 + 0x80, &decoy);
    put(&mut file, 0x400 + 0x100, &pointer);
    let setup = setup_function();
    let settings = settings_function();
    put(&mut file, 0x400 + 0x140, &setup);
    put(&mut file, 0x400 + 0x1A0, &settings);
    put(&mut file, 0x620, &ONE_THIRTIETH.to_le_bytes());
    put(&mut file, 0x624, &ONE_THREE_THOUSANDTH.to_le_bytes());
    put(&mut file, 0x628, &DT.to_le_bytes());
    put(&mut file, 0x62C, &(1.0f32 / 15.0).to_le_bytes());
    let entries = [
        (TEXT, TEXT + floor.len() as u32),
        (DECOY, DECOY + decoy.len() as u32),
        (POINTER, POINTER + pointer.len() as u32),
        (SETUP, SETUP + setup.len() as u32),
        (SETTINGS, SETTINGS + settings.len() as u32),
    ];
    for (index, (begin, end)) in entries.iter().enumerate() {
        let entry = [*begin, *end, PDATA + 0x80]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>();
        put(&mut file, 0x800 + 12 * index, &entry);
    }
    put(&mut file, 0x800 + 0x80, &[0x01, 0x00, 0x00, 0x00]);
    put(&mut file, 0xA00, &TEXT.to_le_bytes());
    put(&mut file, 0xA04, &12u32.to_le_bytes());
    put(
        &mut file,
        0xA08,
        &(0xA000u16 | (POINTER + 2 - TEXT) as u16).to_le_bytes(),
    );
    put(&mut file, 0xA0C, &DATA.to_le_bytes());
    put(&mut file, 0xA10, &12u32.to_le_bytes());
    put(&mut file, 0xA14, &0xA000u16.to_le_bytes());
    file
}

struct NoImports;

impl Resolver for NoImports {
    fn resolve(&mut self, _import: &ImportRef<'_>) -> usize {
        0
    }
}

fn step_of(module: &pe_loader::Module) -> f32 {
    // SAFETY: the entry point is the synthetic function above, which only reads its own constants.
    f32::from_bits(unsafe { module.call_entry() } as u32)
}

fn protection(module: &pe_loader::Module, rva: u32) -> u32 {
    // SAFETY: a plain query of an address inside the module's mapping.
    unsafe {
        let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        VirtualQuery(
            (module.base() + rva as usize) as *const _,
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        );
        info.Protect
    }
}

fn text_buffer(bytes: &[u8]) -> Buffer {
    let mut memory = Buffer(vec![0; 0x5000]);
    memory.0[TEXT as usize..TEXT as usize + 0x200].copy_from_slice(&bytes[0x400..0x600]);
    memory
}

#[test]
fn the_step_floor_is_found_once_and_the_decoy_is_not() {
    let bytes = image();
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let site = finder.find_one(&framerate::step_floor_pattern()).unwrap();
    assert_eq!(site.rva(), TEXT + 20);
    assert_eq!(site.instructions.len(), 7);

    let pattern = Pattern::new("mov r8d, -1").then(test::mov_imm32(0xFFFF_FFFF));
    let sites = finder.find(&pattern);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].rva(), DECOY + 10);
    assert_eq!(
        finder
            .find_one(&Pattern::new("none").then(test::mov_imm32(7)))
            .unwrap_err(),
        PatchError::NotFound {
            name: "none".into()
        }
    );
}

#[test]
fn the_floor_patch_redirects_one_displacement() {
    let bytes = image();
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patches = STEP_FLOOR.patches(&finder).unwrap();
    assert_eq!(patches.len(), 1);
    let patch = &patches[0];
    let load_at = TEXT + 20;
    assert_eq!(patch.rva, load_at + 4);
    assert_eq!(patch.original.len(), 4);
    let next = i64::from(load_at + 8);
    assert_eq!(
        i64::from(i32::from_le_bytes(
            patch.replacement[..4].try_into().unwrap()
        )),
        i64::from(DATA + 0x24) - next
    );
    assert_eq!(framerate::select(&finder, false, &[]).unwrap(), Vec::new());
    let mut all = patches;
    all.extend(CLOCK_FLOOR.patches(&finder).unwrap());
    // This image has none of the sites of the frame timers, which stop the whole selection.
    assert!(framerate::select(&finder, true, &[]).is_err());
    assert_eq!(all.len(), 3);
    // Leaving the frame timers out, as `fix_frame_timers = false` does, selects the floors alone.
    assert_eq!(
        framerate::select_without(&finder, true, &[], &[framerate::FRAME_TIMERS.id]).unwrap(),
        all
    );
}

#[test]
fn patches_apply_check_and_revert_on_a_buffer() {
    let bytes = image();
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patch = STEP_FLOOR.patches(&finder).unwrap().remove(0);
    let mut memory = text_buffer(&bytes);

    let applied = apply(&mut memory, &patch).unwrap();
    assert_eq!(memory.read(patch.rva, 4).unwrap(), patch.replacement);
    assert_eq!(
        apply(&mut memory, &patch).unwrap_err(),
        PatchError::AlreadyApplied {
            name: patch.name.clone()
        }
    );
    applied.revert(&mut memory).unwrap();
    assert_eq!(memory.read(patch.rva, 4).unwrap(), patch.original);

    memory.write(patch.rva, &[1, 2, 3, 4]).unwrap();
    assert_eq!(
        apply(&mut memory, &patch).unwrap_err(),
        PatchError::Mismatch {
            rva: patch.rva,
            expected: patch.original.clone(),
            found: vec![1, 2, 3, 4]
        }
    );
}

#[test]
fn apply_all_undoes_what_it_wrote_when_one_patch_fails() {
    let bytes = image();
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let good = STEP_FLOOR.patches(&finder).unwrap().remove(0);
    let mut bad = good.clone();
    bad.name = "wrong".into();
    bad.rva = DECOY + 10;
    bad.original = vec![0x90; 6];
    let mut memory = text_buffer(&bytes);
    let before = memory.0.clone();
    let error = apply_all(&mut memory, &[good.clone(), bad]).unwrap_err();
    assert!(matches!(error, PatchError::Mismatch { .. }));
    assert_eq!(memory.0, before);

    let applied = apply_all(&mut memory, std::slice::from_ref(&good)).unwrap();
    assert_ne!(memory.0, before);
    revert_all(&mut memory, applied);
    assert_eq!(memory.0, before);
}

#[test]
fn patches_must_cover_whole_unrelocated_instructions_in_code() {
    let bytes = image();
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let mov = DECOY + 10;
    assert!(matches!(
        finder.patch_instructions("inside", mov + 1, 5, &[]),
        Err(PatchError::NotBoundary { .. })
    ));
    assert!(matches!(
        finder.patch_instructions("short", mov, 3, &[]),
        Err(PatchError::NotBoundary { .. })
    ));
    assert!(matches!(
        finder.patch_instructions("data", DATA, 4, &[]),
        Err(PatchError::NotCode { .. })
    ));
    assert!(matches!(
        finder.patch_instructions("pointer", POINTER, 10, &[]),
        Err(PatchError::Relocated { .. })
    ));
    assert!(matches!(
        finder.patch_instructions("long", mov, 6, &[0x90; 7]),
        Err(PatchError::TooLong { .. })
    ));

    let site = finder
        .find_one(&Pattern::new("mov r8d, -1").then(test::mov_imm32(0xFFFF_FFFF)))
        .unwrap();
    let nop = finder.nop("nop", &site, 0).unwrap();
    assert_eq!(nop.replacement, vec![0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00]);
    assert_eq!(nop.original, vec![0x41, 0xB8, 0xFF, 0xFF, 0xFF, 0xFF]);
    let xor = finder
        .replace("xor", &site, 0, &[0x45, 0x31, 0xC0])
        .unwrap();
    assert_eq!(xor.replacement, vec![0x45, 0x31, 0xC0, 0x0F, 0x1F, 0x00]);
    let immediate = finder
        .set_immediate("immediate", &site, 0, &60u32.to_le_bytes())
        .unwrap();
    assert_eq!(immediate.rva, mov + 2);
    assert_eq!(immediate.replacement, 60u32.to_le_bytes());
    assert!(matches!(
        finder.set_immediate("size", &site, 0, &[1, 2]),
        Err(PatchError::TooLong { .. })
    ));
    assert!(matches!(
        finder.set_displacement("none", &site, 0, 4),
        Err(PatchError::NoOperand { .. })
    ));
}

#[test]
fn a_mapped_image_runs_patched_and_reverted_without_ever_being_writable_and_executable() {
    let bytes = image();
    let module = load(
        &bytes,
        &mut NoImports,
        &LoadOptions {
            placement: Placement::Anywhere,
            helpers: None,
        },
    )
    .unwrap();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let patch = STEP_FLOOR.patches(&finder).unwrap().remove(0);
    // SAFETY: `module` stays mapped for as long as `memory` is used.
    let mut memory = unsafe { Mapped::new(module.base(), module.size()) };

    assert_eq!(step_of(&module), ONE_THIRTIETH);
    let applied = apply(&mut memory, &patch).unwrap();
    assert_eq!(step_of(&module), DT);
    assert_eq!(protection(&module, patch.rva), PAGE_EXECUTE_READ);
    applied.revert(&mut memory).unwrap();
    assert_eq!(step_of(&module), ONE_THIRTIETH);
    assert_eq!(protection(&module, patch.rva), PAGE_EXECUTE_READ);

    assert!(matches!(
        memory.read(0x6000, 4),
        Err(PatchError::Outside { .. })
    ));
    assert!(matches!(
        memory.write(0x4FFF, &[0; 2]),
        Err(PatchError::Outside { .. })
    ));
}

fn decode(bytes: &[u8], at: u32) -> Vec<iced_x86::Instruction> {
    use iced_x86::{Decoder, DecoderOptions};
    let mut decoder = Decoder::with_ip(64, bytes, BASE + u64::from(at), DecoderOptions::NONE);
    let mut out = Vec::new();
    while decoder.can_decode() {
        out.push(decoder.decode());
    }
    out
}

#[test]
fn the_clock_floor_stores_become_stores_of_the_first_floor() {
    use iced_x86::{Mnemonic, Register};
    let bytes = image();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let patches = CLOCK_FLOOR.patches(&finder).unwrap();
    assert_eq!(patches.len(), 2);

    let setup = &patches[0];
    assert_eq!(setup.rva, SETUP);
    assert_eq!(setup.original.len(), 18);
    assert_eq!(setup.replacement.len(), 18);
    let code = decode(&setup.replacement, SETUP);
    assert_eq!(code[0].mnemonic(), Mnemonic::Mov);
    assert_eq!(code[0].op0_register(), Register::EAX);
    assert_eq!(code[0].immediate32(), 0x39AE_C33E);
    for (instruction, target) in [(code[1], FLOOR_A), (code[2], LIMIT_FLOOR)] {
        assert_eq!(instruction.mnemonic(), Mnemonic::Mov);
        assert_eq!(instruction.op1_register(), Register::EAX);
        assert_eq!(
            instruction.ip_rel_memory_address(),
            BASE + u64::from(target)
        );
    }
    assert_eq!(code[3].mnemonic(), Mnemonic::Nop);

    let settings = &patches[1];
    assert_eq!(settings.rva, SETTINGS);
    assert_eq!(settings.original.len(), 12);
    let code = decode(&settings.replacement, SETTINGS);
    assert_eq!(code[0].mnemonic(), Mnemonic::Mov);
    assert_eq!(code[0].immediate32(), 0x39AE_C33E);
    assert_eq!(
        code[0].ip_rel_memory_address(),
        BASE + u64::from(LIMIT_FLOOR)
    );
    assert_eq!(code[1].mnemonic(), Mnemonic::Nop);
    assert_eq!(code[0].len() + code[1].len(), 12);

    let mut memory = text_buffer(&bytes);
    let before = memory.0.clone();
    let applied = apply_all(&mut memory, &patches).unwrap();
    assert_ne!(memory.0, before);
    revert_all(&mut memory, applied);
    assert_eq!(memory.0, before);
}

#[test]
fn the_settings_store_is_matched_on_the_address_the_setup_stores_to() {
    let mut bytes = image();
    let at = 0x400 + 0x1A0 + 8;
    bytes[at..at + 4].copy_from_slice(&disp(SETTINGS + 12, FLAG));
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    assert!(matches!(
        CLOCK_FLOOR.patches(&finder),
        Err(PatchError::NotFound { .. })
    ));
}
