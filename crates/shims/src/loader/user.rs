//! `XUserImpl`: one local user, always signed in, with the local privileges and none that needs the Xbox network,
//! and no way to prove who they are.
//!
//! The user has a fixed non-zero user id and local id, is an adult and not a guest. Its gamertag is
//! `LEONIDA_GAMERTAG` (up to 15 printable ASCII characters) or `Player`. Every handle is its own allocation,
//! closed once; handles are looked up in a live set and never read when unknown. Token, signature, gamer
//! picture and issue-resolution requests fail before they start, so nothing waits on the Xbox network.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::com::{ClassImpl, method, write_c_str};
use super::tasks::{self, AsyncBlock};
use super::{E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_INSUFFICIENT_BUFFER, S_OK};

type This = *mut c_void;

const OFFLINE_XUID: u64 = 0x0009_0000_0000_0001;
const OFFLINE_LOCAL_ID: u64 = 1;
const E_GAMEUSER_USER_NOT_FOUND: i32 = 0x8924_5104_u32 as i32;
const E_GAMEUSER_DEFERRAL_NOT_AVAILABLE: i32 = 0x8924_5103_u32 as i32;
const E_GAMEUSER_FAILED_TO_RESOLVE: i32 = 0x8924_5107_u32 as i32;
const E_GAMEUSER_FAILED_TO_GET_TOKEN: i32 = 0x8924_5111_u32 as i32;

static HANDLES: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static TOKENS: AtomicU64 = AtomicU64::new(1 << 40);

/// A new handle on the one user.
pub(super) fn new_handle() -> usize {
    let handle = Box::into_raw(Box::new(0u64)) as usize;
    HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(handle);
    handle
}

pub(super) fn valid(handle: usize) -> bool {
    HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .contains(&handle)
}

fn gamertag() -> String {
    let tag = super::setting("LEONIDA_GAMERTAG", "Player");
    if tag.len() <= 15 && tag.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        tag
    } else {
        "Player".to_string()
    }
}

unsafe extern "win64" fn duplicate_handle(_: This, user: usize, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    if !valid(user) {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { *out = new_handle() };
    S_OK
}

unsafe extern "win64" fn close_handle(_: This, user: usize) {
    let known = HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .remove(&user);
    if known {
        // SAFETY: the box `new_handle` made, closed once.
        drop(unsafe { Box::from_raw(user as *mut u64) });
    }
}

/// Every handle names the one user.
unsafe extern "win64" fn compare(_: This, _first: usize, _second: usize) -> i32 {
    0
}

unsafe extern "win64" fn max_users(_: This, out: *mut u32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 1 };
    S_OK
}

unsafe extern "win64" fn add_async(_: This, options: u32, block: *mut AsyncBlock) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if options & 0x3 == 0x3 {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe { tasks::begin_one_shot(block, "XUserAddAsync", || (S_OK, vec![1])) }
}

unsafe extern "win64" fn add_result(_: This, block: *mut AsyncBlock, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    let mut token = [0u8; 1];
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    // SAFETY: the program's block and a one-byte result.
    let result = unsafe {
        tasks::one_shot_result(block, "XUserAddAsync", 1, token.as_mut_ptr() as *mut c_void)
    };
    if result >= 0 {
        // SAFETY: writable, checked.
        unsafe { *out = new_handle() };
    }
    result
}

unsafe extern "win64" fn add_by_id_async(_: This, id: u64, block: *mut AsyncBlock) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XUserAddByIdWithUiAsync", move || {
            if id == OFFLINE_XUID {
                (S_OK, vec![1])
            } else {
                (E_GAMEUSER_USER_NOT_FOUND, Vec::new())
            }
        })
    }
}

unsafe extern "win64" fn add_by_id_result(_: This, block: *mut AsyncBlock, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    let mut token = [0u8; 1];
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    // SAFETY: the program's block and a one-byte result.
    let result = unsafe {
        tasks::one_shot_result(
            block,
            "XUserAddByIdWithUiAsync",
            1,
            token.as_mut_ptr() as *mut c_void,
        )
    };
    if result >= 0 {
        // SAFETY: writable, checked.
        unsafe { *out = new_handle() };
    }
    result
}

unsafe fn put<T>(user: usize, out: *mut T, value: T) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    if !valid(user) {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { out.write_unaligned(value) };
    S_OK
}

