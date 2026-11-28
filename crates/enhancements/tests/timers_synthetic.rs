//! The frame timer patches on a small executable built byte by byte, loaded, patched and run.
#![cfg(windows)]
#![allow(unsafe_code)]

use enhancements::frame_timers::{
    self, DEFICIT_OFFSET, DT_SLOT, FLOOR_BITS, Layout, TICK_SLOT, TickSite,
};
use enhancements::patch::{Buffer, CodeMemory, Finder, Mapped, PatchError, apply_all, revert_all};
use enhancements::widescreen::fnv1a64;
use pe_image::Image;
use pe_loader::{ImportRef, LoadOptions, Module, Placement, Resolver, load};

const TEXT: u32 = 0x1000;
const DATA: u32 = 0x2000;
const PDATA: u32 = 0x3000;
const RELOC: u32 = 0x4000;
const SLACK_START: u32 = TEXT + 0x400;
const LITERAL: u32 = DATA;
const CAP: u32 = DATA + 4;
const RECORD: u32 = DATA + 0x40;
const FRAME: u32 = RECORD + 4;
const DEFICIT: u32 = RECORD + DEFICIT_OFFSET;
const INPUT: u32 = DATA + 0x80;

const UPDATE: u32 = TEXT;
const CLOCK: u32 = TEXT + 0x40;
const READER_1: u32 = TEXT + 0x60;
const READER_1_FLAG: u32 = TEXT + 0x80;
const READER_2: u32 = TEXT + 0xA0;
const READER_3: u32 = TEXT + 0xE0;
const READER_4: u32 = TEXT + 0x120;
const READER_BAD: u32 = TEXT + 0x160;
const FLOOR: u32 = TEXT + 0x1C0;

struct Code {
    at: u32,
    bytes: Vec<u8>,
}

impl Code {
    fn new(at: u32) -> Self {
        Self {
            at,
            bytes: Vec::new(),
        }
    }

    fn put(&mut self, bytes: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(bytes);
        self
    }

    /// An instruction with a RIP-relative operand: `opcode`, the displacement to `target`, then `tail`.
    fn rip(&mut self, opcode: &[u8], target: u32, tail: &[u8]) -> &mut Self {
        let next = self.at + (self.bytes.len() + opcode.len() + 4 + tail.len()) as u32;
        self.put(opcode);
        self.put(&(target.wrapping_sub(next) as i32).to_le_bytes());
        self.put(tail)
    }
}

