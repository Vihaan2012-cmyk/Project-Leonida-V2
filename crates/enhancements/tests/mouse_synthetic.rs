//! The mouse look patch on a small executable built byte by byte, loaded, patched and run.
#![cfg(windows)]
#![allow(unsafe_code)]

use enhancements::camera_mouse::{self, Layout, MouseLookSettings};
use enhancements::patch::{Finder, Mapped, PatchError, apply_all, revert_all};
use enhancements::widescreen::fnv1a64;
use pe_image::Image;
use pe_loader::{ImportRef, LoadOptions, Module, Placement, Resolver, load};
use shims::gameinput::direct;

const TEXT: u32 = 0x1000;
const DATA: u32 = 0x2000;
const PDATA: u32 = 0x3000;
const RELOC: u32 = 0x4000;
const UPDATE: u32 = TEXT;
const HOOK: u32 = UPDATE + 9 + 24;
const OBJECT_LEN: usize = 0x1d00;

fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

/// The shape of the look controller update around the two stores: `rsi` is the object, `rax` is live.
fn update(decoy: bool) -> Vec<u8> {
    let mut code = vec![0x56, 0x48, 0x89, 0xCE, 0xB8, 0x5A, 0x5A, 0x00, 0x00];
    code.extend_from_slice(&[0xC5, 0xFA, 0x10, 0x86, 0x90, 0x03, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x58, 0x86, 0x88, 0x03, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x59, 0x86, 0x8C, 0x03, 0x00, 0x00]);
    assert_eq!(UPDATE + code.len() as u32, HOOK);
    code.extend_from_slice(&[0xC5, 0xFA, 0x11, 0x86, 0xA0, 0x1C, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x10, 0x86, 0x28, 0x05, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x58, 0x86, 0x20, 0x05, 0x00, 0x00]);
    code.extend_from_slice(&[0xC5, 0xFA, 0x59, 0x86, 0x24, 0x05, 0x00, 0x00]);
    let store = if decoy { 0xA8 } else { 0xA4 };
    code.extend_from_slice(&[0xC5, 0xFA, 0x11, 0x86, store, 0x1C, 0x00, 0x00]);
    code.extend_from_slice(&[0x5E, 0xC3]);
    code
}

fn image(decoy: bool) -> Vec<u8> {
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
    let code = update(decoy);
    put(&mut file, 0x400, &code);
    let entry = [UPDATE, UPDATE + code.len() as u32, PDATA + 0x100]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect::<Vec<u8>>();
    put(&mut file, 0xA00, &entry);
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

fn layout(bytes: &[u8]) -> Layout {
    let image = Image::parse(bytes).unwrap();
    Layout {
        update: UPDATE,
        hash: fnv1a64(image.read(HOOK, camera_mouse::HOOK_LEN).unwrap()),
    }
}

fn load_image(bytes: &[u8]) -> Module {
    load(
        bytes,
        &mut NoImports,
        &LoadOptions {
            placement: Placement::Anywhere,
            helpers: None,
        },
    )
    .unwrap()
}

/// Runs the update on an object whose heading channel sums to `heading` and whose pitch channel sums to `pitch`
/// (both after their scale); returns the two deltas and rax.
fn run(module: &Module, heading: f32, pitch: f32) -> (f32, f32, u32) {
    let mut object = vec![0f32; OBJECT_LEN / 4];
    object[0x390 / 4] = heading / 2.0 - 0.25;
    object[0x388 / 4] = 0.25;
    object[0x38C / 4] = 2.0;
    object[0x528 / 4] = pitch / 3.0 + 0.5;
    object[0x520 / 4] = -0.5;
    object[0x524 / 4] = 3.0;
    // SAFETY: the synthetic update reads and writes the object it is given and returns the marker in rax.
    let function: extern "system" fn(*mut f32) -> u32 =
        unsafe { std::mem::transmute(module.base() + UPDATE as usize) };
    let marker = function(object.as_mut_ptr());
    (object[0x1CA0 / 4], object[0x1CA4 / 4], marker)
}

fn cell_of(module: &Module, built: &camera_mouse::MouseLookPatches) -> *mut i32 {
    (module.base() + built.cell as usize) as *mut i32
}

fn patched(settings: &MouseLookSettings) -> (Module, camera_mouse::MouseLookPatches) {
    let bytes = image(false);
    let module = load_image(&bytes);
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let built = camera_mouse::patches(&finder, &layout(&bytes), settings).unwrap();
    // SAFETY: `module` stays mapped for as long as `memory` is used.
    let mut memory = unsafe { Mapped::new(module.base(), module.size()) };
    apply_all(&mut memory, &built.patches).unwrap();
    (module, built)
}

/// The tests share the input shim's one attached cell.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn without_mouse_motion_the_deltas_are_the_sticks_and_registers_survive() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (module, _built) = patched(&MouseLookSettings::default());
    let (h, p, marker) = run(&module, 0.02, -0.03);
    assert!(
        (h - 0.02).abs() < 1e-7 && (p + 0.03).abs() < 1e-7,
        "{h} {p}"
    );
    assert_eq!(marker, 0x5A5A);
}

