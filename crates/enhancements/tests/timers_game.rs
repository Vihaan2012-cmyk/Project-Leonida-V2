//! The frame timer patches against the real executable, read as plain bytes (never loaded or run). Skipped without
//! `LEONIDA_GAME_DIR`.

use enhancements::frame_timers::{self, DT_FLOOR_FUNCTIONS, FLOOR_BITS, TICK_SITES};
use enhancements::framerate::{DT_FLOOR, FRAME_TIMERS};
use enhancements::patch::{Finder, Patch};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic, OpKind};
use pe_image::Image;

const BASE: u64 = 0x1_4000_0000;

fn executable() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let largest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "exe"))
        .max_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0))?;
    std::fs::read(largest.path()).ok()
}

fn decode(bytes: &[u8], rva: u32) -> Vec<iced_x86::Instruction> {
    let mut decoder = Decoder::with_ip(64, bytes, BASE + u64::from(rva), DecoderOptions::NONE);
    let mut out = Vec::new();
    while decoder.can_decode() {
        out.push(decoder.decode());
    }
    out
}

fn target(instruction: &iced_x86::Instruction) -> u32 {
    (instruction.near_branch_target() - BASE) as u32
}

#[test]
fn every_frame_delta_site_is_where_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patches = FRAME_TIMERS.patches(&finder).unwrap();
    assert_eq!(patches.len(), 2 + TICK_SITES.len());
    let (cave, rest) = patches.split_first().unwrap();
    let (hook, jumps) = rest.split_first().unwrap();

    for site in &TICK_SITES {
        let function = image.function_containing(site.rva).unwrap();
        assert_eq!(function.begin, site.function, "{:#x}", site.rva);
        assert!(site.len >= 5);
        let code = decode(
            image.read(site.rva, usize::from(site.len)).unwrap(),
            site.rva,
        );
        assert_eq!(code[0].mnemonic(), Mnemonic::Sub, "{:#x}", site.rva);
        assert_eq!(code[0].op0_kind(), OpKind::Register);
        if let Some(frames) = site.frames {
            let after = image.read(site.rva, 24).unwrap();
            let compare = decode(after, site.rva)
                .into_iter()
                .find(|i| i.mnemonic() == Mnemonic::Cmp && i.op1_kind() != OpKind::Memory)
                .unwrap();
            assert_eq!(compare.immediate32(), frames, "{:#x}", site.rva);
        }
    }

    assert_eq!(hook.rva, 0x0222_6759);
    let wrapper = decode(&cave.replacement[16..], cave.rva + 16);
    assert!(wrapper.iter().all(|i| !i.is_invalid()));
    assert_eq!(wrapper[0].mnemonic(), Mnemonic::Sub);
    assert_eq!(wrapper[1].mnemonic(), Mnemonic::Call);
    assert_eq!(target(&wrapper[1]), 0x0222_6040);
    assert_eq!(
        target(&decode(&hook.replacement, hook.rva)[0]),
        cave.rva + 16
    );

    let mut returns: Vec<u32> = decode(&cave.replacement[16..], cave.rva + 16)
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Jmp)
        .map(target)
        .collect();
    returns.sort_unstable();
    let mut expected: Vec<u32> = TICK_SITES
        .iter()
        .map(|s| s.rva + u32::from(s.len))
        .collect();
    expected.sort_unstable();
    assert_eq!(returns, expected);

    for (patch, site) in jumps.iter().zip(&TICK_SITES) {
        assert_eq!(patch.rva, site.rva);
        assert_eq!(patch.original.len(), usize::from(site.len));
        let jump = &decode(&patch.replacement, patch.rva)[0];
        assert_eq!(jump.mnemonic(), Mnemonic::Jmp);
        assert!(
            target(jump) >= cave.rva && target(jump) < cave.rva + cave.replacement.len() as u32
        );
    }
    let end = cave.rva + cave.replacement.len() as u32;
    assert!(end <= 0x05f4_1000 - frame_timers::DT_SLOT);
    assert!(cave.rva >= 0x05f4_0a00);
    assert!(cave.original.iter().all(|b| *b == 0));
}

#[test]
fn the_stubs_end_where_the_patched_code_resumes() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patches = FRAME_TIMERS.patches(&finder).unwrap();
    let cave = &patches[0];
    let code = decode(&cave.replacement[16..], cave.rva + 16);
    for site in &TICK_SITES {
        let resume = site.rva + u32::from(site.len);
        let at = code
            .iter()
            .position(|i| i.mnemonic() == Mnemonic::Jmp && target(i) == resume)
            .unwrap();
        let stub = &code[..=at];
        let start = stub
            .iter()
            .rposition(|i| i.mnemonic() == Mnemonic::Int3)
            .map_or(0, |p| p + 1);
        let stub = &stub[start..];
        assert_eq!(stub[0].mnemonic(), Mnemonic::Sub, "{:#x}", site.rva);
        assert!(stub.iter().all(|i| i.flow_control() != FlowControl::Call));
        assert!(stub.iter().any(|i| i.mnemonic() == Mnemonic::Imul));
    }
}

fn floor_patches(finder: &Finder) -> Vec<Patch> {
    DT_FLOOR.patches(finder).unwrap()
}

#[test]
fn the_time_step_floors_are_where_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patches = floor_patches(&finder);
    assert_eq!(patches.len(), 1 + 2 * DT_FLOOR_FUNCTIONS.len());
    let constant = &patches[0];
    assert_eq!(constant.rva, 0x05f4_0ff0);
    assert_eq!(constant.replacement, FLOOR_BITS.to_le_bytes());
    let mut seen: Vec<u32> = Vec::new();
    for patch in &patches[1..] {
        let function = image.function_containing(patch.rva).unwrap();
        assert!(
            DT_FLOOR_FUNCTIONS.contains(&function.begin),
            "{:#x}",
            patch.rva
        );
        seen.push(patch.rva);
        let displacement = i32::from_le_bytes(patch.replacement[..4].try_into().unwrap());
        let old = i32::from_le_bytes(patch.original[..4].try_into().unwrap());
        // the displacement ends the instruction, or is followed by one immediate byte
        let next = patch.rva + 4;
        let literal = (i64::from(next) + i64::from(old)) as u32;
        assert!(
            literal == 0x0615_9e10 || literal + 1 == 0x0615_9e10,
            "{literal:#x}"
        );
        let new = (i64::from(next) + i64::from(displacement)) as u32;
        assert!(new == constant.rva || new + 1 == constant.rva, "{new:#x}");
    }
    for rva in [
        0x00b7_4dfb,
        0x00b7_4e07,
        0x0124_21e4,
        0x0124_21ee,
        0x031e_5c83,
        0x031e_5c8d,
        0x031e_69cb,
        0x031e_69d5,
    ] {
        assert!(seen.contains(&rva), "{rva:#x}");
    }
}
