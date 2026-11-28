//! `XStoreImpl`: what the Microsoft Store can answer with no network and nothing bought: the game license is
//! active and not a trial, there are no add-on licenses, no entitled or associated products, no package
//! updates, and license-change registrations hand out tokens whose callbacks never run. Everything that
//! would need the Store service (license purchases, consumables, collection ids, purchase and redeem
//! screens, downloads) completes with "no network". Nothing is ever reported as owned.
//!
//! Measured on the executable (calls go through the `XStoreImpl` wrappers at RVAs 0x55e0fcc-0x55e26e8,
//! interface version 6, durables on version 3): `XStoreGameLicense` is 104 bytes, read as a C string SKU id
//! at +0x00, three booleans at +0x13..+0x15, a C string trial id at +0x1C and the expiration time at +0x60.
//! The expiration and the SKU text are not measured; the license never expires. The game queries associated
//! and entitled products with 25 items a page and a 30 second timeout, enumerates the product handle, asks
//! for another page only while `XStoreProductsQueryHasMorePages` says so and closes it. A failed call is
//! logged by the game and its operation reports failure; none is fatal. It acquires licenses by package
//! (after `XStoreQueryPackageIdentifier` of a 33-byte buffer) or for durables, asks for game and DLC package
//! updates (a count of zero ends the flow), and waits on every block it starts.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::com::{ClassImpl, c_str, method, write_c_str};
use super::tasks::{self, AsyncBlock};
use super::{
    E_FAIL, E_INVALIDARG, E_POINTER, HR_INSUFFICIENT_BUFFER, HR_NETWORK_UNREACHABLE, HR_NOT_FOUND,
    S_OK, element_text, game_config, package_identifier,
};
use crate::trace::note;

type This = *mut c_void;

const GAME_LICENSE_BYTES: usize = 104;
const SKU: &[u8] = b"0010";
const NEVER_EXPIRES: i64 = 253_402_300_799;

/// What the Store answers with no network.
const STORE_UNAVAILABLE: i32 = HR_NETWORK_UNREACHABLE;

static CONTEXTS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static QUERIES: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static NOTED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
static TOKENS: AtomicU64 = AtomicU64::new(1 << 48);

fn set(
    slot: &'static Mutex<Option<HashSet<usize>>>,
) -> MutexGuard<'static, Option<HashSet<usize>>> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

fn contexts<T>(f: impl FnOnce(&mut HashSet<usize>) -> T) -> T {
    f(set(&CONTEXTS).get_or_insert_with(HashSet::new))
}

/// Notes `text` the first time `key` is seen.
pub(super) fn note_once(key: &'static str, text: &str) {
    let first = NOTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(key);
    if first {
        note(text.to_string());
    }
}

unsafe extern "win64" fn create_context(_: This, user: usize, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    if user == 0 {
        return E_INVALIDARG;
    }
    let context = Box::into_raw(Box::new(0u64)) as usize;
    contexts(|set| set.insert(context));
    // SAFETY: writable, checked.
    unsafe { *out = context };
    S_OK
}

unsafe extern "win64" fn close_context(_: This, context: usize) {
    if contexts(|set| set.remove(&context)) {
        // SAFETY: the box `create_context` made, closed once.
        drop(unsafe { Box::from_raw(context as *mut u64) });
    }
}

fn game_license() -> Vec<u8> {
    let mut license = vec![0u8; GAME_LICENSE_BYTES];
    license[..SKU.len()].copy_from_slice(SKU);
    license[0x13] = 1;
    license[0x60..0x68].copy_from_slice(&NEVER_EXPIRES.to_le_bytes());
    license
}

unsafe extern "win64" fn query_game_license_async(
    _: This,
    context: usize,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !contexts(|set| set.contains(&context)) {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XStoreQueryGameLicenseAsync", || {
            (S_OK, game_license())
        })
    }
}

unsafe extern "win64" fn query_game_license_result(
    _: This,
    block: *mut AsyncBlock,
    license: *mut u8,
) -> i32 {
    if license.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block and a license struct of 104 bytes.
    unsafe {
        tasks::one_shot_result(
            block,
            "XStoreQueryGameLicenseAsync",
            GAME_LICENSE_BYTES,
            license as *mut c_void,
        )
    }
}