unsafe extern "win64" fn get_local_id(_: This, user: usize, out: *mut u64) -> i32 {
    // SAFETY: the program's pointer.
    unsafe { put(user, out, OFFLINE_LOCAL_ID) }
}

unsafe extern "win64" fn find_by_local_id(_: This, id: u64, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    if id != OFFLINE_LOCAL_ID {
        // SAFETY: writable, checked.
        unsafe { *out = 0 };
        return E_GAMEUSER_USER_NOT_FOUND;
    }
    // SAFETY: writable, checked.
    unsafe { *out = new_handle() };
    S_OK
}

unsafe extern "win64" fn get_id(_: This, user: usize, out: *mut u64) -> i32 {
    // SAFETY: the program's pointer.
    unsafe { put(user, out, OFFLINE_XUID) }
}

unsafe extern "win64" fn find_by_id(_: This, id: u64, out: *mut usize) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    if id != OFFLINE_XUID {
        // SAFETY: writable, checked.
        unsafe { *out = 0 };
        return E_GAMEUSER_USER_NOT_FOUND;
    }
    // SAFETY: writable, checked.
    unsafe { *out = new_handle() };
    S_OK
}

unsafe extern "win64" fn get_is_guest(_: This, user: usize, out: *mut u8) -> i32 {
    // SAFETY: the program's pointer.
    unsafe { put(user, out, 0u8) }
}

/// Signed in.
unsafe extern "win64" fn get_state(_: This, user: usize, out: *mut u32) -> i32 {
    // SAFETY: the program's pointer.
    unsafe { put(user, out, 0u32) }
}

/// Adult.
unsafe extern "win64" fn get_age_group(_: This, user: usize, out: *mut u32) -> i32 {
    // SAFETY: the program's pointer.
    unsafe { put(user, out, 3u32) }
}

/// The privileges the Xbox network grants: CrossPlay, Clubs, Sessions, Broadcast, MultiplayerParties,
/// CloudManageSession, CloudJoinSession, SocialNetworkSharing, UserGeneratedContent, Communications, Multiplayer
/// and AddFriends. The title asks for fifteen (table at RVA 0x654fc34); the other three, ManageProfilePrivacy,
/// GameDvr and CloudSavedGames, concern what stays on the machine.
const NETWORK_PRIVILEGES: [u32; 12] = [185, 188, 189, 190, 203, 207, 208, 220, 247, 252, 254, 255];

/// `XUserPrivilegeDenyReason::Unknown`: the reason a console gives when it cannot ask Xbox services.
const DENIED_UNKNOWN: u32 = 4;

/// A user with no Xbox network holds no network privilege and is not told why beyond "unknown"; the others are
/// held. Nothing here grants a licence or an entitlement.
unsafe extern "win64" fn check_privilege(
    _: This,
    user: usize,
    _options: u32,
    privilege: u32,
    has: *mut u8,
    reason: *mut u32,
) -> i32 {
    let allowed = !NETWORK_PRIVILEGES.contains(&privilege);
    if !reason.is_null() {
        // SAFETY: writable, checked.
        unsafe { *reason = if allowed { 0 } else { DENIED_UNKNOWN } };
    }
    // SAFETY: the program's pointer.
    unsafe { put(user, has, u8::from(allowed)) }
}

unsafe extern "win64" fn resolve_privilege_async(
    _: This,
    user: usize,
    _options: u32,
    _privilege: u32,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !valid(user) {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XUserResolvePrivilegeWithUiAsync", || {
            (S_OK, Vec::new())
        })
    }
}

unsafe extern "win64" fn resolve_privilege_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe {
        tasks::one_shot_result(
            block,
            "XUserResolvePrivilegeWithUiAsync",
            0,
            std::ptr::null_mut(),
        )
    }
}

/// The user never changes, so a registration is a token and no event ever comes.
unsafe extern "win64" fn register_change(
    _: This,
    _queue: usize,
    _context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if callback == 0 || token.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) };
    S_OK
}

unsafe extern "win64" fn unregister_change(_: This, _token: u64, _wait: u8) -> u8 {
    1
}

unsafe extern "win64" fn sign_out_deferral(_: This, out: *mut usize) -> i32 {
    if !out.is_null() {
        // SAFETY: writable, checked.
        unsafe { *out = 0 };
    }
    E_GAMEUSER_DEFERRAL_NOT_AVAILABLE
}

