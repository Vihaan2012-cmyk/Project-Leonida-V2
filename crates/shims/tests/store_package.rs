//! `XStoreImpl` and `XPackageImpl`, called through their vtables the way the game's wrappers call them: a
//! store with nothing for sale or owned and no network, and a package whose chunks are the folder's.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

const CLASSES: &str = include_str!("../src/loader/classes.txt");

const NO_NETWORK: i32 = 0x8007_04CF_u32 as i32;
const NOT_FOUND: i32 = 0x8007_0490_u32 as i32;
const ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;
const INVALID_ARG: i32 = 0x8007_0057_u32 as i32;
const POINTER: i32 = 0x8000_4003_u32 as i32;
const PACKAGE: &str = "Test.Package";
const STORE_ID: &str = "9TESTSTOREID";

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

/// The vtable slot of the `nth` method called `method` of a class's one interface chain.
fn slot_of(class: &str, method: &str, nth: usize) -> usize {
    let methods = CLASSES
        .lines()
        .filter_map(|l| l.strip_prefix("iface "))
        .filter(|l| l.split_whitespace().nth(1) == Some(class))
        .flat_map(|l| l.split_whitespace().skip(4))
        .collect::<Vec<_>>();
    3 + methods
        .iter()
        .enumerate()
        .filter(|(_, m)| **m == method)
        .nth(nth)
        .unwrap_or_else(|| panic!("{class}::{method}"))
        .0
}

fn cstr(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

fn configure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("store-package-test");
        let game = root.join("game");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(game.join("pack/one")).unwrap();
        std::fs::write(
            game.join("MicrosoftGame.Config"),
            format!(
                "<Game configVersion=\"1\"><TitleId>7952987A</TitleId><StoreId>{STORE_ID}</StoreId>\
                 <Identity Name=\"{PACKAGE}\" Version=\"1.0.2.1\" />\
                 <ShellVisuals DefaultDisplayName=\"Test Game\" PublisherDisplayName=\"Test Publisher\" />\
                 <Resources><Resource Language=\"en-US\" /><Resource Language=\"de-DE\" /></Resources></Game>"
            ),
        )
        .unwrap();
        std::fs::write(
            game.join("layout_test_xs.xml"),
            "<Package>\
             <Chunk Id=\"1000\"><FileGroup SourcePath=\".\\\" DestinationPath=\"\\\" Include=\"base.bin\" /></Chunk>\
             <Chunk Id=\"1001\"><FileGroup SourcePath=\".\\\" DestinationPath=\"\\\" Include=\"more.bin\" /></Chunk>\
             <Chunk Id=\"3000\">\
             <FileGroup SourcePath=\".\\\" DestinationPath=\"\\pack\\one\\\" Include=\"pack.rpf\" />\
             <FileGroup SourcePath=\".\\\" DestinationPath=\"\\pack\\one\\\" Include=\"missing.rpf\" /></Chunk>\
             </Package>",
        )
        .unwrap();
        std::fs::write(game.join("base.bin"), [0u8; 100]).unwrap();
        std::fs::write(game.join("more.bin"), [0u8; 50]).unwrap();
        std::fs::write(game.join("pack/one/pack.rpf"), [0u8; 8]).unwrap();
        shims::loader::configure(Some(game), root.join("Output"));
    });
}

static QUERIES: Mutex<()> = Mutex::new(());

fn object(class: &str) -> usize {
    configure();
    let _queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = 0usize;
    let hr = shims::loader::query_api(&class_id(class), &class_id(class), &mut out);
    assert_eq!(hr, 0, "{class}");
    out
}

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

fn wait(block: &mut Block) -> i32 {
    let threading = object("XThreadingImpl");
    // SAFETY: XAsyncGetStatus(block, wait).
    unsafe {
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 = slot(threading, 3);
        status(0, block, 1)
    }
}

// ---- the store ----

struct Store {
    object: usize,
    context: usize,
}

fn store() -> Store {
    let object = object("XStoreImpl");
    let mut context = 0usize;
    // SAFETY: XStoreCreateContext(user, &context).
    unsafe {
        let create: unsafe extern "win64" fn(usize, usize, *mut usize) -> i32 =
            slot(object, slot_of("XStoreImpl", "XStoreCreateContext", 0));
        assert_eq!(create(object, 1, &mut context), 0);
    }
    Store { object, context }
}

impl Store {
    fn at<T: Copy>(&self, method: &str) -> T {
        // SAFETY: the caller names the method's own type.
        unsafe { slot(self.object, slot_of("XStoreImpl", method, 0)) }
    }
}

