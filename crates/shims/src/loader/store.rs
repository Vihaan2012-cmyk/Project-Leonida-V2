//! `XStoreImpl`: what the Microsoft Store can answer with no network: the game license is active and not a
//! trial, there are no add-on licenses and no package updates, and license-change registrations hand out
//! tokens whose callbacks never run. Purchases, product queries, consumables and every store interface fail
//! (and are noted) by the class's default.
//!
//! Measured on the executable: `XStoreGameLicense` is 104 bytes, read as a C string SKU id at +0x00, three
//! booleans at +0x13..+0x15, a C string trial id at +0x1C and the expiration time at +0x60. The expiration
//! and the SKU text are not measured; the license never expires.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::com::{ClassImpl, method};
use super::tasks::{self, AsyncBlock};
use super::{E_FAIL, E_INVALIDARG, E_POINTER, S_OK};

type This = *mut c_void;

const GAME_LICENSE_BYTES: usize = 104;
const SKU: &[u8] = b"0010";
const NEVER_EXPIRES: i64 = 253_402_300_799;

static CONTEXTS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static TOKENS: AtomicU64 = AtomicU64::new(1 << 48);

fn contexts<T>(f: impl FnOnce(&mut HashSet<usize>) -> T) -> T {
    f(CONTEXTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new))
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
        ],
        failure: E_FAIL,
    }]
}