fn update() -> Vec<u8> {
    let mut c = Code::new(UPDATE);
    c.put(&[0x48, 0x83, 0xEC, 0x28]);
    c.rip(&[0x89, 0x0D], INPUT, &[]);
    c.rip(&[0x48, 0x8D, 0x0D], RECORD, &[]);
    c.put(&[0x41, 0xB1, 0x01]);
    c.rip(&[0xE8], CLOCK, &[]);
    c.put(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
    c.bytes
}

fn clock() -> Vec<u8> {
    let mut c = Code::new(CLOCK);
    c.rip(&[0x8B, 0x05], INPUT, &[]);
    c.put(&[0x89, 0x41, 0x0C, 0xC3]);
    c.bytes
}

/// `mov eax, [frame]; sub eax, [rcx+214h]; cmp eax, 5Ah; ret`
fn reader_1() -> Vec<u8> {
    let mut c = Code::new(READER_1);
    c.rip(&[0x8B, 0x05], FRAME, &[]);
    c.put(&[0x2B, 0x81, 0x14, 0x02, 0x00, 0x00, 0x83, 0xF8, 0x5A, 0xC3]);
    c.bytes
}

/// The same, answering whether the delta reached 90.
fn reader_1_flag() -> Vec<u8> {
    let mut c = Code::new(READER_1_FLAG);
    c.rip(&[0x8B, 0x05], FRAME, &[]);
    c.put(&[0x2B, 0x81, 0x14, 0x02, 0x00, 0x00, 0x83, 0xF8, 0x5A]);
    c.put(&[0x0F, 0x93, 0xC0, 0x0F, 0xB6, 0xC0, 0xC3]);
    c.bytes
}

/// `mov edx, [frame]; mov ecx, [rcx+158h]; sub edx, ecx; cmp edx, 3Bh; mov eax, edx`
fn reader_2() -> Vec<u8> {
    let mut c = Code::new(READER_2);
    c.rip(&[0x8B, 0x15], FRAME, &[]);
    c.put(&[0x8B, 0x89, 0x58, 0x01, 0x00, 0x00]);
    c.put(&[0x29, 0xCA, 0x83, 0xFA, 0x3B, 0x89, 0xD0, 0xC3]);
    c.bytes
}

/// `mov r8d, [frame]; mov edx, [rcx+150h]; sub r8d, edx; cmp r8d, 3Bh; mov eax, r8d`
fn reader_3() -> Vec<u8> {
    let mut c = Code::new(READER_3);
    c.rip(&[0x44, 0x8B, 0x05], FRAME, &[]);
    c.put(&[0x8B, 0x91, 0x50, 0x01, 0x00, 0x00]);
    c.put(&[
        0x41, 0x29, 0xD0, 0x41, 0x83, 0xF8, 0x3B, 0x44, 0x89, 0xC0, 0xC3,
    ]);
    c.bytes
}

/// A site whose span holds a second instruction that uses `rcx`:
/// `mov r10d, [frame]; sub r10d, [rcx+70h]; mov rcx, [rcx+48h]; cmp r10d, [rcx+24h]; mov eax, r10d`
fn reader_4() -> Vec<u8> {
    let mut c = Code::new(READER_4);
    c.rip(&[0x44, 0x8B, 0x15], FRAME, &[]);
    c.put(&[0x44, 0x2B, 0x51, 0x70, 0x48, 0x8B, 0x49, 0x48]);
    c.put(&[0x44, 0x3B, 0x51, 0x24, 0x44, 0x89, 0xD0, 0xC3]);
    c.bytes
}

/// A delta whose flags are read straight away: not a site the patch may take.
fn reader_bad() -> Vec<u8> {
    let mut c = Code::new(READER_BAD);
    c.rip(&[0x8B, 0x05], FRAME, &[]);
    c.put(&[0x2B, 0x81, 0x14, 0x02, 0x00, 0x00, 0x72, 0x01, 0x90, 0xC3]);
    c.bytes
}

/// The shape of the entity step getter: floor 1/60 s as a compare and a blend value, cap 0.1 s.
fn floor_function() -> Vec<u8> {
    let mut c = Code::new(FLOOR);
    c.rip(&[0xC5, 0xFA, 0x10, 0x0D], CAP, &[]);
    c.rip(&[0xC4, 0xE2, 0x79, 0x18, 0x15], LITERAL, &[]);
    c.put(&[0xC5, 0xF2, 0x5D, 0xC8]);
    c.rip(&[0xC5, 0xFA, 0xC2, 0x05], LITERAL, &[0x01]);
    c.put(&[0xC4, 0xE3, 0x71, 0x4A, 0xC2, 0x00, 0xC3]);
    c.bytes
}

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

fn functions() -> Vec<(u32, Vec<u8>)> {
    vec![
        (UPDATE, update()),
        (CLOCK, clock()),
        (READER_1, reader_1()),
        (READER_1_FLAG, reader_1_flag()),
        (READER_2, reader_2()),
        (READER_3, reader_3()),
        (READER_4, reader_4()),
        (READER_BAD, reader_bad()),
        (FLOOR, floor_function()),
    ]
}

fn image() -> Vec<u8> {
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
    for (index, (rva, size)) in [(3, (PDATA, 12 * functions().len())), (5, (RELOC, 12))] {
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
    for (rva, code) in functions() {
        put(&mut file, 0x400 + (rva - TEXT) as usize, &code);
    }
    put(
        &mut file,
        0x800 + (LITERAL - DATA) as usize,
        &(1.0f32 / 60.0).to_le_bytes(),
    );
    put(
        &mut file,
        0x800 + (CAP - DATA) as usize,
        &0.1f32.to_le_bytes(),
    );
    for (index, (begin, code)) in functions().iter().enumerate() {
        let entry = [*begin, begin + code.len() as u32, PDATA + 0x100]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>();
        put(&mut file, 0xA00 + 12 * index, &entry);
    }
    put(&mut file, 0xA00 + 0x100, &[0x01, 0x00, 0x00, 0x00]);
    put(&mut file, 0xC00, &DATA.to_le_bytes());
    put(&mut file, 0xC04, &12u32.to_le_bytes());
    file
}

struct NoImports;

impl Resolver for NoImports {
    fn resolve(&mut self, _import: &ImportRef<'_>) -> usize {
        0
    }
}

fn sites(bytes: &[u8]) -> Vec<TickSite> {
    let image = Image::parse(bytes).unwrap();
    let site = |rva: u32, len: u8, frames: Option<u32>, function: u32| TickSite {
        rva,
        len,
        hash: fnv1a64(image.read(rva, usize::from(len)).unwrap()),
        frames,
        function,
        what: "synthetic",
    };
    vec![
        site(READER_1 + 6, 6, Some(90), READER_1),
        site(READER_1_FLAG + 6, 6, Some(90), READER_1_FLAG),
        site(READER_2 + 12, 5, Some(59), READER_2),
        site(READER_3 + 13, 7, Some(59), READER_3),
        site(READER_4 + 7, 8, None, READER_4),
    ]
}

fn layout<'a>(bytes: &[u8], sites: &'a [TickSite], floors: &'a [u32]) -> Layout<'a> {
    let image = Image::parse(bytes).unwrap();
    let call_at = UPDATE + 4 + 6 + 7 + 3;
    Layout {
        record: RECORD,
        update: UPDATE,
        hook_hash: fnv1a64(image.read(call_at, 5).unwrap()),
        sites,
        floors,
    }
}

fn patched() -> (Module, Vec<enhancements::patch::Applied>, Vec<u8>) {
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
    let list = sites(&bytes);
    let layout = layout(&bytes, &list, &[FLOOR]);
    let mut patches = frame_timers::dt_floor_patches(&finder, &layout).unwrap();
    patches.extend(frame_timers::tick_scale_patches(&finder, &layout).unwrap());
    // SAFETY: `module` stays mapped for as long as `memory` is used.
    let mut memory = unsafe { Mapped::new(module.base(), module.size()) };
    let applied = apply_all(&mut memory, &patches).unwrap();
    (module, applied, bytes)
}

fn word(module: &Module, rva: u32) -> *mut u32 {
    (module.base() + rva as usize) as *mut u32
}

fn set_clock(module: &Module, frame: u32, deficit: u32) {
    // SAFETY: both words are in the synthetic image's writable data section.
    unsafe {
        word(module, FRAME).write(frame);
        word(module, DEFICIT).write(deficit);
    }
}

fn call_reader(module: &Module, rva: u32, stamps: &[(usize, u32)]) -> u32 {
    let mut object = vec![0u32; 0x100];
    for (offset, value) in stamps {
        object[offset / 4] = *value;
    }
    // SAFETY: the reader is a synthetic function that reads the clock and the object it is given.
    let function: extern "system" fn(*const u32) -> u32 =
        unsafe { std::mem::transmute(module.base() + rva as usize) };
    function(object.as_ptr())
}

fn call_update(module: &Module, step: f32) {
    // SAFETY: the synthetic update stores the step and calls the clock function with the record.
    let function: extern "system" fn(u32) =
        unsafe { std::mem::transmute(module.base() + UPDATE as usize) };
    function(step.to_bits());
}

fn deficit(module: &Module) -> u32 {
    // SAFETY: a word of the synthetic data section.
    unsafe { word(module, DEFICIT).read() }
}

fn floor_of(module: &Module, step: f32) -> f32 {
    // SAFETY: the synthetic floor function takes and returns a float in xmm0.
    let function: extern "system" fn(f32) -> f32 =
        unsafe { std::mem::transmute(module.base() + FLOOR as usize) };
    function(step)
}

#[test]
fn deltas_are_untouched_without_a_deficit() {
    let (module, _applied, _) = patched();
    set_clock(&module, 1000, 0);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 900)]), 100);
    assert_eq!(call_reader(&module, READER_2, &[(0x158, 941)]), 59);
    assert_eq!(call_reader(&module, READER_3, &[(0x150, 940)]), 60);
    assert_eq!(call_reader(&module, READER_1_FLAG, &[(0x214, 910)]), 1);
    assert_eq!(call_reader(&module, READER_1_FLAG, &[(0x214, 911)]), 0);
}

