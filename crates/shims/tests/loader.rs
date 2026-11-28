//! Library loading and the GDK runtime, called the way the program calls them.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, Once};

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
/// Held while a test reads a runtime object's reference count, and by every query, which adds a reference.
static REFERENCES: Mutex<()> = Mutex::new(());

fn object(class: &str) -> usize {
    configure();
    let _queries = REFERENCES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = 0usize;
    let hr = shims::loader::query_api(&class_id(class), &class_id(class), &mut out);
    assert_eq!(hr, 0, "{class}");
    assert_ne!(out, 0);
    out
}

fn object_as(class: &str, interface: &str) -> usize {
    configure();
    let _queries = REFERENCES.lock().unwrap_or_else(|e| e.into_inner());
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
        assert_ne!(
            handle(wide("bink2scarlett.dll").as_ptr()),
            0,
            "the media shim makes Bink a console DLL"
        );
        assert_eq!(
            handle(wide("some_package_library.dll").as_ptr()),
            0,
            "DLLs with no shim are not loaded"
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
        let counting = REFERENCES.lock().unwrap_or_else(|e| e.into_inner());
        let before = add_ref(user);
        assert_eq!(release(user), before - 1);
        drop(counting);
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

// ---- tasks and async ----

static CALLBACKS: AtomicU32 = AtomicU32::new(0);

unsafe extern "win64" fn completed(_block: *mut Block) {
    CALLBACKS.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "win64" fn work(_block: *mut Block) -> i32 {
    0
}

/// A provider that finishes with a 4-byte result of 42.
unsafe extern "win64" fn provider(op: u32, data: *const [usize; 4]) -> i32 {
    // SAFETY: the provider data of a live call: block, buffer size, buffer, context.
    let data = unsafe { &*data };
    match op {
        0 => {
            let schedule: unsafe extern "win64" fn(usize, *mut Block, u32) -> i32 =
                unsafe { slot(THREADING.load(Ordering::SeqCst), 9) };
            // SAFETY: the call's block.
            unsafe { schedule(0, data[0] as *mut Block, 0) }
        }
        1 => {
            let complete: unsafe extern "win64" fn(usize, *mut Block, i32, usize) =
                unsafe { slot(THREADING.load(Ordering::SeqCst), 10) };
            // SAFETY: the call's block.
            unsafe { complete(0, data[0] as *mut Block, 0, 4) };
            0
        }
        2 => {
            // SAFETY: the caller's buffer holds the 4 bytes asked for.
            unsafe { *(data[2] as *mut u32) = 42 };
            0
        }
        _ => 0,
    }
}

static THREADING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[test]
fn async_calls_and_task_queues_run_end_to_end() {
    let threading = object("XThreadingImpl");
    THREADING.store(threading, Ordering::SeqCst);
    // SAFETY: the documented shapes of XAsync and XTaskQueue, with valid pointers.
    unsafe {
        let create: unsafe extern "win64" fn(usize, u32, u32, *mut usize) -> i32 =
            slot(threading, 12);
        let dispatch: unsafe extern "win64" fn(usize, usize, u32, u32) -> u8 = slot(threading, 16);
        let close: unsafe extern "win64" fn(usize, usize) = slot(threading, 17);
        let submit: unsafe extern "win64" fn(usize, usize, u32, usize, usize) -> i32 =
            slot(threading, 18);
        let terminate: unsafe extern "win64" fn(usize, usize, u8, usize, usize) -> i32 =
            slot(threading, 22);
        let run: unsafe extern "win64" fn(usize, *mut Block, usize) -> i32 = slot(threading, 6);
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 = slot(threading, 3);
        let begin: unsafe extern "win64" fn(usize, *mut Block, usize, usize, usize, usize) -> i32 =
            slot(threading, 7);
        let result_size: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 =
            slot(threading, 4);
        let result: unsafe extern "win64" fn(
            usize,
            *mut Block,
            usize,
            usize,
            *mut c_void,
            *mut usize,
        ) -> i32 = slot(threading, 11);

        // a queue the program dispatches itself
        let mut queue = 0usize;
        assert_eq!(create(threading, 0, 0, &mut queue), 0);
        let mut block = Block {
            queue,
            callback: completed as *const () as usize,
            ..Block::default()
        };
        let before = CALLBACKS.load(Ordering::SeqCst);
        assert_eq!(run(threading, &mut block, work as *const () as usize), 0);
        assert_eq!(
            status(threading, &mut block, 0),
            0x8000_000A_u32 as i32,
            "pending until dispatched"
        );
        assert_eq!(dispatch(threading, queue, 0, 2000), 1, "the work");
        assert_eq!(dispatch(threading, queue, 1, 2000), 1, "the completion");
        assert_eq!(dispatch(threading, queue, 1, 0), 0, "nothing left");
        assert_eq!(CALLBACKS.load(Ordering::SeqCst), before + 1);
        assert_eq!(status(threading, &mut block, 0), 0);

        // a provider with a result
        let mut block = Block::default();
        let identity = 1usize;
        assert_eq!(
            begin(
                threading,
                &mut block,
                0,
                identity,
                0,
                provider as *const () as usize
            ),
            0
        );
        assert_eq!(status(threading, &mut block, 1), 0, "waits for the pool");
        let mut size = 0usize;
        assert_eq!(result_size(threading, &mut block, &mut size), 0);
        assert_eq!(size, 4);
        let (mut value, mut used) = (0u32, 0usize);
        assert_eq!(
            result(
                threading,
                &mut block,
                identity,
                4,
                &mut value as *mut u32 as *mut c_void,
                &mut used
            ),
            0
        );
        assert_eq!((value, used), (42, 4));

        // a callback on a pool queue, then termination of the manual one
        let mut pool = 0usize;
        assert_eq!(create(threading, 1, 1, &mut pool), 0);
        unsafe extern "win64" fn bump(context: *mut c_void, canceled: u8) {
            assert_eq!(canceled, 0);
            // SAFETY: the counter the test passed.
            unsafe { (*(context as *const AtomicU32)).fetch_add(1, Ordering::SeqCst) };
        }
        let counter = AtomicU32::new(0);
        assert_eq!(
            submit(
                threading,
                pool,
                0,
                &counter as *const AtomicU32 as usize,
                bump as *const () as usize
            ),
            0
        );
        let started = std::time::Instant::now();
        while counter.load(Ordering::SeqCst) == 0 && started.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        static TERMINATED: AtomicU32 = AtomicU32::new(0);
        unsafe extern "win64" fn terminated(_context: *mut c_void) {
            TERMINATED.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(
            terminate(threading, queue, 1, 0, terminated as *const () as usize),
            0
        );
        assert_eq!(TERMINATED.load(Ordering::SeqCst), 1);
        assert_eq!(
            submit(threading, queue, 0, 0, bump as *const () as usize),
            0x8000_4004_u32 as i32,
            "a terminated queue refuses work"
        );
        close(threading, queue);
        close(threading, pool);
    }
}

#[test]
fn time_sensitivity_is_per_thread() {
    let threading = object("XThreadingImpl");
    // SAFETY: the documented shapes.
    unsafe {
        let set: unsafe extern "win64" fn(usize, u8) -> i32 = slot(threading, 27);
        let is: unsafe extern "win64" fn(usize) -> u8 = slot(threading, 30);
        assert_eq!(is(threading), 0);
        assert_eq!(set(threading, 1), 0);
        assert_eq!(is(threading), 1);
        let other = std::thread::spawn(move || is(threading)).join().unwrap();
        assert_eq!(other, 0);
        set(threading, 0);
    }
}

// ---- the small classes ----

#[test]
fn system_game_and_feature_classes_answer_from_the_package_config() {
    let system = object("XSystemImpl");
    let game = object("XGameImpl");
    let feature = object("XGameRuntimeFeatureImpl");
    let analytics = object("XSystemAnalyticsImpl");
    // SAFETY: the documented shapes.
    unsafe {
        let console: unsafe extern "win64" fn(usize, usize, *mut u8, *mut usize) -> i32 =
            slot(system, 3);
        let mut buffer = [0u8; 64];
        let mut used = 0usize;
        assert_eq!(
            console(system, 10, buffer.as_mut_ptr(), &mut used),
            0x8007_007A_u32 as i32
        );
        assert_eq!(console(system, 64, buffer.as_mut_ptr(), &mut used), 0);
        assert_eq!(used, 39);
        let sandbox: unsafe extern "win64" fn(usize, usize, *mut u8, *mut usize) -> i32 =
            slot(system, 4);
        assert_eq!(sandbox(system, 16, buffer.as_mut_ptr(), &mut used), 0);
        assert_eq!(&buffer[..7], b"RETAIL\0");
        let valid: unsafe extern "win64" fn(usize, usize) -> u8 = slot(system, 7);
        assert_eq!((valid(system, 0), valid(system, 8)), (0, 1));

        let title: unsafe extern "win64" fn(usize, *mut u32) -> i32 = slot(game, 3);
        let mut id = 0u32;
        assert_eq!(title(game, &mut id), 0);
        assert_eq!(id, 0x7952_987A);
        let restart: unsafe extern "win64" fn(usize, *const u8, u32) -> i32 = slot(game, 5);
        assert_eq!(restart(game, cstr("").as_ptr(), 0), 0);

        let available: unsafe extern "win64" fn(usize, u32) -> u8 = slot(feature, 3);
        assert_eq!(available(feature, 2), 1, "XAsync");
        assert_eq!(available(feature, 7), 1, "XGameSave");
        assert_eq!(
            available(feature, 10),
            0,
            "XNetworking is not available offline"
        );
        assert_eq!(available(feature, 1), 0, "XAppCapture is not served");
        assert_eq!(available(feature, 99), 0);

        let info: unsafe extern "win64" fn(usize, *mut [u8; 144]) -> *mut [u8; 144] =
            slot(analytics, 3);
        let mut result = [0u8; 144];
        assert_eq!(info(analytics, &mut result), &mut result as *mut _);
        assert_eq!(&result[16..20], b"Xbox");
    }
}

#[test]
fn package_and_persistent_storage_use_the_config() {
    let package = object("XPackageImpl");
    let storage = object("XPersistentLocalStorageImpl");
    // SAFETY: the documented shapes.
    unsafe {
        let identifier: unsafe extern "win64" fn(usize, usize, *mut u8) -> i32 = slot(package, 3);
        let mut buffer = [0u8; 33];
        assert_eq!(identifier(package, 33, buffer.as_mut_ptr()), 0);
        assert_eq!(&buffer[..13], b"Test.Package\0");
        let packaged: unsafe extern "win64" fn(usize) -> u8 = slot(package, 4);
        assert_eq!(packaged(package), 1);
        let locale: unsafe extern "win64" fn(usize, usize, *mut u8) -> i32 = slot(package, 11);
        assert_eq!(
            locale(package, 3, buffer.as_mut_ptr()),
            0x8007_007A_u32 as i32
        );
        assert_eq!(locale(package, 33, buffer.as_mut_ptr()), 0);
        let mount: unsafe extern "win64" fn(usize, *const u8, *mut usize) -> i32 =
            slot(package, 23);
        let mut handle = 5usize;
        assert_eq!(
            mount(package, cstr("Test.Package").as_ptr(), &mut handle),
            0x8007_0005_u32 as i32
        );
        assert_eq!(handle, 0);
        assert_eq!(
            mount(package, cstr("Other").as_ptr(), &mut handle),
            0x8007_0490_u32 as i32
        );

        let path_size: unsafe extern "win64" fn(usize, *mut usize) -> i32 = slot(storage, 3);
        let path: unsafe extern "win64" fn(usize, usize, *mut u8, *mut usize) -> i32 =
            slot(storage, 4);
        let space: unsafe extern "win64" fn(usize, *mut [u64; 4]) -> i32 = slot(storage, 5);
        let mut size = 0usize;
        assert_eq!(path_size(storage, &mut size), 0);
        let mut folder = vec![0u8; size];
        let mut used = 0usize;
        assert_eq!(path(storage, size, folder.as_mut_ptr(), &mut used), 0);
        assert_eq!(used, size);
        assert!(std::path::Path::new(std::str::from_utf8(&folder[..size - 1]).unwrap()).is_dir());
        let mut info = [0u64; 4];
        assert_eq!(space(storage, &mut info), 0);
        assert_eq!(info[3], 2048 * 1024 * 1024, "the quota of the config");
        assert_eq!(info[1] + info[2], info[3]);

        let prompt: unsafe extern "win64" fn(usize, u64, *mut Block) -> i32 = slot(storage, 6);
        let prompt_result: unsafe extern "win64" fn(usize, *mut Block) -> i32 = slot(storage, 7);
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
            slot(object("XThreadingImpl"), 3);
        let mut block = Block::default();
        assert_eq!(prompt(storage, 1024, &mut block), 0);
        assert_eq!(status(0, &mut block, 1), 0);
        assert_eq!(prompt_result(storage, &mut block), 0);
        let mut block = Block::default();
        assert_eq!(prompt(storage, 4 << 40, &mut block), 0);
        assert_eq!(
            status(0, &mut block, 1),
            0x8007_0070_u32 as i32,
            "more than the quota"
        );
        assert_eq!(prompt_result(storage, &mut block), 0x8007_0070_u32 as i32);
    }
}

#[test]
fn the_user_signs_in_and_can_be_asked_nothing_online() {
    let user = object("XUserImpl");
    let tag = object_as("XUserImpl", "IXUserGamertagImpl");
    let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
        unsafe { slot(object("XThreadingImpl"), 3) };
    // SAFETY: the documented shapes.
    unsafe {
        let add: unsafe extern "win64" fn(usize, u32, *mut Block) -> i32 = slot(user, 7);
        let add_result: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 =
            slot(user, 8);
        let mut block = Block::default();
        assert_eq!(
            add(user, 3, &mut block),
            0x8007_0057_u32 as i32,
            "guests and silent add together"
        );
        assert_eq!(add(user, 1, &mut block), 0);
        assert_eq!(status(0, &mut block, 1), 0);
        let mut handle = 0usize;
        assert_eq!(add_result(user, &mut block, &mut handle), 0);
        assert_ne!(handle, 0);

        let id: unsafe extern "win64" fn(usize, usize, *mut u64) -> i32 = slot(user, 11);
        let mut xuid = 0u64;
        assert_eq!(id(user, handle, &mut xuid), 0);
        assert_ne!(xuid, 0);
        let state: unsafe extern "win64" fn(usize, usize, *mut u32) -> i32 = slot(user, 14);
        let mut value = 9u32;
        assert_eq!(state(user, handle, &mut value), 0);
        assert_eq!(value, 0, "signed in");
        let age: unsafe extern "win64" fn(usize, usize, *mut u32) -> i32 = slot(user, 19);
        assert_eq!(age(user, handle, &mut value), 0);
        assert_eq!(value, 3);
        let privilege: unsafe extern "win64" fn(usize, usize, u32, u32, *mut u8, *mut u32) -> i32 =
            slot(user, 20);
        let mut has = 0u8;
        assert_eq!(privilege(user, handle, 0, 7, &mut has, &mut value), 0);
        assert_eq!(has, 1);
        let token: unsafe extern "win64" fn(usize, usize, u32, u32, *mut Block) -> i32 =
            slot(user, 23);
        assert_eq!(
            token(user, handle, 0, 0, std::ptr::null_mut()),
            0x8924_5111_u32 as i32,
            "no tokens offline"
        );
        let compare: unsafe extern "win64" fn(usize, usize, usize) -> i32 = slot(user, 5);
        let mut other = 0usize;
        let duplicate: unsafe extern "win64" fn(usize, usize, *mut usize) -> i32 = slot(user, 3);
        assert_eq!(duplicate(user, handle, &mut other), 0);
        assert_ne!(other, handle);
        assert_eq!(compare(user, handle, other), 0);
        let close: unsafe extern "win64" fn(usize, usize) = slot(user, 4);
        close(user, other);
        assert_eq!(
            duplicate(user, other, &mut handle),
            0x8007_0057_u32 as i32,
            "a closed handle is refused"
        );

        let gamertag: unsafe extern "win64" fn(
            usize,
            usize,
            u32,
            usize,
            *mut u8,
            *mut usize,
        ) -> i32 = slot(tag, 3);
        let mut text = [0u8; 16];
        let mut used = 0usize;
        assert_eq!(
            gamertag(tag, handle, 0, 16, text.as_mut_ptr(), &mut used),
            0
        );
        assert_eq!(&text[..used], b"Player\0");
        assert_eq!(
            gamertag(tag, handle, 0, 3, text.as_mut_ptr(), &mut used),
            0x8007_007A_u32 as i32
        );
    }
}

#[test]
fn saves_round_trip_through_containers_and_blobs() {
    use std::sync::Mutex;
    let save = object("XGameSaveImpl");
    let user = object("XUserImpl");
    // SAFETY: the documented shapes.
    unsafe {
        let new_user: unsafe extern "win64" fn(usize, u64, *mut usize) -> i32 = slot(user, 12);
        let mut handle = 0usize;
        assert_eq!(new_user(user, 0, &mut handle), 0x8924_5104_u32 as i32);

        let init: unsafe extern "win64" fn(usize, usize, *const u8, u8, *mut usize) -> i32 =
            slot(save, 3);
        let init_async: unsafe extern "win64" fn(usize, usize, *const u8, u8, *mut Block) -> i32 =
            slot(save, 4);
        let init_result: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 =
            slot(save, 5);
        let close_provider: unsafe extern "win64" fn(usize, usize) = slot(save, 6);
        let quota: unsafe extern "win64" fn(usize, usize, *mut i64) -> i32 = slot(save, 7);
        let create_container: unsafe extern "win64" fn(usize, usize, *const u8, *mut usize) -> i32 =
            slot(save, 16);
        let close_container: unsafe extern "win64" fn(usize, usize) = slot(save, 17);
        let create_update: unsafe extern "win64" fn(usize, usize, *const u8, *mut usize) -> i32 =
            slot(save, 23);
        let close_update: unsafe extern "win64" fn(usize, usize) = slot(save, 24);
        let write: unsafe extern "win64" fn(usize, usize, *const u8, *const u8, usize) -> i32 =
            slot(save, 25);
        let delete_blob: unsafe extern "win64" fn(usize, usize, *const u8) -> i32 = slot(save, 26);
        let submit: unsafe extern "win64" fn(usize, usize) -> i32 = slot(save, 27);
        let submit_async: unsafe extern "win64" fn(usize, usize, *mut Block) -> i32 =
            slot(save, 28);
        let submit_result: unsafe extern "win64" fn(usize, *mut Block) -> i32 = slot(save, 29);
        let enumerate_blobs: unsafe extern "win64" fn(usize, usize, usize, usize) -> i32 =
            slot(save, 18);
        let enumerate_containers: unsafe extern "win64" fn(usize, usize, usize, usize) -> i32 =
            slot(save, 14);
        let read: unsafe extern "win64" fn(
            usize,
            usize,
            *const *const u8,
            *mut u32,
            usize,
            *mut u8,
        ) -> i32 = slot(save, 20);
        let read_async: unsafe extern "win64" fn(
            usize,
            usize,
            *const *const u8,
            u32,
            *mut Block,
        ) -> i32 = slot(save, 21);
        let read_result: unsafe extern "win64" fn(
            usize,
            *mut Block,
            usize,
            *mut u8,
            *mut u32,
        ) -> i32 = slot(save, 22);
        let delete_container: unsafe extern "win64" fn(usize, usize, *const u8) -> i32 =
            slot(save, 10);
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
            slot(object("XThreadingImpl"), 3);
        let result_size: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 =
            slot(object("XThreadingImpl"), 4);

        let mut provider = 0usize;
        assert_eq!(
            init(save, 0, cstr("config").as_ptr(), 0, &mut provider),
            0x8007_0057_u32 as i32,
            "needs a user"
        );
        assert_eq!(init(save, 1, cstr("config").as_ptr(), 0, &mut provider), 0);
        let mut block = Block::default();
        let mut second = 0usize;
        assert_eq!(
            init_async(save, 1, cstr("config").as_ptr(), 0, &mut block),
            0
        );
        assert_eq!(status(0, &mut block, 1), 0);
        assert_eq!(init_result(save, &mut block, &mut second), 0);
        assert_ne!(second, 0);
        close_provider(save, second);
        let mut left = 0i64;
        assert_eq!(quota(save, provider, &mut left), 0);
        assert!(left > 0);

        let mut container = 0usize;
        assert_eq!(
            create_container(save, provider, cstr("bad name!").as_ptr(), &mut container),
            0x8083_0001_u32 as i32
        );
        assert_eq!(
            create_container(save, provider, cstr("slot_one").as_ptr(), &mut container),
            0
        );
        let mut update = 0usize;
        assert_eq!(
            create_update(save, container, cstr("Slot One").as_ptr(), &mut update),
            0
        );
        let blob = [7u8; 300];
        assert_eq!(
            write(
                save,
                update,
                cstr("data/main").as_ptr(),
                blob.as_ptr(),
                blob.len()
            ),
            0
        );
        assert_eq!(
            write(save, update, cstr("tiny").as_ptr(), b"abc".as_ptr(), 3),
            0
        );
        assert_eq!(submit(save, update), 0);
        close_update(save, update);

        static SEEN: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
        unsafe extern "win64" fn blob_info(info: *const (usize, u32), _context: *mut c_void) -> u8 {
            // SAFETY: a name and a size, as the callback is documented.
            let (name, size) =
                unsafe { (std::ffi::CStr::from_ptr((*info).0 as *const _), (*info).1) };
            SEEN.lock()
                .unwrap()
                .push((name.to_string_lossy().into_owned(), size));
            1
        }
        assert_eq!(
            enumerate_blobs(save, container, 0, blob_info as *const () as usize),
            0
        );
        assert_eq!(
            *SEEN.lock().unwrap(),
            vec![("data/main".to_string(), 300), ("tiny".to_string(), 3)]
        );

        static CONTAINERS: Mutex<Vec<(String, String, u32, u64)>> = Mutex::new(Vec::new());
        #[repr(C)]
        struct Info {
            name: usize,
            display: usize,
            blobs: u32,
            total: u64,
        }
        unsafe extern "win64" fn container_info(info: *const Info, _context: *mut c_void) -> u8 {
            // SAFETY: the documented container info.
            unsafe {
                let text = |p: usize| {
                    std::ffi::CStr::from_ptr(p as *const _)
                        .to_string_lossy()
                        .into_owned()
                };
                CONTAINERS.lock().unwrap().push((
                    text((*info).name),
                    text((*info).display),
                    (*info).blobs,
                    (*info).total,
                ));
            }
            1
        }
        assert_eq!(
            enumerate_containers(save, provider, 0, container_info as *const () as usize),
            0
        );
        let found = CONTAINERS.lock().unwrap().clone();
        assert!(
            found.contains(&("slot_one".to_string(), "Slot One".to_string(), 2, 303)),
            "{found:?}"
        );

        let names = [cstr("tiny"), cstr("data/main")];
        let pointers = [names[0].as_ptr(), names[1].as_ptr()];
        let mut count = 2u32;
        let mut small = [0u8; 16];
        assert_eq!(
            read(
                save,
                container,
                pointers.as_ptr(),
                &mut count,
                16,
                small.as_mut_ptr()
            ),
            0x8083_0007_u32 as i32
        );
        let mut buffer = vec![0u8; 4096];
        assert_eq!(
            read(
                save,
                container,
                pointers.as_ptr(),
                &mut count,
                buffer.len(),
                buffer.as_mut_ptr()
            ),
            0
        );
        assert_eq!(count, 2);
        let entry = |i: usize| -> (String, u32, Vec<u8>) {
            let at = buffer.as_ptr() as usize;
            let raw = &buffer[i * 24..(i + 1) * 24];
            let name = usize::from_le_bytes(raw[0..8].try_into().unwrap());
            let size = u32::from_le_bytes(raw[8..12].try_into().unwrap());
            let data = usize::from_le_bytes(raw[16..24].try_into().unwrap());
            assert!(name >= at && data >= at, "pointers lead into the buffer");
            // SAFETY: the pointers lead into `buffer`.
            unsafe {
                (
                    std::ffi::CStr::from_ptr(name as *const _)
                        .to_string_lossy()
                        .into_owned(),
                    size,
                    std::slice::from_raw_parts(data as *const u8, size as usize).to_vec(),
                )
            }
        };
        assert_eq!(entry(0), ("tiny".to_string(), 3, b"abc".to_vec()));
        assert_eq!(entry(1), ("data/main".to_string(), 300, blob.to_vec()));
        let missing = [cstr("absent")];
        let missing_pointers = [missing[0].as_ptr()];
        let mut count = 1u32;
        assert_eq!(
            read(
                save,
                container,
                missing_pointers.as_ptr(),
                &mut count,
                buffer.len(),
                buffer.as_mut_ptr()
            ),
            0x8083_0008_u32 as i32
        );

        // the same read, asynchronously, for every blob
        let mut block = Block::default();
        assert_eq!(
            read_async(save, container, std::ptr::null(), 0, &mut block),
            0
        );
        assert_eq!(status(0, &mut block, 1), 0);
        let mut needed = 0usize;
        assert_eq!(result_size(0, &mut block, &mut needed), 0);
        assert_eq!(needed, 2 * 24 + (10 + 5) + 303, "entries, names and data");
        let mut buffer = vec![0u8; needed];
        let mut count = 0u32;
        assert_eq!(
            read_result(save, &mut block, needed, buffer.as_mut_ptr(), &mut count),
            0
        );
        assert_eq!(count, 2);

        // an update that deletes one blob, submitted asynchronously
        let mut update = 0usize;
        assert_eq!(
            create_update(save, container, cstr("Slot One").as_ptr(), &mut update),
            0
        );
        assert_eq!(delete_blob(save, update, cstr("tiny").as_ptr()), 0);
        let mut block = Block::default();
        assert_eq!(submit_async(save, update, &mut block), 0);
        assert_eq!(status(0, &mut block, 1), 0);
        assert_eq!(submit_result(save, &mut block), 0);
        close_update(save, update);
        SEEN.lock().unwrap().clear();
        assert_eq!(
            enumerate_blobs(save, container, 0, blob_info as *const () as usize),
            0
        );
        assert_eq!(*SEEN.lock().unwrap(), vec![("data/main".to_string(), 300)]);

        close_container(save, container);
        assert_eq!(
            delete_container(save, provider, cstr("slot_one").as_ptr()),
            0
        );
        close_provider(save, provider);
        assert_eq!(
            quota(save, provider, &mut left),
            0x8083_000D_u32 as i32,
            "a closed provider is expired"
        );
    }
    let on_disk = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("loader-test")
        .join("Output")
        .join("saves");
    assert!(on_disk.join("local-user").is_dir());
}

#[test]
fn the_store_says_the_game_is_licensed_and_nothing_else_is_for_sale() {
    let store = object("XStoreImpl");
    let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
        unsafe { slot(object("XThreadingImpl"), 3) };
    // SAFETY: the documented shapes.
    unsafe {
        let create: unsafe extern "win64" fn(usize, usize, *mut usize) -> i32 = slot(store, 3);
        let close: unsafe extern "win64" fn(usize, usize) = slot(store, 4);
        let license_async: unsafe extern "win64" fn(usize, usize, *mut Block) -> i32 =
            slot(store, 28);
        let license_result: unsafe extern "win64" fn(usize, *mut Block, *mut u8) -> i32 =
            slot(store, 29);
        let addons_async: unsafe extern "win64" fn(usize, usize, *mut Block) -> i32 =
            slot(store, 30);
        let addons_count: unsafe extern "win64" fn(usize, *mut Block, *mut u32) -> i32 =
            slot(store, 31);
        let unserved: unsafe extern "win64" fn(usize) -> i32 = slot(store, 70);
        let mut context = 0usize;
        assert_eq!(create(store, 0, &mut context), 0x8007_0057_u32 as i32);
        assert_eq!(create(store, 1, &mut context), 0);
        let mut block = Block::default();
        assert_eq!(license_async(store, context, &mut block), 0);
        assert_eq!(status(0, &mut block, 1), 0);
        let mut license = [0u8; 104];
        assert_eq!(license_result(store, &mut block, license.as_mut_ptr()), 0);
        assert_eq!(&license[..5], b"0010\0");
        assert_eq!(&license[0x13..0x16], [1, 0, 0], "active, no trial, no disc");
        assert!(i64::from_le_bytes(license[0x60..0x68].try_into().unwrap()) > 4_000_000_000);

        let mut block = Block::default();
        assert_eq!(addons_async(store, context, &mut block), 0);
        assert_eq!(status(0, &mut block, 1), 0);
        let mut count = 9u32;
        assert_eq!(addons_count(store, &mut block, &mut count), 0);
        assert_eq!(count, 0);
        assert_eq!(
            unserved(store),
            0x8000_4005_u32 as i32,
            "XStoreIsAvailabilityPurchasable is not served"
        );
        close(store, context);
    }
}

#[test]
fn display_launcher_accessibility_devices_ui_streaming_and_networking_answer_offline() {
    let display = object("XDisplayImpl");
    let launcher = object("XLauncherImpl");
    let accessibility = object("XAccessibilityImpl");
    let devices = object("XUserDeviceImpl");
    let ui = object("XGameUiImpl");
    let streaming = object("XGameStreamingImpl");
    let networking = object("XNetworkingImpl");
    let events = object("XGameEventImpl");
    let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
        unsafe { slot(object("XThreadingImpl"), 3) };
    // SAFETY: the documented shapes.
    unsafe {
        let hdr: unsafe extern "win64" fn(usize, u32, *mut [f32; 3]) -> u32 = slot(display, 4);
        let mut info = [1.0f32; 3];
        assert_eq!(hdr(display, 0, &mut info), 2, "disabled");
        let uri: unsafe extern "win64" fn(usize, usize, *const u8) -> i32 = slot(launcher, 3);
        assert_eq!(
            uri(launcher, 1, cstr("https://example.com").as_ptr()),
            0x8000_4005_u32 as i32
        );

        let contrast: unsafe extern "win64" fn(usize, *mut u32) -> i32 = slot(accessibility, 5);
        let mut mode = 9u32;
        assert_eq!(contrast(accessibility, &mut mode), 0);
        assert_eq!(mode, 0);
        let create_voice: unsafe extern "win64" fn(usize, *mut usize) -> i32 =
            slot(accessibility, 9);
        let mut voice = 4usize;
        assert_eq!(
            create_voice(accessibility, &mut voice),
            0x8000_4005_u32 as i32
        );
        assert_eq!(voice, 0);

        let find: unsafe extern "win64" fn(usize, *const u8, *mut usize) -> i32 = slot(devices, 3);
        let mut handle = 0usize;
        assert_eq!(find(devices, [0u8; 32].as_ptr(), &mut handle), 0);
        assert_ne!(handle, 0);
        let endpoint: unsafe extern "win64" fn(
            usize,
            u64,
            u32,
            usize,
            *mut u16,
            *mut usize,
        ) -> i32 = slot(devices, 6);
        let (mut id, mut used) = ([1u16; 4], 7usize);
        assert_eq!(
            endpoint(devices, 1, 0, 4, id.as_mut_ptr(), &mut used),
            0x8007_0490_u32 as i32
        );
        assert_eq!((id[0], used), (0, 0));

        let dialog: unsafe extern "win64" fn(
            usize,
            *mut Block,
            *const u8,
            *const u8,
            *const u8,
            *const u8,
            *const u8,
            u32,
            u32,
        ) -> i32 = slot(ui, 3);
        let dialog_result: unsafe extern "win64" fn(usize, *mut Block, *mut u32) -> i32 =
            slot(ui, 4);
        let mut block = Block::default();
        let (title, text, yes, no) = (cstr("Title"), cstr("Text"), cstr("Yes"), cstr("No"));
        assert_eq!(
            dialog(
                ui,
                &mut block,
                title.as_ptr(),
                text.as_ptr(),
                yes.as_ptr(),
                no.as_ptr(),
                std::ptr::null(),
                1,
                1
            ),
            0
        );
        assert_eq!(status(0, &mut block, 1), 0);
        let mut button = 9u32;
        assert_eq!(dialog_result(ui, &mut block, &mut button), 0);
        assert_eq!(button, 1, "the default button");

        let streaming_now: unsafe extern "win64" fn(usize) -> u8 = slot(streaming, 5);
        assert_eq!(streaming_now(streaming), 0);
        let touch: unsafe extern "win64" fn(usize) -> i32 = slot(streaming, 21);
        assert_eq!(touch(streaming), 0x8924_5400_u32 as i32, "not initialized");

        let hint: unsafe extern "win64" fn(usize, *mut [u32; 4]) -> i32 = slot(networking, 15);
        let mut value = [9u32; 4];
        assert_eq!(hint(networking, &mut value), 0);
        assert_eq!(value[0], 1, "no connectivity");
        let port: unsafe extern "win64" fn(usize, *mut u16) -> i32 = slot(networking, 3);
        let mut udp = 5u16;
        assert_eq!(port(networking, &mut udp), 0x8007_04CF_u32 as i32);
        let security: unsafe extern "win64" fn(usize, *const u16, *mut Block) -> i32 =
            slot(networking, 11);
        let security_size: unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32 =
            slot(networking, 12);
        let mut block = Block::default();
        assert_eq!(
            security(networking, wide("https://example.com").as_ptr(), &mut block),
            0
        );
        assert_eq!(status(0, &mut block, 1), 0x8007_04CF_u32 as i32);
        let mut size = 1usize;
        assert_eq!(
            security_size(networking, &mut block, &mut size),
            0x8007_04CF_u32 as i32
        );
        let verify: unsafe extern "win64" fn(usize, usize, usize) -> i32 = slot(networking, 14);
        assert_eq!(verify(networking, 0, 0), 0x8007_04CF_u32 as i32);
        let unregister: unsafe extern "win64" fn(usize, u64, u8) -> u8 = slot(networking, 17);
        assert_eq!(unregister(networking, 1, 0), 1);

        let write: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            *const u8,
            *const u8,
            *const u8,
            *const u8,
        ) -> i32 = slot(events, 3);
        let name = cstr("BootEvent");
        assert_eq!(
            write(
                events,
                1,
                cstr("cfg").as_ptr(),
                cstr("session").as_ptr(),
                name.as_ptr(),
                cstr("{}").as_ptr(),
                cstr("{}").as_ptr()
            ),
            0
        );
        assert_eq!(
            write(
                events,
                1,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0x8007_0057_u32 as i32
        );
    }
}