type Begin2 = unsafe extern "win64" fn(usize, usize, *mut Block) -> i32;
type ResultHandle = unsafe extern "win64" fn(usize, *mut Block, *mut usize) -> i32;

static VISITED: AtomicU32 = AtomicU32::new(0);

unsafe extern "win64" fn visit_product(_: *mut c_void, _: *const u8) -> u8 {
    VISITED.fetch_add(1, Ordering::SeqCst);
    1
}

/// Runs a product query that makes a handle and checks the handle behaves as an empty query.
fn check_empty_query(store: &Store, begin: impl FnOnce(&mut Block) -> i32) {
    // SAFETY: the documented shapes.
    unsafe {
        let mut block = Block::default();
        assert_eq!(begin(&mut block), 0);
        assert_eq!(wait(&mut block), 0);
        let result: ResultHandle = store.at("XStoreQueryAssociatedProductsResult");
        let mut handle = 0usize;
        assert_eq!(result(store.object, &mut block, &mut handle), 0);
        assert_ne!(handle, 0);

        let has_more: unsafe extern "win64" fn(usize, usize) -> u8 =
            store.at("XStoreProductsQueryHasMorePages");
        assert_eq!(has_more(store.object, handle), 0);
        let enumerate: unsafe extern "win64" fn(usize, usize, *mut c_void, usize) -> i32 =
            store.at("XStoreEnumerateProductsQuery");
        let before = VISITED.load(Ordering::SeqCst);
        assert_eq!(
            enumerate(
                store.object,
                handle,
                std::ptr::null_mut(),
                visit_product as *const () as usize
            ),
            0
        );
        assert_eq!(
            VISITED.load(Ordering::SeqCst),
            before,
            "no product to visit"
        );

        let next: Begin2 = store.at("XStoreProductsQueryNextPageAsync");
        let mut page = Block::default();
        assert_eq!(next(store.object, handle, &mut page), 0);
        assert_eq!(wait(&mut page), 0);
        let next_result: ResultHandle = store.at("XStoreProductsQueryNextPageResult");
        let mut second = 0usize;
        assert_eq!(next_result(store.object, &mut page, &mut second), 0);
        assert_ne!(second, 0);
        assert_eq!(has_more(store.object, second), 0);

        let close: unsafe extern "win64" fn(usize, usize) =
            store.at("XStoreCloseProductsQueryHandle");
        close(store.object, handle);
        close(store.object, second);
        close(store.object, second);
        assert_eq!(
            enumerate(store.object, handle, std::ptr::null_mut(), 1),
            INVALID_ARG,
            "a closed handle is gone"
        );
    }
}

#[test]
fn every_product_query_succeeds_with_no_products_and_its_handle_works() {
    let store = store();
    // SAFETY: the documented shapes.
    unsafe {
        let associated: unsafe extern "win64" fn(usize, usize, u32, u32, *mut Block) -> i32 =
            store.at("XStoreQueryAssociatedProductsAsync");
        check_empty_query(&store, |b| {
            associated(store.object, store.context, 0x1f, 25, b)
        });
        let entitled: unsafe extern "win64" fn(usize, usize, u32, u32, *mut Block) -> i32 =
            store.at("XStoreQueryEntitledProductsAsync");
        check_empty_query(&store, |b| entitled(store.object, store.context, 1, 25, b));
        let products: unsafe extern "win64" fn(
            usize,
            usize,
            u32,
            *const *const u8,
            usize,
            *const *const u8,
            usize,
            *mut Block,
        ) -> i32 = store.at("XStoreQueryProductsAsync");
        let id = cstr("9NOTOWNED");
        let ids = [id.as_ptr()];
        check_empty_query(&store, |b| {
            products(
                store.object,
                store.context,
                1,
                ids.as_ptr(),
                1,
                std::ptr::null(),
                0,
                b,
            )
        });
        let current: Begin2 = store.at("XStoreQueryProductForCurrentGameAsync");
        check_empty_query(&store, |b| current(store.object, store.context, b));
        let for_package: unsafe extern "win64" fn(usize, usize, u32, *const u8, *mut Block) -> i32 =
            store.at("XStoreQueryProductForPackageAsync");
        let package = cstr(PACKAGE);
        check_empty_query(&store, |b| {
            for_package(store.object, store.context, 1, package.as_ptr(), b)
        });

        let mut block = Block::default();
        assert_eq!(
            associated(store.object, store.context, 1, 25, std::ptr::null_mut()),
            POINTER
        );
        assert_eq!(
            associated(store.object, 0xdead, 1, 25, &mut block),
            INVALID_ARG
        );
    }
}