#[test]
fn deltas_shrink_by_the_deficit_and_the_compare_follows_them() {
    let (module, _applied, _) = patched();
    set_clock(&module, 1000, 32768);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 900)]), 50);
    assert_eq!(call_reader(&module, READER_2, &[(0x158, 800)]), 100);
    assert_eq!(call_reader(&module, READER_3, &[(0x150, 900)]), 50);
    assert_eq!(call_reader(&module, READER_1_FLAG, &[(0x214, 910)]), 0);
    assert_eq!(call_reader(&module, READER_1_FLAG, &[(0x214, 820)]), 1);
    set_clock(&module, 1000, 49152);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 600)]), 100);
}

#[test]
fn the_delta_rounds_up_and_a_negative_one_is_left_alone() {
    let (module, _applied, _) = patched();
    set_clock(&module, 1000, 32768);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 999)]), 1);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 997)]), 2);
    assert_eq!(call_reader(&module, READER_1, &[(0x214, 1000)]), 0);
    assert_eq!(
        call_reader(&module, READER_1, &[(0x214, 1016)]),
        (-16i32) as u32
    );
}

#[test]
fn a_copied_instruction_after_the_delta_still_runs() {
    let (module, _applied, _) = patched();
    set_clock(&module, 1000, 32768);
    // delta 40 against a limit of 30 read through the replaced rcx: the register survives the stub
    let mut type_object = [0u32; 0x20];
    type_object[0x24 / 4] = 30;
    let mut object = vec![0u32; 0x100];
    object[0x70 / 4] = 960;
    let pointer = type_object.as_ptr() as usize;
    object[0x48 / 4] = pointer as u32;
    object[0x48 / 4 + 1] = (pointer >> 32) as u32;
    // SAFETY: reader 4 reads the clock, a stamp at +0x70 and a count through the pointer at +0x48.
    let function: extern "system" fn(*const u32) -> u32 =
        unsafe { std::mem::transmute(module.base() + READER_4 as usize) };
    assert_eq!(function(object.as_ptr()), 20);
    set_clock(&module, 1000, 0);
    assert_eq!(function(object.as_ptr()), 40);
}

