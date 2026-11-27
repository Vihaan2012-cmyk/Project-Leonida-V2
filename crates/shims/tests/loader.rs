//! Library loading and the GDK runtime, called the way the program calls them.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicU32, Ordering};

const CLASSES: &str = include_str!("../src/loader/classes.txt");

fn shim_address(dll: &str, name: &str) -> usize {
    shims::builtin()
        .into_iter()
        .find(|s| s.dll == dll && s.name == name)
        .unwrap_or_else(|| panic!("{dll}!{name} registered"))
        .address
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn cstr(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

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

fn interface_id(name: &str) -> [u8; 16] {
    CLASSES
        .lines()
        .filter_map(|l| l.strip_prefix("iface "))
        .find_map(|l| {
            let mut words = l.split_whitespace();
            (words.next()? == name).then(|| guid(words.nth(1).unwrap()))
        })
        .unwrap_or_else(|| panic!("interface {name}"))
}

const NOT_PROVIDED: i32 = 0x8007_0032_u32 as i32;

fn configure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("loader-test");
        let game = root.join("game");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            game.join("MicrosoftGame.Config"),
            "<Game configVersion=\"1\"><TitleId>7952987A</TitleId>\
             <Identity Name=\"Test.Package\" Version=\"1.0.0.0\" />\
             <PersistentLocalStorage><SizeMB>2048</SizeMB></PersistentLocalStorage></Game>",
        )
        .unwrap();
        shims::loader::configure(Some(game), root.join("Output"));
    });
}

/// The object a class serves for its own id.
fn object(class: &str) -> usize {
    configure();
    let mut out = 0usize;
    let hr = shims::loader::query_api(&class_id(class), &class_id(class), &mut out);
    assert_eq!(hr, 0, "{class}");
    assert_ne!(out, 0);
    out
}

fn object_as(class: &str, interface: &str) -> usize {
    configure();
    let mut out = 0usize;
    let hr = shims::loader::query_api(&class_id(class), &interface_id(interface), &mut out);
    assert_eq!(hr, 0, "{class} as {interface}");
    out
}

/// The function in `slot` of a COM object's vtable.
unsafe fn slot<T: Copy>(object: usize, index: usize) -> T {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: a COM object's first word is its vtable.
    unsafe {
        let vtable = *(object as *const *const usize);
        std::mem::transmute_copy(&*vtable.add(index))
    }
}

#[repr(C)]
#[derive(Default)]
struct Block {
    queue: usize,
    context: usize,
    callback: usize,
    internal: [u8; 32],
}

// ---- library loading ----

type LoadLibraryW = unsafe extern "win64" fn(*const u16) -> usize;
type LoadLibraryExA = unsafe extern "win64" fn(*const u8, usize, u32) -> usize;
type GetModuleHandleW = unsafe extern "win64" fn(*const u16) -> usize;
type GetModuleHandleExW = unsafe extern "win64" fn(u32, *const u16, *mut usize) -> i32;
type GetProcAddress = unsafe extern "win64" fn(usize, *const u8) -> usize;
type FreeLibrary = unsafe extern "win64" fn(usize) -> i32;
type DisableThreadLibraryCalls = unsafe extern "win64" fn(usize) -> i32;

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last error.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

fn fn_at<T: Copy>(dll: &str, name: &str) -> T {
    let address = shim_address(dll, name);
    // SAFETY: `T` is the function pointer type of the shim's documented shape.
    unsafe { std::mem::transmute_copy(&address) }
}

#[test]
fn a_console_dll_loads_and_resolves_through_the_shim_table() {
    let load: LoadLibraryW = fn_at("kernel32.dll", "LoadLibraryW");
    let proc: GetProcAddress = fn_at("kernel32.dll", "GetProcAddress");
    let free: FreeLibrary = fn_at("kernel32.dll", "FreeLibrary");
    // SAFETY: the shims' documented shapes with valid strings.
    unsafe {
        let module = load(wide("XMem.DLL").as_ptr());
        assert_ne!(module, 0);
        assert_eq!(
            module,
            load(wide("C:\\games\\xmem.dll").as_ptr()),
            "one handle per library"
        );
        assert_eq!(
            proc(module, cstr("XMemAlloc").as_ptr()),
            shim_address("xmem.dll", "XMemAlloc")
        );
        assert_eq!(proc(module, cstr("NoSuchFunction").as_ptr()), 0);
        assert_eq!(last_error(), 127);
        assert_eq!(proc(module, 7usize as *const u8), 0, "no ordinals");
        assert_eq!(free(module), 1);
        assert_eq!(free(module), 1);
    }
}

