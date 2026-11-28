//! The GDK runtime classes the executable reaches through its `xgameruntime` wrappers, called through their
//! vtables the way those wrappers call them: every called method exists at the slot the wrapper uses, and the
//! offline answers of the classes `store_package.rs` and `loader.rs` do not cover.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, Once};

const CLASSES: &str = include_str!("../src/loader/classes.txt");
const CALLS: &str = include_str!("xgame_calls.txt");

const INVALID_ARG: i32 = 0x8007_0057_u32 as i32;
const POINTER: i32 = 0x8000_4003_u32 as i32;
const ABORT: i32 = 0x8000_4004_u32 as i32;
const NOT_SUPPORTED: i32 = 0x8007_0032_u32 as i32;
const TOO_SMALL: i32 = 0x8007_007A_u32 as i32;

fn guid(text: &str) -> [u8; 16] {
    let hex: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b'-')
        .map(|b| (b as char).to_digit(16).unwrap() as u8)
        .collect();
    let byte = |i: usize| hex[2 * i] << 4 | hex[2 * i + 1];
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut out = [0u8; 16];
    for (to, from) in order.into_iter().enumerate() {
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
        .unwrap_or_else(|| panic!("class {name}"))
}

struct Interface {
    id: [u8; 16],
    parent: Option<String>,
    methods: Vec<String>,
}

fn interfaces() -> HashMap<String, Interface> {
    CLASSES
        .lines()
        .filter_map(|l| l.strip_prefix("iface "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (
                w[0].to_string(),
                Interface {
                    id: guid(w[2]),
                    parent: (w[3] != "-").then(|| w[3].to_string()),
                    methods: w[4..].iter().map(|m| m.to_string()).collect(),
                },
            )
        })
        .collect()
}

/// The vtable slots of an interface, IUnknown's three first, then each ancestor's methods, base first.
fn slots_of(interface: &str) -> Vec<String> {
    let all = interfaces();
    let mut chain = vec![interface.to_string()];
    while let Some(parent) = all[chain.last().unwrap()].parent.clone() {
        chain.push(parent);
    }
    let mut slots: Vec<String> = vec!["QueryInterface".into(), "AddRef".into(), "Release".into()];
    for name in chain.iter().rev() {
        slots.extend(all[name].methods.iter().cloned());
    }
    slots
}

fn configure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("xgame-services-test");
        let game = root.join("game");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            game.join("MicrosoftGame.Config"),
            "<Game configVersion=\"1\"><TitleId>7952987A</TitleId>\
             <Identity Name=\"Test.Package\" Version=\"1.0.0.0\" /></Game>",
        )
        .unwrap();
        shims::loader::configure(Some(game), root.join("Output"));
    });
}

static QUERIES: Mutex<()> = Mutex::new(());

fn object_as(class: &str, interface: &str) -> usize {
    configure();
    let _queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
    let id = if interface.is_empty() {
        class_id(class)
    } else {
        interfaces()[interface].id
    };
    let mut out = 0usize;
    let hr = shims::loader::query_api(&class_id(class), &id, &mut out);
    assert_eq!(hr, 0, "{class} as {interface}");
    out
}

fn object(class: &str) -> usize {
    object_as(class, "")
}

unsafe fn slot<T: Copy>(object: usize, index: usize) -> T {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: a COM object's first word is its vtable.
    unsafe {
        let vtable = *(object as *const *const usize);
        std::mem::transmute_copy(&*vtable.add(index))
    }
}

fn cstr(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

#[repr(C)]
#[derive(Default)]
struct Block {
    queue: usize,
    context: usize,
    callback: usize,
    internal: [u8; 32],
}

fn wait(block: &mut Block) -> i32 {
    // SAFETY: XAsyncGetStatus(block, wait).
    unsafe {
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
            slot(object("XThreadingImpl"), 3);
        status(0, block, 1)
    }
}

fn calls() -> Vec<[&'static str; 5]> {
    CALLS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            [w[0], w[1], w[2], w[3], w[4]]
        })
        .collect()
}