/// An add-on or update query that finds nothing: the count call reads zero.
unsafe extern "win64" fn query_none_async(_: This, context: usize, block: *mut AsyncBlock) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !contexts(|set| set.contains(&context)) {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe { tasks::begin_one_shot(block, "XStoreQueryNone", || (S_OK, Vec::new())) }
}

unsafe extern "win64" fn query_package_updates_async(
    _: This,
    context: usize,
    _package_ids: *const *const u8,
    _count: usize,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { query_none_async(std::ptr::null_mut(), context, block) }
}

unsafe extern "win64" fn query_none_count(_: This, block: *mut AsyncBlock, count: *mut u32) -> i32 {
    if block.is_null() || count.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *count = 0 };
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

unsafe extern "win64" fn query_none_result(
    _: This,
    block: *mut AsyncBlock,
    _count: u32,
    _results: *mut c_void,
) -> i32 {
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

/// A registration is a token and the callback never runs: the license never changes offline.
unsafe extern "win64" fn register_license(
    _: This,
    context: usize,
    _queue: usize,
    _callback_context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if callback == 0 || token.is_null() {
        return E_INVALIDARG;
    }
    if !contexts(|set| set.contains(&context)) {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) };
    S_OK
}

unsafe extern "win64" fn unregister_license(
    _: This,
    _context: usize,
    _token: u64,
    _wait: u8,
) -> u8 {
    1
}

// ---- product queries: nothing is for sale, entitled or associated ----

const PRODUCTS: &str = "XStoreQueryProducts";

/// A product query handle whose products are none and whose pages are one. Nothing to read, so the handle is
/// only a token the program closes.
fn new_query() -> usize {
    let handle = Box::into_raw(Box::new(0u64)) as usize;
    set(&QUERIES)
        .get_or_insert_with(HashSet::new)
        .insert(handle);
    handle
}

fn is_query(handle: usize) -> bool {
    set(&QUERIES)
        .as_ref()
        .is_some_and(|queries| queries.contains(&handle))
}

/// # Safety
/// `block` is a writable block the program keeps alive until the call completes.
unsafe fn begin_products(context: usize, block: *mut AsyncBlock, call: &'static str) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !contexts(|set| set.contains(&context)) {
        return E_INVALIDARG;
    }
    note_once(call, &format!("{call}: answered with no products"));
    // SAFETY: as the caller promises.
    unsafe {
        tasks::begin_one_shot(block, PRODUCTS, || {
            (S_OK, new_query().to_le_bytes().to_vec())
        })
    }
}

/// `XStoreQueryAssociatedProductsAsync(context, kinds, maxItemsPerPage, block)`.
unsafe extern "win64" fn query_associated_async(
    _: This,
    context: usize,
    _kinds: u32,
    _page: u32,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { begin_products(context, block, "XStoreQueryAssociatedProductsAsync") }
}

/// `XStoreQueryEntitledProductsAsync(context, kinds, maxItemsPerPage, block)`: nothing is entitled.
unsafe extern "win64" fn query_entitled_async(
    _: This,
    context: usize,
    _kinds: u32,
    _page: u32,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { begin_products(context, block, "XStoreQueryEntitledProductsAsync") }
}

/// `XStoreQueryProductsAsync(context, kinds, ids, idCount, filters, filterCount, block)`.
unsafe extern "win64" fn query_products_async(
    _: This,
    context: usize,
    _kinds: u32,
    _ids: *const *const u8,
    _id_count: usize,
    _filters: *const *const u8,
    _filter_count: usize,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { begin_products(context, block, "XStoreQueryProductsAsync") }
}

/// `XStoreQueryProductForCurrentGameAsync(context, block)`.
unsafe extern "win64" fn query_current_game_async(
    _: This,
    context: usize,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { begin_products(context, block, "XStoreQueryProductForCurrentGameAsync") }
}

/// `XStoreQueryProductForPackageAsync(context, kinds, package, block)`.
unsafe extern "win64" fn query_for_package_async(
    _: This,
    context: usize,
    _kinds: u32,
    _package: *const u8,
    block: *mut AsyncBlock,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { begin_products(context, block, "XStoreQueryProductForPackageAsync") }
}

/// The result of any call that makes a product query handle.
unsafe extern "win64" fn products_result(
    _: This,
    block: *mut AsyncBlock,
    handle: *mut usize,
) -> i32 {
    if handle.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *handle = 0 };
    // SAFETY: the program's block and handle.
    unsafe { tasks::one_shot_result(block, PRODUCTS, 8, handle as *mut c_void) }
}