#[test]
fn module_handles_answer_for_console_dlls_and_the_other_load_variants() {
    let handle: GetModuleHandleW = fn_at("kernelbase.dll", "GetModuleHandleW");
    let handle_ex: GetModuleHandleExW = fn_at(
        "api-ms-win-core-libraryloader-l1-2-0.dll",
        "GetModuleHandleExW",
    );
    let load_ex: LoadLibraryExA = fn_at("kernel32.dll", "LoadLibraryExA");
    let disable: DisableThreadLibraryCalls = fn_at("kernel32.dll", "DisableThreadLibraryCalls");
    // SAFETY: the shims' documented shapes with valid strings.
    unsafe {
        let loaded = load_ex(cstr("d3d12_xs.dll").as_ptr(), 0, 0x800);
        assert_ne!(loaded, 0);
        assert_eq!(handle(wide("D3D12_XS.dll").as_ptr()), loaded);
        let mut again = 0;
        assert_eq!(handle_ex(1, wide("d3d12_xs").as_ptr(), &mut again), 1);
        assert_eq!(again, loaded);
        assert_eq!(disable(loaded), 1);
        assert_eq!(
            handle(wide("bink2scarlett.dll").as_ptr()),
            0,
            "package DLLs are not loaded"
        );
        assert_eq!(last_error(), 126);
        assert_ne!(handle(std::ptr::null()), 0, "this process");
    }
}

#[test]
fn what_the_offline_policy_blocks_is_missing() {
    let load: LoadLibraryW = fn_at("kernel32.dll", "LoadLibraryW");
    let proc: GetProcAddress = fn_at("kernel32.dll", "GetProcAddress");
    // SAFETY: the shims' documented shapes with valid strings.
    unsafe {
        for name in [
            "winhttp.dll",
            "WS2_32",
            "C:\\Windows\\System32\\wininet.dll",
            "XCurl.dll",
            "libHttpClient.GDK.dll",
        ] {
            assert_eq!(load(wide(name).as_ptr()), 0, "{name}");
            assert_eq!(last_error(), 126, "{name}");
        }
        let runtime = load(wide("xgameruntime.dll").as_ptr());
        assert_ne!(runtime, 0);
        assert_eq!(
            proc(runtime, cstr("XNetworkingQueryStatistics").as_ptr()),
            0
        );
        assert_eq!(
            proc(runtime, cstr("XUserGetTokenAndSignatureAsync").as_ptr()),
            0
        );
    }
}

#[test]
fn allowed_windows_dlls_load_and_their_functions_resolve_to_shims_where_one_exists() {
    let load: LoadLibraryW = fn_at("kernel32.dll", "LoadLibraryW");
    let proc: GetProcAddress = fn_at("kernel32.dll", "GetProcAddress");
    // SAFETY: the shims' documented shapes with valid strings.
    unsafe {
        let kernel = load(wide("kernel32.dll").as_ptr());
        assert_ne!(kernel, 0);
        assert_eq!(
            proc(kernel, cstr("LoadLibraryW").as_ptr()),
            shim_address("kernel32.dll", "LoadLibraryW"),
            "the program cannot step around the shims"
        );
        assert_ne!(proc(kernel, cstr("GetTickCount").as_ptr()), 0);
        assert_eq!(
            load(wide("dbghelp.dll").as_ptr()),
            0,
            "not on the allow-list"
        );
    }
}

#[test]
fn the_runtime_is_found_through_load_library_and_get_proc_address() {
    configure();
    let load: LoadLibraryW = fn_at("kernel32.dll", "LoadLibraryW");
    let proc: GetProcAddress = fn_at("kernel32.dll", "GetProcAddress");
    // SAFETY: the shims' documented shapes with valid strings.
    unsafe {
        let module = load(wide("xgameruntime.dll").as_ptr());
        for name in [
            "InitializeApiImpl",
            "InitializeApiImplEx",
            "InitializeApiImplEx2",
            "QueryApiImpl",
            "UninitializeApiImpl",
            "XErrorReport",
        ] {
            assert_ne!(proc(module, cstr(name).as_ptr()), 0, "{name}");
        }
        let initialize: unsafe extern "win64" fn(u32, u32, i8, *const c_void) -> i32 =
            std::mem::transmute(proc(module, cstr("InitializeApiImplEx2").as_ptr()));
        assert_eq!(initialize(2, 1, 0, std::ptr::null()), 0);
        let query: unsafe extern "win64" fn(*const [u8; 16], *const [u8; 16], *mut usize) -> i32 =
            std::mem::transmute(proc(module, cstr("QueryApiImpl").as_ptr()));
        let class = class_id("XErrorImpl");
        let mut out = 0usize;
        assert_eq!(query(&class, &class, &mut out), 0);
        let report: unsafe extern "win64" fn(usize, i32, *const u8) -> i32 = slot(out, 3);
        assert_eq!(
            report(out, 0x8000_4005_u32 as i32, cstr("boot check").as_ptr()),
            0
        );
        let export: unsafe extern "win64" fn(i32, *const u8) -> i32 =
            std::mem::transmute(proc(module, cstr("XErrorReport").as_ptr()));
        assert_eq!(export(0x8000_4005_u32 as i32, cstr("export").as_ptr()), 0);
        let uninitialize: unsafe extern "win64" fn() -> i32 =
            std::mem::transmute(proc(module, cstr("UninitializeApiImpl").as_ptr()));
        assert_eq!(uninitialize(), 0);

        let services = load(wide("gameplatformservices.dll").as_ptr());
        let version: unsafe extern "win64" fn(*mut u64, *mut u64) -> i32 =
            std::mem::transmute(proc(services, cstr("GetApiVersionImpl").as_ptr()));
        let (mut a, mut b) = (0u64, 0u64);
        assert_eq!(version(&mut a, &mut b), 0);
        assert_eq!((a, b), (1, 1));
        assert_ne!(proc(services, cstr("QueryApiImpl").as_ptr()), 0);
    }
}