#[test]
fn every_method_the_executable_calls_is_at_its_slot_and_implemented() {
    let calls = calls();
    assert!(calls.len() >= 160, "{} measured calls", calls.len());
    let mut missing = Vec::new();
    for [class, interface, method, measured, rva] in &calls {
        let slots = slots_of(interface);
        let measured: usize = measured.parse().unwrap();
        assert_eq!(
            slots.get(measured).map(String::as_str),
            Some(*method),
            "{interface}::{method} is not at slot {measured} (wrapper {rva})"
        );
        object_as(class, interface);
        if shims::loader::unimplemented_methods(class)
            .iter()
            .any(|m| m == method)
        {
            missing.push(format!("{class}::{method} (wrapper {rva})"));
        }
    }
    assert!(
        missing.is_empty(),
        "called but not implemented: {missing:?}"
    );
}

#[test]
fn every_class_the_executable_calls_is_served() {
    let wanted: std::collections::BTreeSet<&str> = calls().iter().map(|c| c[0]).collect();
    for class in [
        "XAccessibilityImpl",
        "XAppCaptureImpl",
        "XDisplayImpl",
        "XGameEventImpl",
        "XGameImpl",
        "XGameRuntimeFeatureImpl",
        "XGameSaveImpl",
        "XGameUiImpl",
        "XLauncherImpl",
        "XNetworkingImpl",
        "XPackageImpl",
        "XPersistentLocalStorageImpl",
        "XStoreImpl",
        "XSystemAnalyticsImpl",
        "XSystemImpl",
        "XThreadingImpl",
        "XUserImpl",
    ] {
        assert!(wanted.contains(class), "{class} is called");
        object(class);
    }
}

#[test]
fn the_runtime_reports_every_feature_the_executable_asks_about_as_a_console_does() {
    let feature = object("XGameRuntimeFeatureImpl");
    // SAFETY: XGameRuntimeIsFeatureAvailable(feature).
    unsafe {
        let available: unsafe extern "win64" fn(usize, u32) -> u8 = slot(feature, 3);
        assert_eq!(
            available(feature, 10),
            1,
            "XNetworking, which RVA 0x18e7220 asks for"
        );
        assert_eq!(
            available(feature, 20),
            1,
            "XGameEvent, which the events service asks for"
        );
        assert_eq!(available(feature, 1), 1, "XAppCapture");
        for id in 0..=21 {
            assert_eq!(available(feature, id), 1, "feature {id}");
        }
        assert_eq!(available(feature, 22), 0);
    }
}

#[test]
fn a_screenshot_is_refused_because_nothing_can_capture_one() {
    let capture = object_as("XAppCaptureImpl", "IXAppCaptureImpl4");
    // SAFETY: XAppCaptureTakeScreenshot(user, result).
    unsafe {
        let take: unsafe extern "win64" fn(usize, usize, *mut [u8; 0x11c]) -> i32 =
            slot(capture, 5);
        let mut result = [0xAAu8; 0x11c];
        assert_eq!(take(capture, 1, &mut result), NOT_SUPPORTED);
        assert!(
            result.iter().all(|b| *b == 0xAA),
            "a failure writes no result"
        );
        let enable: unsafe extern "win64" fn(usize) -> i32 = slot(capture, 10);
        assert_eq!(enable(capture), 0);
    }
}

#[test]
fn the_display_timeout_is_deferred_with_a_handle_that_closes_once() {
    let launcher = object("XLauncherImpl");
    // SAFETY: the documented shapes.
    unsafe {
        let acquire: unsafe extern "win64" fn(usize, *mut usize) -> i32 = slot(launcher, 4);
        let close: unsafe extern "win64" fn(usize, usize) = slot(launcher, 5);
        assert_eq!(acquire(launcher, std::ptr::null_mut()), POINTER);
        let (mut first, mut second) = (0usize, 0usize);
        assert_eq!(acquire(launcher, &mut first), 0);
        assert_eq!(acquire(launcher, &mut second), 0);
        assert_ne!(first, 0);
        assert_ne!(first, second);
        close(launcher, first);
        close(launcher, first);
        close(launcher, 0x1234);
        close(launcher, second);
    }
}