/// `XStoreEnumerateProductsQuery(handle, context, callback)`: no product to visit.
unsafe extern "win64" fn enumerate_products(
    _: This,
    handle: usize,
    _context: *mut c_void,
    callback: usize,
) -> i32 {
    if callback == 0 || !is_query(handle) {
        return E_INVALIDARG;
    }
    S_OK
}

unsafe extern "win64" fn products_has_more_pages(_: This, _handle: usize) -> u8 {
    0
}

/// `XStoreProductsQueryNextPageAsync(handle, block)`: there is never a second page, so the next one is empty.
unsafe extern "win64" fn products_next_page_async(
    _: This,
    handle: usize,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !is_query(handle) {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, PRODUCTS, || {
            (S_OK, new_query().to_le_bytes().to_vec())
        })
    }
}

unsafe extern "win64" fn close_products_query(_: This, handle: usize) {
    if set(&QUERIES)
        .as_mut()
        .is_some_and(|queries| queries.remove(&handle))
    {
        // SAFETY: the box `new_query` made, closed once.
        drop(unsafe { Box::from_raw(handle as *mut u64) });
    }
}

// ---- calls the Store service must answer ----

const UNAVAILABLE: &str = "XStoreUnavailable";

/// # Safety
/// `block` is a writable block the program keeps alive until the call completes.
unsafe fn begin_unavailable(context: usize, block: *mut AsyncBlock, call: &'static str) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !contexts(|set| set.contains(&context)) {
        return E_INVALIDARG;
    }
    note_once(call, &format!("{call}: the Store is unavailable offline"));
    // SAFETY: as the caller promises.
    unsafe { tasks::begin_one_shot(block, UNAVAILABLE, || (STORE_UNAVAILABLE, Vec::new())) }
}

/// Defines the `Async` function of a call that begins and completes with "no network": the arguments after
/// the context, then the block.
macro_rules! unavailable {
    ($function:ident, $call:literal $(, $argument:ident: $kind:ty)*) => {
        unsafe extern "win64" fn $function(
            _: This,
            context: usize,
            $($argument: $kind,)*
            block: *mut AsyncBlock,
        ) -> i32 {
            $(let _ = $argument;)*
            // SAFETY: the program's arguments.
            unsafe { begin_unavailable(context, block, $call) }
        }
    };
}

unavailable!(can_acquire_store_id_async, "XStoreCanAcquireLicenseForStoreIdAsync", store_id: *const u8);
unavailable!(can_acquire_package_async, "XStoreCanAcquireLicenseForPackageAsync", package: *const u8);
unavailable!(acquire_package_async, "XStoreAcquireLicenseForPackageAsync", package: *const u8);
unavailable!(acquire_durables_async, "XStoreAcquireLicenseForDurablesAsync", store_id: *const u8);
unavailable!(consumable_balance_async, "XStoreQueryConsumableBalanceRemainingAsync", store_id: *const u8);
unavailable!(
    report_consumable_async,
    "XStoreReportConsumableFulfillmentAsync",
    store_id: *const u8,
    quantity: u32,
    tracking_id: *const u8
);
unavailable!(
    user_collections_id_async,
    "XStoreGetUserCollectionsIdAsync",
    ticket: *const u8,
    publisher_user: *const u8
);
unavailable!(
    user_purchase_id_async,
    "XStoreGetUserPurchaseIdAsync",
    ticket: *const u8,
    publisher_user: *const u8
);
unavailable!(
    license_token_async,
    "XStoreQueryLicenseTokenAsync",
    store_ids: *const *const u8,
    count: usize,
    custom: *const u8
);
unavailable!(
    show_purchase_async,
    "XStoreShowPurchaseUIAsync",
    store_id: *const u8,
    name: *const u8,
    extended: *const u8
);
unavailable!(
    show_redeem_token_async,
    "XStoreShowRedeemTokenUIAsync",
    token: *const u8,
    allowed: *const *const u8,
    count: usize,
    disallow_csv: u8
);
unavailable!(show_rate_and_review_async, "XStoreShowRateAndReviewUIAsync");
unavailable!(
    show_associated_products_async,
    "XStoreShowAssociatedProductsUIAsync",
    store_id: *const u8,
    kinds: u32
);
unavailable!(
    show_product_page_async,
    "XStoreShowProductPageUIAsync",
    store_id: *const u8
);
unavailable!(
    download_updates_async,
    "XStoreDownloadPackageUpdatesAsync",
    package_ids: *const *const u8,
    count: usize
);
unavailable!(
    download_and_install_updates_async,
    "XStoreDownloadAndInstallPackageUpdatesAsync",
    package_ids: *const *const u8,
    count: usize
);
unavailable!(
    download_and_install_packages_async,
    "XStoreDownloadAndInstallPackagesAsync",
    store_ids: *const *const u8,
    count: usize
);