#[test]
fn counts_are_added_once_scaled_and_signed_by_the_settings() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let settings = MouseLookSettings {
        sensitivity: [2.0, 0.5],
        invert: [false, true],
    };
    let (module, built) = patched(&settings);
    let cell = cell_of(&module, &built);
    // SAFETY: the cell is in the synthetic image's writable data tail and stays mapped.
    unsafe { direct::attach(cell) };
    assert!(direct::push(100, -40));
    let [sx, sy] = settings.scales();
    assert!(sx < 0.0 && sy > 0.0);
    let (h, p, _) = run(&module, 0.02, -0.03);
    assert!((h - (0.02 + 100.0 * sx)).abs() < 1e-6, "{h}");
    assert!((p - (-0.03 + -40.0 * sy)).abs() < 1e-6, "{p}");
    // SAFETY: as above; the routine exchanged the counts with zero.
    let counts = unsafe { [cell.read(), cell.add(1).read()] };
    assert_eq!(counts, [0, 0]);
    let (h2, p2, _) = run(&module, 0.02, -0.03);
    assert!((h2 - 0.02).abs() < 1e-7 && (p2 + 0.03).abs() < 1e-7);
    direct::detach();
}

#[test]
fn the_cell_starts_with_the_scales_and_the_shim_updates_them() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let settings = MouseLookSettings::default();
    let (module, built) = patched(&settings);
    let cell = cell_of(&module, &built);
    // SAFETY: as above.
    unsafe {
        let scales = settings.scales();
        assert_eq!(f32::from_bits(cell.add(2).read() as u32), scales[0]);
        direct::attach(cell);
    }
    direct::set_scales(0.001, 0.002);
    direct::push(10, 10);
    let (h, p, _) = run(&module, 0.0, 0.0);
    assert!(
        (h - 0.01).abs() < 1e-6 && (p - 0.02).abs() < 1e-6,
        "{h} {p}"
    );
    direct::detach();
}

#[test]
fn reverting_puts_every_original_byte_back() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = image(false);
    let module = load_image(&bytes);
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let built =
        camera_mouse::patches(&finder, &layout(&bytes), &MouseLookSettings::default()).unwrap();
    // SAFETY: `module` stays mapped for as long as `memory` is used.
    let mut memory = unsafe { Mapped::new(module.base(), module.size()) };
    let before = |at: u32, len: usize| {
        // SAFETY: the range is inside the mapped synthetic image.
        unsafe { std::slice::from_raw_parts((module.base() + at as usize) as *const u8, len) }
            .to_vec()
    };
    let hook = before(HOOK, camera_mouse::HOOK_LEN);
    let applied = apply_all(&mut memory, &built.patches).unwrap();
    assert_ne!(before(HOOK, camera_mouse::HOOK_LEN), hook);
    revert_all(&mut memory, applied);
    assert_eq!(before(HOOK, camera_mouse::HOOK_LEN), hook);
    assert!(before(built.cave, 64).iter().all(|b| *b == 0));
}

#[test]
fn a_different_code_shape_or_hash_is_refused() {
    let bytes = image(true);
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let error =
        camera_mouse::patches(&finder, &layout(&bytes), &MouseLookSettings::default()).unwrap_err();
    assert!(matches!(error, PatchError::NotFound { .. }), "{error:?}");

    let bytes = image(false);
    let parsed = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&parsed);
    let wrong = Layout {
        update: UPDATE,
        hash: 1,
    };
    let error = camera_mouse::patches(&finder, &wrong, &MouseLookSettings::default()).unwrap_err();
    assert!(matches!(error, PatchError::Mismatch { .. }), "{error:?}");
}