fn user_handle() -> usize {
    let user = object("XUserImpl");
    // SAFETY: XUserAddAsync(options, block) and XUserAddResult(block, &user).
    unsafe {
        let add: unsafe extern "win64" fn(usize, u32, *mut Block) -> i32 = slot(user, 7);
        let result: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 = slot(user, 8);
        let mut block = Block::default();
        assert_eq!(add(user, 1, &mut block), 0);
        assert_eq!(wait(&mut block), 0);
        let mut handle = 0usize;
        assert_eq!(result(user, &mut block, &mut handle), 0);
        handle
    }
}

#[test]
fn the_profile_card_is_dismissed_at_once() {
    let ui = object("XGameUiImpl");
    let user = user_handle();
    // SAFETY: XGameUiShowPlayerProfileCardAsync(block, user, xuid) and its result.
    unsafe {
        let card: unsafe extern "win64" fn(usize, *mut Block, usize, u64) -> i32 = slot(ui, 7);
        let result: unsafe extern "win64" fn(usize, *mut Block) -> i32 = slot(ui, 8);
        let mut block = Block::default();
        assert_eq!(card(ui, std::ptr::null_mut(), user, 5), POINTER);
        assert_eq!(
            card(ui, &mut block, 0xdead, 5),
            INVALID_ARG,
            "an unknown user"
        );
        assert_eq!(card(ui, &mut block, user, 0x0009_0000_0000_0007), 0);
        assert_eq!(wait(&mut block), 0);
        assert_eq!(result(ui, &mut block), 0);
    }
}

struct TextEntry {
    ui: usize,
}

impl TextEntry {
    /// Shows the dialog and returns its status and text, as the executable reads them (RVA 0x43a2a80).
    fn ask(&self, default: Option<&str>, max: u32) -> (i32, Option<(u32, Vec<u8>)>) {
        // SAFETY: the documented shapes of the three text entry methods.
        unsafe {
            let begin: unsafe extern "win64" fn(
                usize,
                *mut Block,
                *const u8,
                *const u8,
                *const u8,
                u32,
                u32,
            ) -> i32 = slot(self.ui, 17);
            let size: unsafe extern "win64" fn(usize, *mut Block, *mut u32) -> i32 =
                slot(self.ui, 18);
            let text: unsafe extern "win64" fn(usize, *mut Block, u32, *mut u8, *mut u32) -> i32 =
                slot(self.ui, 19);
            let (title, description) = (cstr("Name"), cstr("Enter a name"));
            let default = default.map(cstr);
            let mut block = Block::default();
            let started = begin(
                self.ui,
                &mut block,
                title.as_ptr(),
                description.as_ptr(),
                default.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
                0,
                max,
            );
            if started != 0 {
                return (started, None);
            }
            let status = wait(&mut block);
            if status != 0 {
                let mut bytes = 99u32;
                assert_eq!(size(self.ui, &mut block, &mut bytes), status);
                return (status, None);
            }
            let mut bytes = 0u32;
            assert_eq!(size(self.ui, &mut block, &mut bytes), 0);
            let mut buffer = vec![0x55u8; bytes as usize];
            let mut used = 0u32;
            let hr = text(self.ui, &mut block, bytes, buffer.as_mut_ptr(), &mut used);
            assert_eq!(hr, 0);
            buffer.truncate(used as usize);
            (0, Some((bytes, buffer)))
        }
    }
}

