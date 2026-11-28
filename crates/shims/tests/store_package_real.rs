//! The package shim against the real game folder, when `LEONIDA_GAME_DIR` names one: the chunks of its layout
//! file are the ones its files make installed. Skipped without the folder. Reads only file names and sizes.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;

const CLASSES: &str = include_str!("../src/loader/classes.txt");

fn guid(text: &str) -> [u8; 16] {
    let hex: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b'-')
        .map(|b| (b as char).to_digit(16).unwrap() as u8)
        .collect();
    let byte = |i: usize| hex[2 * i] << 4 | hex[2 * i + 1];
    let mut out = [0u8; 16];
    for (to, from) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15]
        .into_iter()
        .enumerate()
    {
        out[to] = byte(from);
    }
    out
}

fn class_id(name: &str) -> [u8; 16] {
    CLASSES
        .lines()
        .filter_map(|l| l.strip_prefix("class "))
        .find_map(|l| {
            let (n, id) = l.split_once(' ')?;
            (n == name).then(|| guid(id))
        })
        .unwrap()
}

fn slot_of(method: &str) -> usize {
    3 + CLASSES
        .lines()
        .filter_map(|l| l.strip_prefix("iface "))
        .filter(|l| l.split_whitespace().nth(1) == Some("XPackageImpl"))
        .flat_map(|l| l.split_whitespace().skip(4))
        .position(|m| m == method)
        .unwrap()
}

static SEEN: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

#[repr(C)]
struct Selector {
    kind: u32,
    padding: u32,
    value: usize,
}

unsafe extern "win64" fn collect(_: *mut c_void, selector: *const Selector, state: u32) -> u8 {
    // SAFETY: the record of the call.
    let value = unsafe { (*selector).value };
    SEEN.lock().unwrap().push((value, state));
    1
}

#[test]
fn the_chunks_of_the_real_layout_are_installed_when_their_files_are() {
    let Some(game) = std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from) else {
        return;
    };
    if !game.join("MicrosoftGame.Config").is_file() {
        return;
    }
    shims::loader::configure(Some(game.clone()), game.join("../Output-test"));
    let mut package = 0usize;
    let id = class_id("XPackageImpl");
    assert_eq!(shims::loader::query_api(&id, &id, &mut package), 0);
    let mut name = [0u8; 40];
    // SAFETY: the documented shapes of the package interface.
    unsafe {
        let vtable = *(package as *const *const usize);
        let identifier: unsafe extern "win64" fn(usize, usize, *mut u8) -> i32 =
            std::mem::transmute_copy(&*vtable.add(3));
        assert_eq!(identifier(package, 33, name.as_mut_ptr()), 0);
        let enumerate: unsafe extern "win64" fn(usize, *const u8, u32, *mut c_void, usize) -> i32 =
            std::mem::transmute_copy(&*vtable.add(slot_of("XPackageEnumerateChunkAvailability")));
        assert_eq!(
            enumerate(
                package,
                name.as_ptr(),
                2,
                std::ptr::null_mut(),
                collect as *const () as usize
            ),
            0
        );
        let find: unsafe extern "win64" fn(
            usize,
            *const u8,
            u32,
            *const Selector,
            *mut u32,
        ) -> i32 = std::mem::transmute_copy(&*vtable.add(slot_of("XPackageFindChunkAvailability")));
        let seen = SEEN.lock().unwrap().clone();
        assert!(!seen.is_empty(), "the folder has a layout file");
        for (chunk, state) in seen {
            let selector = Selector {
                kind: 2,
                padding: 0,
                value: chunk,
            };
            let mut found = 99u32;
            assert_eq!(find(package, name.as_ptr(), 1, &selector, &mut found), 0);
            assert_eq!(found, state, "chunk {chunk}: enumerating and finding agree");
            assert_eq!(state, 0, "chunk {chunk} of the game folder is installed");
        }
    }
}
