//! The facts `enhancements::camera` records, checked against the real executable read as plain bytes (never loaded
//! or run). Skipped without `LEONIDA_GAME_DIR`.

use enhancements::camera::{
    CONTROL_HASH_TABLE, LOOK_LR, LOOK_LR_HASH, LOOK_UD, LOOK_UD_HASH, control_handle_offset,
};
use enhancements::patch::{Finder, Pattern};
use iced_x86::{Mnemonic, OpKind, Register};
use pe_image::Image;

fn executable() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let largest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "exe"))
        .max_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0))?;
    std::fs::read(largest.path()).ok()
}

fn dword(image: &Image, rva: u32) -> u32 {
    u32::from_le_bytes(image.read(rva, 4).unwrap().try_into().unwrap())
}

fn float(image: &Image, rva: u32) -> f32 {
    f32::from_bits(dword(image, rva))
}

#[test]
fn the_look_controls_are_where_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    assert_eq!(
        dword(&image, CONTROL_HASH_TABLE + 4 * LOOK_LR),
        LOOK_LR_HASH
    );
    assert_eq!(
        dword(&image, CONTROL_HASH_TABLE + 4 * LOOK_UD),
        LOOK_UD_HASH
    );
    assert_eq!(control_handle_offset(LOOK_UD), 0x2a6c);
}

#[test]
fn the_look_helper_constants_are_what_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    assert_eq!(float(&image, 0x0615_a2c4), 5.0);
    assert!((float(&image, 0x05fc_6f98) - 0.9).abs() < 1e-6);
    assert!((float(&image, 0x05fc_6f00) - 1e-6).abs() < 1e-9);
    assert!((float(&image, 0x0615_9b38) - std::f32::consts::PI / 180.0).abs() < 1e-7);
}

/// The two products of the helper, `speed * envelope` and then `* raw_x`, which a patch for the cameras that use
/// it would add the mouse to.
fn helper_products() -> Pattern {
    let speed_times_envelope = |i: &iced_x86::Instruction, _: &enhancements::patch::Probe| {
        i.mnemonic() == Mnemonic::Vmulss
            && i.op0_register() == Register::XMM0
            && i.op1_register() == Register::XMM9
            && i.op2_register() == Register::XMM0
    };
    let times_raw_x = |i: &iced_x86::Instruction, _: &enhancements::patch::Probe| {
        i.mnemonic() == Mnemonic::Vmulss
            && i.op0_register() == Register::XMM1
            && i.op2_kind() == OpKind::Memory
            && i.memory_base() == Register::RDI
    };
    Pattern::new("look helper products")
        .then(speed_times_envelope)
        .then(times_raw_x)
}

#[test]
fn the_look_helper_products_are_where_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let function = image.function_containing(0x04a2_0480).unwrap();
    assert_eq!(function.begin, 0x04a2_0480);
    let sites = finder.find_in(function, &helper_products());
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].rva(), 0x04a2_0787);
    assert_eq!(sites[0].instructions[1].rva, 0x04a2_078b);
}