#[test]
fn the_deficit_follows_the_step_and_stays_zero_up_to_sixty_fps() {
    let (module, _applied, _) = patched();
    set_clock(&module, 0, 0);
    for step in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 45.0] {
        for _ in 0..50 {
            call_update(&module, step);
        }
        assert_eq!(deficit(&module), 0, "step {step}");
    }
    for _ in 0..200 {
        call_update(&module, 1.0 / 120.0);
    }
    let half = deficit(&module);
    assert!((32768 - 8..=32768 + 8).contains(&half), "{half}");
    for _ in 0..200 {
        call_update(&module, 1.0 / 240.0);
    }
    let quarter = deficit(&module);
    assert!((49152 - 8..=49152 + 8).contains(&quarter), "{quarter}");
    for _ in 0..200 {
        call_update(&module, 1.0 / 60.0);
    }
    assert!(deficit(&module) <= 4, "{}", deficit(&module));
}

#[test]
fn one_slow_frame_moves_the_deficit_by_an_eighth() {
    let (module, _applied, _) = patched();
    set_clock(&module, 0, 0);
    for _ in 0..300 {
        call_update(&module, 1.0 / 120.0);
    }
    let before = deficit(&module);
    call_update(&module, 1.0 / 20.0);
    let after = deficit(&module);
    let expected = f64::from(before) - f64::from(before) / 8.0;
    assert!(
        (f64::from(after) - expected).abs() < 3.0,
        "{before} {after}"
    );
}

#[test]
fn a_degenerate_step_does_not_break_the_deficit() {
    let (module, _applied, _) = patched();
    set_clock(&module, 0, 0);
    for step in [0.0f32, f32::NAN, f32::INFINITY, 1.0 / 3000.0] {
        for _ in 0..40 {
            call_update(&module, step);
        }
        assert!(deficit(&module) < 65536, "{step}");
    }
}

#[test]
fn the_step_floor_is_one_three_thousandth_and_the_cap_stays() {
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
    assert_eq!(floor_of(&module, 1.0 / 120.0), 1.0 / 60.0);
    assert_eq!(floor_of(&module, 0.5), 0.1);
    let (patched_module, _applied, _) = patched();
    assert_eq!(floor_of(&patched_module, 1.0 / 120.0), 1.0 / 120.0);
    assert_eq!(
        floor_of(&patched_module, 1.0 / 5000.0),
        f32::from_bits(FLOOR_BITS)
    );
    assert_eq!(floor_of(&patched_module, 1.0 / 30.0), 1.0 / 30.0);
    assert_eq!(floor_of(&patched_module, 0.5), 0.1);
}