#[test]
fn what_needs_the_store_service_completes_with_no_network_and_grants_nothing() {
    let store = store();
    let store_id = cstr("9NOTOWNED");
    let ids = [store_id.as_ptr()];
    // SAFETY: the documented shapes.
    unsafe {
        let finish = |block: &mut Block, result: &dyn Fn(&mut Block) -> i32| {
            assert_eq!(wait(block), NO_NETWORK);
            assert_eq!(result(block), NO_NETWORK);
        };

        let acquire: unsafe extern "win64" fn(usize, usize, *const u8, *mut Block) -> i32 =
            store.at("XStoreAcquireLicenseForPackageAsync");
        let acquire_result: ResultHandle = store.at("XStoreAcquireLicenseForPackageResult");
        let mut block = Block::default();
        assert_eq!(
            acquire(store.object, store.context, store_id.as_ptr(), &mut block),
            0
        );
        finish(&mut block, &|b| {
            let mut license = 7usize;
            let hr = acquire_result(store.object, b, &mut license);
            assert_eq!(license, 0, "no license is made");
            hr
        });
        let valid: unsafe extern "win64" fn(usize, usize) -> u8 = store.at("XStoreIsLicenseValid");
        assert_eq!(valid(store.object, 0x1234), 0);
        let close_license: unsafe extern "win64" fn(usize, usize) =
            store.at("XStoreCloseLicenseHandle");
        close_license(store.object, 0x1234);

        let durables: unsafe extern "win64" fn(usize, usize, *const u8, *mut Block) -> i32 =
            store.at("XStoreAcquireLicenseForDurablesAsync");
        let durables_result: ResultHandle = store.at("XStoreAcquireLicenseForDurablesResult");
        let mut block = Block::default();
        assert_eq!(
            durables(store.object, store.context, store_id.as_ptr(), &mut block),
            0
        );
        finish(&mut block, &|b| {
            let mut license = 7usize;
            durables_result(store.object, b, &mut license)
        });

        let balance: unsafe extern "win64" fn(usize, usize, *const u8, *mut Block) -> i32 =
            store.at("XStoreQueryConsumableBalanceRemainingAsync");
        let balance_result: unsafe extern "win64" fn(usize, *mut Block, *mut u32) -> i32 =
            store.at("XStoreQueryConsumableBalanceRemainingResult");
        let mut block = Block::default();
        assert_eq!(
            balance(store.object, store.context, store_id.as_ptr(), &mut block),
            0
        );
        finish(&mut block, &|b| {
            let mut quantity = 0xAAAA_AAAAu32;
            let hr = balance_result(store.object, b, &mut quantity);
            assert_eq!(quantity, 0xAAAA_AAAA, "no balance is invented");
            hr
        });

        let report: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            u32,
            *const [u8; 16],
            *mut Block,
        ) -> i32 = store.at("XStoreReportConsumableFulfillmentAsync");
        let tracking = [0u8; 16];
        let mut block = Block::default();
        assert_eq!(
            report(
                store.object,
                store.context,
                store_id.as_ptr(),
                5,
                &tracking,
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);

        let collections: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            *const u8,
            *mut Block,
        ) -> i32 = store.at("XStoreGetUserCollectionsIdAsync");
        let collections_size: ResultHandle = store.at("XStoreGetUserCollectionsIdResultSize");
        let mut block = Block::default();
        let ticket = cstr("ticket");
        assert_eq!(
            collections(
                store.object,
                store.context,
                ticket.as_ptr(),
                ticket.as_ptr(),
                &mut block
            ),
            0
        );
        finish(&mut block, &|b| {
            let mut size = 9usize;
            let hr = collections_size(store.object, b, &mut size);
            assert_eq!(size, 0);
            hr
        });
        let purchase_id: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            *const u8,
            *mut Block,
        ) -> i32 = store.at("XStoreGetUserPurchaseIdAsync");
        let mut block = Block::default();
        assert_eq!(
            purchase_id(
                store.object,
                store.context,
                ticket.as_ptr(),
                ticket.as_ptr(),
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);

        let purchase: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            *const u8,
            *const u8,
            *mut Block,
        ) -> i32 = store.at("XStoreShowPurchaseUIAsync");
        let mut block = Block::default();
        assert_eq!(
            purchase(
                store.object,
                store.context,
                store_id.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);
        let purchase_result: unsafe extern "win64" fn(usize, *mut Block) -> i32 =
            store.at("XStoreShowPurchaseUIResult");
        assert_eq!(purchase_result(store.object, &mut block), NO_NETWORK);

        let redeem: unsafe extern "win64" fn(
            usize,
            usize,
            *const u8,
            *const *const u8,
            usize,
            u8,
            *mut Block,
        ) -> i32 = store.at("XStoreShowRedeemTokenUIAsync");
        let mut block = Block::default();
        assert_eq!(
            redeem(
                store.object,
                store.context,
                std::ptr::null(),
                ids.as_ptr(),
                1,
                0,
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);

        let page: unsafe extern "win64" fn(usize, usize, *const u8, *mut Block) -> i32 =
            store.at("XStoreShowProductPageUIAsync");
        let mut block = Block::default();
        assert_eq!(
            page(store.object, store.context, store_id.as_ptr(), &mut block),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);

        let download: unsafe extern "win64" fn(
            usize,
            usize,
            *const *const u8,
            usize,
            *mut Block,
        ) -> i32 = store.at("XStoreDownloadAndInstallPackageUpdatesAsync");
        let mut block = Block::default();
        assert_eq!(
            download(store.object, store.context, ids.as_ptr(), 1, &mut block),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);
        let install: unsafe extern "win64" fn(
            usize,
            usize,
            *const *const u8,
            usize,
            *mut Block,
        ) -> i32 = store.at("XStoreDownloadAndInstallPackagesAsync");
        let install_count: unsafe extern "win64" fn(usize, *mut Block, *mut u32) -> i32 =
            store.at("XStoreDownloadAndInstallPackagesResultCount");
        let mut block = Block::default();
        assert_eq!(
            install(store.object, store.context, ids.as_ptr(), 1, &mut block),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);
        let mut count = 3u32;
        assert_eq!(
            install_count(store.object, &mut block, &mut count),
            NO_NETWORK
        );
        assert_eq!(count, 0);

        let mut block = Block::default();
        assert_eq!(
            acquire(store.object, 0xdead, store_id.as_ptr(), &mut block),
            INVALID_ARG,
            "a context the program never made"
        );
    }
    let notes = shims::trace::notes();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("XStoreShowPurchaseUIAsync"))
    );
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.contains("XStoreShowPurchaseUIAsync"))
            .count(),
        1,
        "each call is noted once"
    );
}