/// A result whose call failed: the status of the block, nothing written.
unsafe extern "win64" fn unavailable_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

/// A result that writes a handle or a size first: zero, then the status. Results into larger records are
/// left unwritten.
unsafe extern "win64" fn unavailable_result_out(
    _: This,
    block: *mut AsyncBlock,
    out: *mut usize,
) -> i32 {
    if !out.is_null() {
        // SAFETY: writable, checked.
        unsafe { *out = 0 };
    }
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

/// `XStoreIsLicenseValid(handle)`: no license is ever made.
unsafe extern "win64" fn is_license_valid(_: This, _handle: usize) -> u8 {
    0
}

unsafe extern "win64" fn close_license(_: This, _handle: usize) {}

/// `XStoreQueryPackageIdentifier(storeId, size, buffer)`: the game's own Store id is its package, no other
/// product has a package here.
unsafe extern "win64" fn query_package_identifier(
    _: This,
    store_id: *const u8,
    size: usize,
    buffer: *mut u8,
) -> i32 {
    // SAFETY: a string or null.
    let Some(store_id) = (unsafe { c_str(store_id) }) else {
        return E_INVALIDARG;
    };
    if buffer.is_null() || size == 0 {
        return E_INVALIDARG;
    }
    let own = game_config().and_then(|xml| element_text(&xml, "StoreId"));
    if own.as_deref() != Some(store_id.as_str()) {
        note_once(
            "XStoreQueryPackageIdentifier",
            &format!("XStoreQueryPackageIdentifier({store_id}): no package for this product"),
        );
        return HR_NOT_FOUND;
    }
    // SAFETY: `size` bytes are writable.
    if unsafe { write_c_str(buffer, size, &package_identifier()) } {
        S_OK
    } else {
        HR_INSUFFICIENT_BUFFER
    }
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![ClassImpl {
        class: "XStoreImpl",
        root: "IXStoreImpl",
        methods: vec![
            method!("XStoreCreateContext", create_context),
            method!("XStoreCloseContextHandle", close_context),
            method!("XStoreQueryGameLicenseAsync", query_game_license_async),
            method!("XStoreQueryGameLicenseResult", query_game_license_result),
            method!("XStoreQueryAddOnLicensesAsync", query_none_async),
            method!("XStoreQueryAddOnLicensesResultCount", query_none_count),
            method!("XStoreQueryAddOnLicensesResult", query_none_result),
            method!("XStoreQueryGameAndDlcPackageUpdatesAsync", query_none_async),
            method!(
                "XStoreQueryGameAndDlcPackageUpdatesResultCount",
                query_none_count
            ),
            method!(
                "XStoreQueryGameAndDlcPackageUpdatesResult",
                query_none_result
            ),
            method!("XStoreRegisterGameLicenseChanged", register_license),
            method!("XStoreUnregisterGameLicenseChanged", unregister_license),
            method!("XStoreRegisterPackageLicenseLost", register_license),
            method!("XStoreUnregisterPackageLicenseLost", unregister_license),
            method!(
                "XStoreQueryPackageUpdatesAsync",
                query_package_updates_async
            ),
            method!("XStoreQueryPackageUpdatesResultCount", query_none_count),
            method!("XStoreQueryPackageUpdatesResult", query_none_result),
            method!("XStoreQueryAssociatedProductsAsync", query_associated_async),
            method!("XStoreQueryAssociatedProductsResult", products_result),
            method!("XStoreQueryProductsAsync", query_products_async),
            method!("XStoreQueryProductsResult", products_result),
            method!("XStoreQueryEntitledProductsAsync", query_entitled_async),
            method!("XStoreQueryEntitledProductsResult", products_result),
            method!(
                "XStoreQueryProductForCurrentGameAsync",
                query_current_game_async
            ),
            method!("XStoreQueryProductForCurrentGameResult", products_result),
            method!("XStoreQueryProductForPackageAsync", query_for_package_async),
            method!("XStoreQueryProductForPackageResult", products_result),
            method!("XStoreEnumerateProductsQuery", enumerate_products),
            method!("XStoreProductsQueryHasMorePages", products_has_more_pages),
            method!("XStoreProductsQueryNextPageAsync", products_next_page_async),
            method!("XStoreProductsQueryNextPageResult", products_result),
            method!("XStoreCloseProductsQueryHandle", close_products_query),
            method!("XStoreAcquireLicenseForPackageAsync", acquire_package_async),
            method!(
                "XStoreAcquireLicenseForPackageResult",
                unavailable_result_out
            ),
            method!("XStoreIsLicenseValid", is_license_valid),
            method!("XStoreCloseLicenseHandle", close_license),
            method!(
                "XStoreCanAcquireLicenseForStoreIdAsync",
                can_acquire_store_id_async
            ),
            method!(
                "XStoreCanAcquireLicenseForStoreIdResult",
                unavailable_result
            ),
            method!(
                "XStoreCanAcquireLicenseForPackageAsync",
                can_acquire_package_async
            ),
            method!(
                "XStoreCanAcquireLicenseForPackageResult",
                unavailable_result
            ),
            method!(
                "XStoreQueryConsumableBalanceRemainingAsync",
                consumable_balance_async
            ),
            method!(
                "XStoreQueryConsumableBalanceRemainingResult",
                unavailable_result
            ),
            method!(
                "XStoreReportConsumableFulfillmentAsync",
                report_consumable_async
            ),
            method!(
                "XStoreReportConsumableFulfillmentResult",
                unavailable_result
            ),
            method!("XStoreGetUserCollectionsIdAsync", user_collections_id_async),
            method!(
                "XStoreGetUserCollectionsIdResultSize",
                unavailable_result_out
            ),
            method!("XStoreGetUserCollectionsIdResult", unavailable_result),
            method!("XStoreGetUserPurchaseIdAsync", user_purchase_id_async),
            method!("XStoreGetUserPurchaseIdResultSize", unavailable_result_out),
            method!("XStoreGetUserPurchaseIdResult", unavailable_result),
            method!("XStoreQueryLicenseTokenAsync", license_token_async),
            method!("XStoreQueryLicenseTokenResultSize", unavailable_result_out),
            method!("XStoreQueryLicenseTokenResult", unavailable_result),
            method!("XStoreShowPurchaseUIAsync", show_purchase_async),
            method!("XStoreShowPurchaseUIResult", unavailable_result),
            method!("XStoreShowRateAndReviewUIAsync", show_rate_and_review_async),
            method!("XStoreShowRateAndReviewUIResult", unavailable_result),
            method!("XStoreShowRedeemTokenUIAsync", show_redeem_token_async),
            method!("XStoreShowRedeemTokenUIResult", unavailable_result),
            method!("XStoreDownloadPackageUpdatesAsync", download_updates_async),
            method!("XStoreDownloadPackageUpdatesResult", unavailable_result),
            method!(
                "XStoreDownloadAndInstallPackageUpdatesAsync",
                download_and_install_updates_async
            ),
            method!(
                "XStoreDownloadAndInstallPackageUpdatesResult",
                unavailable_result
            ),
            method!(
                "XStoreDownloadAndInstallPackagesAsync",
                download_and_install_packages_async
            ),
            method!(
                "XStoreDownloadAndInstallPackagesResultCount",
                query_none_count
            ),
            method!("XStoreDownloadAndInstallPackagesResult", query_none_result),
            method!("XStoreQueryPackageIdentifier", query_package_identifier),
            method!(
                "XStoreAcquireLicenseForDurablesAsync",
                acquire_durables_async
            ),
            method!(
                "XStoreAcquireLicenseForDurablesResult",
                unavailable_result_out
            ),
            method!(
                "XStoreShowAssociatedProductsUIAsync",
                show_associated_products_async
            ),
            method!("XStoreShowAssociatedProductsUIResult", unavailable_result),
            method!("XStoreShowProductPageUIAsync", show_product_page_async),
            method!("XStoreShowProductPageUIResult", unavailable_result),
        ],
        failure: E_FAIL,
    }]
}