#[test]
fn patches_revert_to_the_original_image() {
    let bytes = image();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let list = sites(&bytes);
    let layout = layout(&bytes, &list, &[FLOOR]);
    let mut patches = frame_timers::dt_floor_patches(&finder, &layout).unwrap();
    patches.extend(frame_timers::tick_scale_patches(&finder, &layout).unwrap());
    let mut memory = Buffer(vec![0; 0x5000]);
    memory.0[TEXT as usize..TEXT as usize + 0x400].copy_from_slice(&bytes[0x400..0x800]);
    memory.0[DATA as usize..DATA as usize + 0x200].copy_from_slice(&bytes[0x800..0xA00]);
    let before = memory.0.clone();
    let applied = apply_all(&mut memory, &patches).unwrap();
    assert_ne!(memory.0, before);
    revert_all(&mut memory, applied);
    assert_eq!(memory.0, before);
}

#[test]
fn the_cave_lives_in_the_slack_after_the_code_and_nothing_else_is_written_there() {
    let bytes = image();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let list = sites(&bytes);
    let layout = layout(&bytes, &list, &[FLOOR]);
    let floor = frame_timers::dt_floor_patches(&finder, &layout).unwrap();
    let constant = &floor[0];
    assert_eq!(constant.rva, DATA - DT_SLOT);
    assert_eq!(constant.replacement, FLOOR_BITS.to_le_bytes());
    assert_eq!(constant.original, vec![0; 4]);
    let tick = frame_timers::tick_scale_patches(&finder, &layout).unwrap();
    let cave = &tick[0];
    assert_eq!(cave.rva, DATA - DT_SLOT - TICK_SLOT);
    assert!(cave.rva >= SLACK_START);
    assert!(cave.replacement.len() <= TICK_SLOT as usize);
    assert!(cave.original.iter().all(|b| *b == 0));
    for patch in floor.iter().skip(1).chain(tick.iter().skip(1)) {
        assert!(
            patch.rva < SLACK_START || patch.rva >= DATA,
            "{}",
            patch.name
        );
        assert_eq!(patch.original.len(), patch.replacement.len());
    }
    let mut memory = Buffer(vec![0; 0x5000]);
    memory.write(cave.rva, &[1]).unwrap();
    assert!(matches!(
        apply_all(&mut memory, &tick),
        Err(PatchError::Mismatch { .. })
    ));
}

#[test]
fn a_site_whose_flags_are_read_at_once_or_whose_code_changed_is_refused() {
    let bytes = image();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let mut list = sites(&bytes);
    let layout_for = |sites: &[TickSite]| {
        let l = layout(&bytes, sites, &[FLOOR]);
        frame_timers::tick_scale_patches(&finder, &l)
    };
    let bad = TickSite {
        rva: READER_BAD + 6,
        len: 6,
        hash: fnv1a64(parsed.read(READER_BAD + 6, 6).unwrap()),
        frames: None,
        function: READER_BAD,
        what: "synthetic",
    };
    let mut with_bad = list.clone();
    with_bad.push(bad);
    assert!(matches!(
        layout_for(&with_bad),
        Err(PatchError::NotFound { name }) if name.contains("flags")
    ));
    list[0].hash ^= 1;
    assert!(matches!(
        layout_for(&list),
        Err(PatchError::NotFound { name }) if name.contains("code changed")
    ));
}

#[test]
fn the_floor_patch_wants_exactly_the_functions_it_was_told_about() {
    let bytes = image();
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let list = sites(&bytes);
    let none = layout(&bytes, &list, &[]);
    assert!(matches!(
        frame_timers::dt_floor_patches(&finder, &none),
        Err(PatchError::Ambiguous { count: 1, .. })
    ));
    let other = layout(&bytes, &list, &[CLOCK]);
    assert!(matches!(
        frame_timers::dt_floor_patches(&finder, &other),
        Err(PatchError::Ambiguous { .. })
    ));
}