#[test]
fn a_package_identifier_is_known_only_for_the_game_itself() {
    let store = store();
    // SAFETY: XStoreQueryPackageIdentifier(storeId, size, buffer).
    unsafe {
        let query: unsafe extern "win64" fn(usize, *const u8, usize, *mut u8) -> i32 =
            store.at("XStoreQueryPackageIdentifier");
        let mut buffer = [0u8; 33];
        assert_eq!(
            query(
                store.object,
                cstr(STORE_ID).as_ptr(),
                33,
                buffer.as_mut_ptr()
            ),
            0
        );
        assert_eq!(&buffer[..13], b"Test.Package\0");
        assert_eq!(
            query(
                store.object,
                cstr(STORE_ID).as_ptr(),
                4,
                buffer.as_mut_ptr()
            ),
            0x8007_007A_u32 as i32
        );
        assert_eq!(
            query(
                store.object,
                cstr("9NOTOWNED").as_ptr(),
                33,
                buffer.as_mut_ptr()
            ),
            NOT_FOUND,
            "no add-on has a package here"
        );
        assert_eq!(
            query(store.object, std::ptr::null(), 33, buffer.as_mut_ptr()),
            INVALID_ARG
        );
    }
}

// ---- the package ----

struct Package {
    object: usize,
}

fn package() -> Package {
    Package {
        object: object("XPackageImpl"),
    }
}