unsafe extern "win64" fn close_sign_out_deferral(_: This, _handle: usize) {}

unsafe extern "win64" fn is_store_user(_: This, _user: usize) -> u8 {
    1
}

unsafe extern "win64" fn is_sign_out_present(_: This) -> u8 {
    0
}

/// Token, signature and Microsoft account token requests fail before they start.
unsafe extern "win64" fn token_failure(_: This) -> i32 {
    E_GAMEUSER_FAILED_TO_GET_TOKEN
}

unsafe extern "win64" fn resolve_failure(_: This) -> i32 {
    E_GAMEUSER_FAILED_TO_RESOLVE
}

/// `XUserGetGamertag(user, component, size, buffer, used)`: classic, modern and unique-modern are the same text,
/// and the modern suffix is empty.
unsafe extern "win64" fn get_gamertag(
    _: This,
    user: usize,
    component: u32,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    if !valid(user) {
        return E_INVALIDARG;
    }
    let text = match component {
        0 | 1 | 3 => gamertag(),
        2 => String::new(),
        _ => return E_INVALIDARG,
    };
    // SAFETY: `size` bytes are writable.
    if !unsafe { write_c_str(buffer, size, &text) } {
        return HR_INSUFFICIENT_BUFFER;
    }
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = text.len() + 1 };
    }
    S_OK
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![
        ClassImpl {
            class: "XUserImpl",
            root: "IXUserImpl",
            methods: vec![
                method!("XUserDuplicateHandle", duplicate_handle),
                method!("XUserCloseHandle", close_handle),
                method!("XUserCompare", compare),
                method!("XUserGetMaxUsers", max_users),
                method!("XUserAddAsync", add_async),
                method!("XUserAddResult", add_result),
                method!("XUserGetLocalId", get_local_id),
                method!("XUserFindUserByLocalId", find_by_local_id),
                method!("XUserGetId", get_id),
                method!("XUserFindUserById", find_by_id),
                method!("XUserGetIsGuest", get_is_guest),
                method!("XUserGetState", get_state),
                method!("XUserGetGamerPictureAsync", token_failure),
                method!("XUserGetGamerPictureResultSize", token_failure),
                method!("XUserGetGamerPictureResult", token_failure),
                method!("XUserGetAgeGroup", get_age_group),
                method!("XUserCheckPrivilege", check_privilege),
                method!("XUserResolvePrivilegeWithUiAsync", resolve_privilege_async),
                method!(
                    "XUserResolvePrivilegeWithUiResult",
                    resolve_privilege_result
                ),
                method!("XUserGetTokenAndSignatureAsync", token_failure),
                method!("XUserGetTokenAndSignatureResultSize", token_failure),
                method!("XUserGetTokenAndSignatureResult", token_failure),
                method!("XUserGetTokenAndSignatureUtf16Async", token_failure),
                method!("XUserGetTokenAndSignatureUtf16ResultSize", token_failure),
                method!("XUserGetTokenAndSignatureUtf16Result", token_failure),
                method!("XUserResolveIssueWithUiAsync", resolve_failure),
                method!("XUserResolveIssueWithUiResult", resolve_failure),
                method!("XUserResolveIssueWithUiUtf16Async", resolve_failure),
                method!("XUserResolveIssueWithUiUtf16Result", resolve_failure),
                method!("XUserRegisterForChangeEvent", register_change),
                method!("XUserUnregisterForChangeEvent", unregister_change),
                method!("XUserGetSignOutDeferral", sign_out_deferral),
                method!("XUserCloseSignOutDeferralHandle", close_sign_out_deferral),
                method!("XUserAddByIdWithUiAsync", add_by_id_async),
                method!("XUserAddByIdWithUiResult", add_by_id_result),
                method!("XUserGetMsaTokenSilentlyAsync", token_failure),
                method!("XUserGetMsaTokenSilentlyResult", token_failure),
                method!("XUserGetMsaTokenSilentlyResultSize", token_failure),
                method!("XUserIsStoreUser", is_store_user),
                method!("XUserIsSignOutPresent", is_sign_out_present),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XUserImpl",
            root: "IXUserGamertagImpl",
            methods: vec![method!("XUserGetGamertag", get_gamertag)],
            failure: E_FAIL,
        },
    ]
}