#[test]
fn a_text_entry_is_accepted_with_its_default_or_cancelled_when_there_is_none() {
    let entry = TextEntry {
        ui: object("XGameUiImpl"),
    };
    let (status, answer) = entry.ask(Some("Franklin"), 1024);
    assert_eq!(status, 0);
    assert_eq!(answer, Some((9, b"Franklin\0".to_vec())));

    let (_, cut) = entry.ask(Some("Franklin"), 3);
    assert_eq!(
        cut,
        Some((4, b"Fra\0".to_vec())),
        "cut to the character limit"
    );

    let (_, accents) = entry.ask(Some("Jos\u{e9}"), 4);
    assert_eq!(
        accents,
        Some((6, "Jos\u{e9}\0".as_bytes().to_vec())),
        "characters, not bytes"
    );

    assert_eq!(
        entry.ask(None, 100),
        (ABORT, None),
        "no default is a cancel"
    );
    assert_eq!(entry.ask(Some(""), 100), (ABORT, None));
    assert_eq!(entry.ask(Some("x"), 0), (INVALID_ARG, None));

    // SAFETY: a result read with a buffer that is too small.
    unsafe {
        let begin: unsafe extern "win64" fn(
            usize,
            *mut Block,
            *const u8,
            *const u8,
            *const u8,
            u32,
            u32,
        ) -> i32 = slot(entry.ui, 17);
        let text: unsafe extern "win64" fn(usize, *mut Block, u32, *mut u8, *mut u32) -> i32 =
            slot(entry.ui, 19);
        let (title, default) = (cstr("t"), cstr("abcdef"));
        let mut block = Block::default();
        assert_eq!(
            begin(
                entry.ui,
                &mut block,
                title.as_ptr(),
                title.as_ptr(),
                default.as_ptr(),
                0,
                10
            ),
            0
        );
        assert_eq!(wait(&mut block), 0);
        let mut small = [0u8; 3];
        let mut used = 9u32;
        assert_eq!(
            text(entry.ui, &mut block, 3, small.as_mut_ptr(), &mut used),
            TOO_SMALL
        );
        assert_eq!(used, 0);
        let mut full = [0u8; 7];
        assert_eq!(
            text(entry.ui, &mut block, 7, full.as_mut_ptr(), &mut used),
            0
        );
        assert_eq!(&full, b"abcdef\0");
    }
}

#[test]
fn the_user_holds_no_network_privilege_and_keeps_the_local_ones() {
    let user = object("XUserImpl");
    let handle = user_handle();
    // SAFETY: XUserCheckPrivilege(user, options, privilege, &has, &reason).
    unsafe {
        let check: unsafe extern "win64" fn(usize, usize, u32, u32, *mut u8, *mut u32) -> i32 =
            slot(user, 20);
        // The fifteen privileges the executable caches at sign-in (table at RVA 0x654fc34).
        let asked = [
            185u32, 188, 189, 190, 196, 198, 203, 207, 208, 209, 220, 247, 252, 254, 255,
        ];
        let mut denied = Vec::new();
        for privilege in asked {
            let (mut has, mut reason) = (9u8, 9u32);
            assert_eq!(check(user, handle, 0, privilege, &mut has, &mut reason), 0);
            if has == 0 {
                assert_eq!(reason, 4, "{privilege} is denied as unknown");
                denied.push(privilege);
            } else {
                assert_eq!((has, reason), (1, 0));
            }
        }
        assert_eq!(
            denied,
            [185, 188, 189, 190, 203, 207, 208, 220, 247, 252, 254, 255]
        );
        for local in [196u32, 198, 209] {
            let (mut has, mut reason) = (0u8, 0u32);
            assert_eq!(check(user, handle, 0, local, &mut has, &mut reason), 0);
            assert_eq!(has, 1, "{local}");
        }
        let (mut has, mut reason) = (0u8, 0u32);
        assert_eq!(
            check(user, 0xdead, 0, 254, &mut has, &mut reason),
            INVALID_ARG
        );
        assert_eq!(
            check(user, handle, 0, 254, std::ptr::null_mut(), &mut reason),
            POINTER
        );
    }
}