// ---- QueryApiImpl ----

#[test]
fn every_served_class_answers_query_api_impl() {
    configure();
    for class in [
        "XErrorImpl",
        "XThreadingImpl",
        "XSystemImpl",
        "XSystemAnalyticsImpl",
        "XPackageImpl",
        "XGameImpl",
        "XGameRuntimeFeatureImpl",
        "XPersistentLocalStorageImpl",
        "XGameSaveImpl",
        "XUserImpl",
        "XStoreImpl",
        "XLauncherImpl",
        "XDisplayImpl",
        "XAccessibilityImpl",
        "XUserDeviceImpl",
        "XGameEventImpl",
        "XGameUiImpl",
        "XGameStreamingImpl",
        "XNetworkingImpl",
    ] {
        let first = object(class);
        let second = object(class);
        assert_eq!(first, second, "{class} is one object");
    }
    for class in ["XAppCaptureImpl", "XGameInviteImpl", "XGameProtocolImpl"] {
        let mut out = 1usize;
        let hr = shims::loader::query_api(&class_id(class), &class_id(class), &mut out);
        assert_eq!(hr, NOT_PROVIDED, "{class}");
        assert_eq!(out, 0);
    }
    let mut out = 1usize;
    assert_eq!(
        shims::loader::query_api(&[7; 16], &[7; 16], &mut out),
        NOT_PROVIDED
    );
}

#[test]
fn derived_interfaces_answer_with_the_same_object_and_a_second_family_is_its_own() {
    let package = object("XPackageImpl");
    assert_eq!(object_as("XPackageImpl", "IXPackageImpl4"), package);
    assert_eq!(object_as("XPackageImpl", "IXPackageImpl2"), package);
    let user = object("XUserImpl");
    assert_eq!(object_as("XUserImpl", "IXUserImpl6"), user);
    let gamertag = object_as("XUserImpl", "IXUserGamertagImpl");
    assert_ne!(gamertag, user);
    let mut out = 1usize;
    let hr = shims::loader::query_api(&class_id("XUserImpl"), &[9; 16], &mut out);
    assert_eq!(
        hr, 0x8000_4002_u32 as i32,
        "an interface the class does not have"
    );
    assert_eq!(out, 0);
    // SAFETY: IUnknown's slots of a runtime object.
    unsafe {
        let add_ref: unsafe extern "win64" fn(usize) -> u32 = slot(user, 1);
        let release: unsafe extern "win64" fn(usize) -> u32 = slot(user, 2);
        let before = add_ref(user);
        assert_eq!(release(user), before - 1);
        let query: unsafe extern "win64" fn(usize, *const [u8; 16], *mut usize) -> i32 =
            slot(user, 0);
        let mut again = 0usize;
        assert_eq!(query(user, &interface_id("IXUserImpl3"), &mut again), 0);
        assert_eq!(again, user);
        release(user);
    }
}

#[test]
fn an_unimplemented_slot_notes_the_call_and_fails() {
    let package = object("XPackageImpl");
    // SAFETY: slot 12 is XPackageFindChunkAvailability, which nothing implements.
    let hr = unsafe {
        let call: unsafe extern "win64" fn(usize) -> i32 = slot(package, 12);
        call(package)
    };
    assert_eq!(hr, 0x8000_4001_u32 as i32);
    assert!(
        shims::trace::notes()
            .iter()
            .any(|n| n.contains("XPackageImpl slot 12 (XPackageFindChunkAvailability)"))
    );
}