impl Package {
    fn at<T: Copy>(&self, method: &str, nth: usize) -> T {
        // SAFETY: the caller names the method's own type.
        unsafe { slot(self.object, slot_of("XPackageImpl", method, nth)) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Selector {
    kind: u32,
    padding: u32,
    value: usize,
}

const LANGUAGE: u32 = 0;
const TAG: u32 = 1;
const CHUNK: u32 = 2;
const FEATURE: u32 = 3;

fn chunk(id: u32) -> Selector {
    Selector {
        kind: CHUNK,
        padding: 0,
        value: id as usize,
    }
}

type Find = unsafe extern "win64" fn(usize, *const u8, u32, *const Selector, *mut u32) -> i32;

#[test]
fn chunk_availability_follows_the_files_of_the_game_folder() {
    let package = package();
    let name = cstr(PACKAGE);
    let find: Find = package.at("XPackageFindChunkAvailability", 0);
    let ask = |selectors: &[Selector]| {
        let mut state = 99u32;
        // SAFETY: XPackageFindChunkAvailability on the game's own package.
        let hr = unsafe {
            find(
                package.object,
                name.as_ptr(),
                selectors.len() as u32,
                selectors.as_ptr(),
                &mut state,
            )
        };
        assert_eq!(hr, 0);
        state
    };
    let english = cstr("en-US");
    let french = cstr("fr-FR");
    let language = |text: &Vec<u8>| Selector {
        kind: LANGUAGE,
        padding: 0,
        value: text.as_ptr() as usize,
    };
    assert_eq!(ask(&[chunk(1000)]), 0, "installed");
    assert_eq!(ask(&[chunk(1001)]), 0);
    assert_eq!(ask(&[chunk(3000)]), 3, "one of its files is missing");
    assert_eq!(ask(&[chunk(7777)]), 3, "no such chunk");
    assert_eq!(
        ask(&[chunk(1000), chunk(3000)]),
        3,
        "all selectors must be installed"
    );
    assert_eq!(ask(&[chunk(1000), chunk(1001)]), 0);
    assert_eq!(ask(&[language(&english)]), 0, "a language of the config");
    assert_eq!(ask(&[language(&french)]), 3);
    let tag = Selector {
        kind: TAG,
        padding: 0,
        value: english.as_ptr() as usize,
    };
    let feature = Selector {
        kind: FEATURE,
        ..tag
    };
    assert_eq!(ask(&[tag]), 3, "the layout declares no tags");
    assert_eq!(ask(&[feature]), 3);

    // SAFETY: the same call with arguments the game never sends.
    unsafe {
        let mut state = 0u32;
        let other = cstr("Other.Package");
        assert_eq!(
            find(package.object, other.as_ptr(), 1, &chunk(1000), &mut state),
            NOT_FOUND
        );
        assert_eq!(state, 3);
        assert_eq!(
            find(
                package.object,
                name.as_ptr(),
                1,
                std::ptr::null(),
                &mut state
            ),
            INVALID_ARG
        );
        let bad = Selector {
            kind: 9,
            padding: 0,
            value: 0,
        };
        assert_eq!(
            find(package.object, name.as_ptr(), 1, &bad, &mut state),
            INVALID_ARG
        );
        assert_eq!(
            find(
                package.object,
                name.as_ptr(),
                1,
                &chunk(1000),
                std::ptr::null_mut()
            ),
            POINTER
        );
    }
}

static SEEN: Mutex<Vec<(u32, usize, u32)>> = Mutex::new(Vec::new());

unsafe extern "win64" fn collect_chunk(
    _: *mut c_void,
    selector: *const Selector,
    state: u32,
) -> u8 {
    // SAFETY: the selector record of the call.
    let selector = unsafe { *selector };
    let value = if selector.kind == LANGUAGE {
        // SAFETY: a language selector's text.
        unsafe { std::ffi::CStr::from_ptr(selector.value as *const _) }
            .to_bytes()
            .len()
    } else {
        selector.value
    };
    SEEN.lock().unwrap().push((selector.kind, value, state));
    u8::from(selector.kind != CHUNK || selector.value != 1001)
}

#[test]
fn the_chunks_and_languages_of_the_folder_can_be_enumerated() {
    let package = package();
    let name = cstr(PACKAGE);
    let enumerate: unsafe extern "win64" fn(usize, *const u8, u32, *mut c_void, usize) -> i32 =
        package.at("XPackageEnumerateChunkAvailability", 0);
    // SAFETY: XPackageEnumerateChunkAvailability(identifier, kind, context, callback).
    unsafe {
        SEEN.lock().unwrap().clear();
        let callback = collect_chunk as *const () as usize;
        assert_eq!(
            enumerate(
                package.object,
                name.as_ptr(),
                CHUNK,
                std::ptr::null_mut(),
                callback
            ),
            0
        );
        assert_eq!(
            *SEEN.lock().unwrap(),
            [(CHUNK, 1000, 0), (CHUNK, 1001, 0)],
            "the callback returned false at 1001"
        );
        SEEN.lock().unwrap().clear();
        assert_eq!(
            enumerate(
                package.object,
                name.as_ptr(),
                LANGUAGE,
                std::ptr::null_mut(),
                callback
            ),
            0
        );
        assert_eq!(*SEEN.lock().unwrap(), [(LANGUAGE, 5, 0), (LANGUAGE, 5, 0)]);
        SEEN.lock().unwrap().clear();
        assert_eq!(
            enumerate(
                package.object,
                name.as_ptr(),
                TAG,
                std::ptr::null_mut(),
                callback
            ),
            0
        );
        assert!(SEEN.lock().unwrap().is_empty());
        let other = cstr("Other.Package");
        assert_eq!(
            enumerate(
                package.object,
                other.as_ptr(),
                CHUNK,
                std::ptr::null_mut(),
                callback
            ),
            NOT_FOUND
        );
    }
}

type Create =
    unsafe extern "win64" fn(usize, *const u8, u32, *const Selector, u32, usize, *mut usize) -> i32;

fn progress(package: &Package, monitor: usize) -> [u8; 32] {
    let get: unsafe extern "win64" fn(usize, usize, *mut [u8; 32]) =
        package.at("XPackageGetInstallationProgress", 0);
    let mut record = [0xEE; 32];
    // SAFETY: XPackageGetInstallationProgress(monitor, &progress).
    unsafe { get(package.object, monitor, &mut record) };
    record
}

#[test]
fn the_game_package_has_a_finished_installation_monitor() {
    let package = package();
    let name = cstr(PACKAGE);
    let create: Create = package.at("XPackageCreateInstallationMonitor", 0);
    let close: unsafe extern "win64" fn(usize, usize) =
        package.at("XPackageCloseInstallationMonitorHandle", 0);
    let register: unsafe extern "win64" fn(usize, usize, usize, usize, *mut u64) -> i32 =
        package.at("XPackageRegisterInstallationProgressChanged", 0);
    let unregister: unsafe extern "win64" fn(usize, usize, u64, u8) -> u8 =
        package.at("XPackageUnregisterInstallationProgressChanged", 0);
    // SAFETY: the documented shapes.
    unsafe {
        let mut monitor = 0usize;
        assert_eq!(
            create(
                package.object,
                name.as_ptr(),
                0,
                std::ptr::null(),
                1000,
                0,
                &mut monitor
            ),
            0
        );
        assert_ne!(monitor, 0);
        let record = progress(&package, monitor);
        let total = u64::from_le_bytes(record[..8].try_into().unwrap());
        assert_eq!(total, 100 + 50, "the files of the installed chunks");
        assert_eq!(&record[8..16], &record[..8], "all of it installed");
        assert_eq!(
            (record[0x18], record[0x19]),
            (1, 1),
            "launchable and completed"
        );

        let mut token = 0u64;
        assert_eq!(
            register(package.object, monitor, 0, 0, &mut token),
            INVALID_ARG
        );
        unsafe extern "win64" fn never(_: *mut c_void, _: usize) {
            panic!("the progress of a finished install does not change");
        }
        assert_eq!(
            register(
                package.object,
                monitor,
                0,
                never as *const () as usize,
                &mut token
            ),
            0
        );
        assert_ne!(token, 0);
        assert_eq!(unregister(package.object, monitor, token, 1), 1);

        close(package.object, monitor);
        assert_eq!(
            progress(&package, monitor),
            [0; 32],
            "a closed monitor reports nothing"
        );
        assert_eq!(
            register(
                package.object,
                monitor,
                0,
                never as *const () as usize,
                &mut token
            ),
            INVALID_ARG
        );

        let other = cstr("Other.Package");
        assert_eq!(
            create(
                package.object,
                other.as_ptr(),
                0,
                std::ptr::null(),
                1000,
                0,
                &mut monitor
            ),
            NOT_FOUND
        );
        assert_eq!(monitor, 0);
        assert_eq!(
            create(
                package.object,
                name.as_ptr(),
                1,
                &chunk(3000),
                1000,
                0,
                &mut monitor
            ),
            NOT_FOUND,
            "a chunk that is not there cannot be watched"
        );
        let one = [chunk(1001)];
        assert_eq!(
            create(
                package.object,
                name.as_ptr(),
                1,
                one.as_ptr(),
                1000,
                0,
                &mut monitor
            ),
            0
        );
        assert_eq!(
            u64::from_le_bytes(progress(&package, monitor)[..8].try_into().unwrap()),
            50
        );
        close(package.object, monitor);
    }
}

static REPORTED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "win64" fn reported(_: *mut c_void, monitor: usize) {
    REPORTED.store(monitor, Ordering::SeqCst);
}

#[test]
fn installing_chunks_that_are_there_finishes_and_missing_ones_cannot_be_fetched() {
    let package = package();
    let name = cstr(PACKAGE);
    type InstallAsync = unsafe extern "win64" fn(
        usize,
        *const u8,
        u32,
        *const Selector,
        u32,
        u8,
        *mut Block,
    ) -> i32;
    let install: InstallAsync = package.at("XPackageInstallChunksAsync", 0);
    let result: ResultHandle = package.at("XPackageInstallChunksResult", 0);
    let register: unsafe extern "win64" fn(usize, usize, usize, usize, *mut u64) -> i32 =
        package.at("XPackageRegisterInstallationProgressChanged", 0);
    // SAFETY: the documented shapes.
    unsafe {
        let ready = [chunk(1000)];
        let mut block = Block::default();
        assert_eq!(
            install(
                package.object,
                name.as_ptr(),
                1,
                ready.as_ptr(),
                0,
                0,
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), 0);
        let mut monitor = 0usize;
        assert_eq!(result(package.object, &mut block, &mut monitor), 0);
        assert_ne!(monitor, 0);
        assert_eq!(progress(&package, monitor)[0x19], 1);
        REPORTED.store(0, Ordering::SeqCst);
        let mut token = 0u64;
        assert_eq!(
            register(
                package.object,
                monitor,
                0,
                reported as *const () as usize,
                &mut token
            ),
            0
        );
        let started = Instant::now();
        while REPORTED.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            REPORTED.load(Ordering::SeqCst),
            monitor,
            "a waiting caller is told the install is over"
        );

        let missing = [chunk(3000)];
        let mut block = Block::default();
        assert_eq!(
            install(
                package.object,
                name.as_ptr(),
                1,
                missing.as_ptr(),
                0,
                0,
                &mut block
            ),
            0
        );
        assert_eq!(wait(&mut block), NO_NETWORK);
        let mut monitor = 5usize;
        assert_eq!(result(package.object, &mut block, &mut monitor), NO_NETWORK);
        assert_eq!(monitor, 0);

        let other = cstr("Other.Package");
        let mut block = Block::default();
        assert_eq!(
            install(
                package.object,
                other.as_ptr(),
                1,
                ready.as_ptr(),
                0,
                0,
                &mut block
            ),
            NOT_FOUND
        );
    }
}

#[test]
fn size_estimates_and_uninstalls_are_the_ones_of_a_finished_install() {
    let package = package();
    let name = cstr(PACKAGE);
    let estimate: unsafe extern "win64" fn(
        usize,
        *const u8,
        u32,
        *const Selector,
        *mut u64,
        *mut u8,
    ) -> i32 = package.at("XPackageEstimateDownloadSize", 0);
    let uninstall: unsafe extern "win64" fn(usize, *const u8, u32, *const Selector) -> i32 =
        package.at("XPackageUninstallChunks", 0);
    let reorder: unsafe extern "win64" fn(usize, *const u8, u32, *const Selector) -> i32 =
        package.at("XPackageChangeChunkInstallOrder", 0);
    let uninstall_package: unsafe extern "win64" fn(usize, *const u8) -> u8 =
        package.at("XPackageUninstallPackage", 0);
    // SAFETY: the documented shapes.
    unsafe {
        let (mut bytes, mut confirm) = (9u64, 9u8);
        let ready = [chunk(1000)];
        assert_eq!(
            estimate(
                package.object,
                name.as_ptr(),
                1,
                ready.as_ptr(),
                &mut bytes,
                &mut confirm
            ),
            0
        );
        assert_eq!((bytes, confirm), (0, 0), "nothing to download");
        assert_eq!(
            estimate(
                package.object,
                name.as_ptr(),
                1,
                ready.as_ptr(),
                &mut bytes,
                std::ptr::null_mut()
            ),
            0
        );
        let missing = [chunk(3000)];
        assert_eq!(
            estimate(
                package.object,
                name.as_ptr(),
                1,
                missing.as_ptr(),
                &mut bytes,
                &mut confirm
            ),
            NOT_FOUND
        );

        assert_eq!(reorder(package.object, name.as_ptr(), 1, ready.as_ptr()), 0);
        assert_eq!(
            uninstall(package.object, name.as_ptr(), 1, ready.as_ptr()),
            ACCESS_DENIED,
            "the game folder is never changed"
        );
        assert_eq!(uninstall_package(package.object, name.as_ptr()), 0);
        let other = cstr("Other.Package");
        assert_eq!(
            uninstall(package.object, other.as_ptr(), 1, ready.as_ptr()),
            NOT_FOUND
        );
    }
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("store-package-test/game");
    assert!(folder.join("base.bin").is_file());
}

#[test]
fn content_packages_cannot_be_mounted_and_the_game_itself_is_denied() {
    let package = package();
    let mount: unsafe extern "win64" fn(usize, *const u8, *mut Block) -> i32 =
        package.at("XPackageMountWithUiAsync", 0);
    let result: ResultHandle = package.at("XPackageMountWithUiResult", 0);
    // SAFETY: the documented shapes.
    unsafe {
        let dlc = cstr("Some.Dlc.Package");
        let mut block = Block::default();
        assert_eq!(mount(package.object, dlc.as_ptr(), &mut block), 0);
        assert_eq!(wait(&mut block), NOT_FOUND);
        let mut handle = 5usize;
        assert_eq!(result(package.object, &mut block, &mut handle), NOT_FOUND);
        assert_eq!(handle, 0);

        let own = cstr(PACKAGE);
        let mut block = Block::default();
        assert_eq!(mount(package.object, own.as_ptr(), &mut block), 0);
        assert_eq!(wait(&mut block), ACCESS_DENIED);

        let mut block = Block::default();
        assert_eq!(
            mount(package.object, std::ptr::null(), &mut block),
            INVALID_ARG
        );
        assert_eq!(
            mount(package.object, dlc.as_ptr(), std::ptr::null_mut()),
            POINTER
        );
    }
}

#[repr(C)]
struct Details {
    identifier: *const u8,
    version: [u16; 4],
    kind: u32,
    display_name: *const u8,
    description: *const u8,
    publisher: *const u8,
    store_id: *const u8,
    installing: u8,
}

type Reported = (String, [u16; 4], String, String, String);

static PACKAGES: Mutex<Vec<Reported>> = Mutex::new(Vec::new());

unsafe extern "win64" fn collect_package(_: *mut c_void, details: *const Details) -> u8 {
    // SAFETY: the record of the call, its texts C strings.
    unsafe {
        let text = |p: *const u8| {
            std::ffi::CStr::from_ptr(p as *const _)
                .to_string_lossy()
                .into_owned()
        };
        let d = &*details;
        assert_eq!(d.kind, 0, "a game");
        assert_eq!(d.installing, 0);
        PACKAGES.lock().unwrap().push((
            text(d.identifier),
            d.version,
            text(d.display_name),
            text(d.publisher),
            text(d.store_id),
        ));
    }
    1
}

#[test]
fn enumerating_packages_reports_the_game_and_no_content_in_every_interface_version() {
    let package = package();
    // SAFETY: XPackageEnumeratePackages(kind, scope, context, callback), in each interface version.
    unsafe {
        for nth in 0..3 {
            let slot_number = slot_of("XPackageImpl", "XPackageEnumeratePackages", nth);
            let enumerate: unsafe extern "win64" fn(usize, u32, u32, *mut c_void, usize) -> i32 =
                slot(package.object, slot_number);
            PACKAGES.lock().unwrap().clear();
            let callback = collect_package as *const () as usize;
            assert_eq!(
                enumerate(package.object, 0, 0, std::ptr::null_mut(), callback),
                0
            );
            assert_eq!(
                *PACKAGES.lock().unwrap(),
                [(
                    PACKAGE.to_string(),
                    [1, 0, 2, 1],
                    "Test Game".to_string(),
                    "Test Publisher".to_string(),
                    STORE_ID.to_string()
                )],
                "slot {slot_number}"
            );
            PACKAGES.lock().unwrap().clear();
            assert_eq!(
                enumerate(package.object, 1, 0, std::ptr::null_mut(), callback),
                0
            );
            assert!(PACKAGES.lock().unwrap().is_empty(), "no content package");
            assert_eq!(
                enumerate(package.object, 0, 0, std::ptr::null_mut(), 0),
                INVALID_ARG
            );
        }

        let mut tokens = Vec::new();
        for nth in 0..3 {
            let register: unsafe extern "win64" fn(usize, usize, usize, usize, *mut u64) -> i32 =
                slot(
                    package.object,
                    slot_of("XPackageImpl", "XPackageRegisterPackageInstalled", nth),
                );
            let mut token = 0u64;
            assert_eq!(register(package.object, 0, 0, 1, &mut token), 0);
            assert_ne!(token, 0);
            tokens.push(token);
        }
        let unregister: unsafe extern "win64" fn(usize, u64, u8) -> u8 =
            package.at("XPackageUnregisterPackageInstalled", 0);
        assert_eq!(unregister(package.object, tokens[0], 1), 1);

        let features: unsafe extern "win64" fn(usize, *const u8, *mut c_void, usize) -> i32 =
            package.at("XPackageEnumerateFeatures", 0);
        let name = cstr(PACKAGE);
        assert_eq!(
            features(package.object, name.as_ptr(), std::ptr::null_mut(), 1),
            0
        );
        let other = cstr("Other.Package");
        assert_eq!(
            features(package.object, other.as_ptr(), std::ptr::null_mut(), 1),
            NOT_FOUND
        );
    }
}
