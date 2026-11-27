//! The network imports the executable links (WS2_32, WinHTTP, libHttpClient.GDK), answered as a machine with no
//! network answers them. Nothing here opens a connection, and no call waits.
//!
//! Measured in the executable:
//! * `WSAStartup` (RVA 0x05A61140) treats a non-zero result as fatal (`int3`), so it succeeds. Every socket call
//!   after it fails at once: `socket` gives `INVALID_SOCKET` with `WSAENETDOWN`, which the engine's error mapper
//!   (RVA 0x041E45A0) turns into its "network down" code; `WSAEWOULDBLOCK` and `WSAEINPROGRESS` are never used
//!   because `connect` (RVA 0x041E2DD0) reads them as "connection pending".
//! * Name lookups fail with `WSAHOST_NOT_FOUND` and a null result, which the engine's resolvers (RVAs 0x041D3170,
//!   0x041E3BE0) log and answer with false. `htons` and `inet_addr` only convert, so they work.
//! * Both WinHTTP clients (the engine's asynchronous one, RVA 0x041E08A0, and the crash reporter's, RVA 0x055E4100)
//!   check `WinHttpOpen` for null and give up, so it fails with `ERROR_WINHTTP_CANNOT_CONNECT` and no handle ever
//!   exists. Every other WinHTTP call fails the way WinHTTP fails on a handle of the wrong kind.
//! * libHttpClient: the services layer built on it (RVAs 0x0562FA10, 0x0562FC10) reads the outcome of a call
//!   through `HCHttpCallResponseGetNetworkErrorCode` once the `XAsyncBlock` completes, so a performed call
//!   completes with `S_OK` and reports `E_HC_NO_NETWORK` (the public header's value, not measured in the
//!   executable) with `ERROR_WINHTTP_CANNOT_CONNECT`. `HCCleanupAsync` completes its block too, or shutdown
//!   would wait forever.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::SetLastError;

use crate::Shim;

const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
pub const WSAEFAULT: i32 = 10014;
pub const WSAENOTSOCK: i32 = 10038;
pub const WSAENETDOWN: i32 = 10050;
pub const WSAHOST_NOT_FOUND: i32 = 11001;
pub const ERROR_WINHTTP_INCORRECT_HANDLE_TYPE: u32 = 12018;
pub const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
const ERROR_INVALID_HANDLE: u32 = 6;
const INADDR_NONE: u32 = u32::MAX;
const WINSOCK_VERSION: u16 = 0x0202;

pub const S_OK: i32 = 0;
pub const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
pub const E_HC_NO_NETWORK: i32 = 0x8923_5001_u32 as i32;

thread_local! {
    static LAST_SOCKET_ERROR: Cell<i32> = const { Cell::new(0) };
}

fn socket_error(code: i32) {
    LAST_SOCKET_ERROR.with(|e| e.set(code));
}

fn down<T>(value: T) -> T {
    socket_error(WSAENETDOWN);
    value
}

fn win_error(code: u32) {
    // SAFETY: sets the calling thread's last error.
    unsafe { SetLastError(code) };
}

/// `WSADATA` on x64: versions, socket limits, vendor pointer, description (257 bytes) and status (129 bytes).
#[repr(C)]
struct WsaData {
    version: u16,
    high_version: u16,
    max_sockets: u16,
    max_udp: u16,
    vendor: usize,
    description: [u8; 257],
    status: [u8; 129],
}

extern "win64" fn wsa_startup(requested: u16, data: *mut WsaData) -> i32 {
    if data.is_null() {
        return WSAEFAULT;
    }
    let mut out = WsaData {
        version: requested.min(WINSOCK_VERSION),
        high_version: WINSOCK_VERSION,
        max_sockets: 0,
        max_udp: 0,
        vendor: 0,
        description: [0; 257],
        status: [0; 129],
    };
    let text = b"Leonida offline";
    out.description[..text.len()].copy_from_slice(text);
    let text = b"Running";
    out.status[..text.len()].copy_from_slice(text);
    // SAFETY: the program passes a writable WSADATA, checked for null above.
    unsafe { data.write_unaligned(out) };
    0
}

extern "win64" fn wsa_get_last_error() -> i32 {
    LAST_SOCKET_ERROR.with(Cell::get)
}

extern "win64" fn socket() -> usize {
    down(INVALID_SOCKET)
}

extern "win64" fn accept() -> usize {
    down(INVALID_SOCKET)
}

extern "win64" fn failing_call() -> i32 {
    down(SOCKET_ERROR)
}

extern "win64" fn closesocket() -> i32 {
    socket_error(WSAENOTSOCK);
    SOCKET_ERROR
}

extern "win64" fn fd_is_set() -> i32 {
    0
}

extern "win64" fn htons(value: u16) -> u16 {
    value.swap_bytes()
}

/// `inet_addr`'s rules: one to four numbers, decimal, hexadecimal (`0x`) or octal (leading `0`), the last
/// filling the remaining bytes. The result is in network byte order.
pub fn parse_ipv4(text: &str) -> Option<u32> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut numbers = Vec::new();
    for part in &parts {
        let (digits, radix) = match part {
            p if p.starts_with("0x") || p.starts_with("0X") => (&p[2..], 16),
            p if p.len() > 1 && p.starts_with('0') => (&p[1..], 8),
            p => (*p, 10),
        };
        if digits.is_empty() {
            return None;
        }
        numbers.push(u32::from_str_radix(digits, radix).ok()?);
    }
    let (last, leading) = numbers.split_last()?;
    if leading.iter().any(|n| *n > 0xff) {
        return None;
    }
    let room = 8 * (5 - parts.len() as u32);
    if room < 32 && u64::from(*last) >> room != 0 {
        return None;
    }
    let mut host = *last;
    for (i, n) in leading.iter().enumerate() {
        host |= n << (24 - 8 * i as u32);
    }
    Some(host.to_be())
}

extern "win64" fn inet_addr(text: *const c_char) -> u32 {
    if text.is_null() {
        return INADDR_NONE;
    }
    // SAFETY: the program passes a NUL-terminated string.
    let text = unsafe { CStr::from_ptr(text) };
    text.to_str()
        .ok()
        .and_then(parse_ipv4)
        .unwrap_or(INADDR_NONE)
}

extern "win64" fn gethostname(buffer: *mut u8, length: i32) -> i32 {
    const NAME: &[u8] = b"localhost\0";
    if buffer.is_null() || (length as usize) < NAME.len() {
        socket_error(WSAEFAULT);
        return SOCKET_ERROR;
    }
    // SAFETY: the program's buffer holds `length` bytes, at least `NAME.len()` as checked.
    unsafe { std::ptr::copy_nonoverlapping(NAME.as_ptr(), buffer, NAME.len()) };
    0
}

extern "win64" fn getaddrinfo(
    _node: *const c_char,
    _service: *const c_char,
    _hints: *const c_void,
    result: *mut usize,
) -> i32 {
    if !result.is_null() {
        // SAFETY: the program's out pointer.
        unsafe { *result = 0 };
    }
    socket_error(WSAHOST_NOT_FOUND);
    WSAHOST_NOT_FOUND
}

extern "win64" fn freeaddrinfo() {}

extern "win64" fn gethostbyname() -> usize {
    socket_error(WSAHOST_NOT_FOUND);
    0
}

extern "win64" fn http_open() -> usize {
    win_error(ERROR_WINHTTP_CANNOT_CONNECT);
    0
}

extern "win64" fn http_wrong_handle() -> i32 {
    win_error(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE);
    0
}

extern "win64" fn http_close_handle() -> i32 {
    win_error(ERROR_INVALID_HANDLE);
    0
}

extern "win64" fn http_set_status_callback() -> usize {
    win_error(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE);
    usize::MAX
}

extern "win64" fn http_crack_url() -> i32 {
    win_error(ERROR_WINHTTP_CANNOT_CONNECT);
    0
}

/// A libHttpClient call handle. Settings are kept so the services layer can read back what it set and clone a
/// call for a retry; no request ever leaves the process.
struct Call {
    references: AtomicU32,
    method: CString,
    url: CString,
    headers: Vec<(CString, CString)>,
    body: Vec<u8>,
    retry_allowed: u8,
    retry_cache_id: u32,
    retry_delay: u32,
    timeout: u32,
    timeout_window: u32,
}

const NO_TEXT: &[u8; 1] = b"\0";
const NO_NETWORK_TEXT: &[u8] = b"no network: Leonida runs offline\0";

fn call_mut<'a>(call: *mut Call) -> Option<&'a mut Call> {
    // SAFETY: a handle `hc_call_create` made and the program has not closed; null is refused.
    unsafe { call.as_mut() }
}

fn put<T>(out: *mut T, value: T) {
    if !out.is_null() {
        // SAFETY: the program's out pointer.
        unsafe { out.write_unaligned(value) };
    }
}

fn cstring(text: *const c_char) -> CString {
    if text.is_null() {
        return CString::default();
    }
    // SAFETY: the program passes a NUL-terminated string.
    unsafe { CStr::from_ptr(text) }.to_owned()
}

extern "win64" fn hc_ok() -> i32 {
    S_OK
}

extern "win64" fn hc_initialize(_queue: usize) -> i32 {
    S_OK
}

extern "win64" fn hc_trace_message() {}

extern "win64" fn hc_settings_get_trace_level(level: *mut u32) -> i32 {
    put(level, 0);
    S_OK
}

extern "win64" fn hc_call_create(out: *mut *mut Call) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let call = Box::new(Call {
        references: AtomicU32::new(1),
        method: CString::default(),
        url: CString::default(),
        headers: Vec::new(),
        body: Vec::new(),
        retry_allowed: 1,
        retry_cache_id: 0,
        retry_delay: 2,
        timeout: 30,
        timeout_window: 20,
    });
    put(out, Box::into_raw(call));
    S_OK
}

extern "win64" fn hc_call_duplicate(call: *mut Call) -> *mut Call {
    if let Some(call) = call_mut(call) {
        call.references.fetch_add(1, Ordering::AcqRel);
    }
    call
}

extern "win64" fn hc_call_close(call: *mut Call) -> i32 {
    let Some(held) = call_mut(call) else {
        return E_INVALIDARG;
    };
    if held.references.fetch_sub(1, Ordering::AcqRel) == 1 {
        // SAFETY: the last reference to a box `hc_call_create` made.
        drop(unsafe { Box::from_raw(call) });
    }
    S_OK
}

extern "win64" fn hc_call_perform(call: *mut Call, block: *mut c_void) -> i32 {
    if call.is_null() || block.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block, which it keeps alive until the call completes.
    unsafe { crate::loader::complete_async(block, "HCHttpCallPerformAsync", S_OK) }
}

extern "win64" fn hc_cleanup(block: *mut c_void) -> i32 {
    if block.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: as above.
    unsafe { crate::loader::complete_async(block, "HCCleanupAsync", S_OK) }
}

extern "win64" fn hc_set_url(call: *mut Call, method: *const c_char, url: *const c_char) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    call.method = cstring(method);
    call.url = cstring(url);
    S_OK
}

extern "win64" fn hc_get_url(
    call: *mut Call,
    method: *mut *const c_char,
    url: *mut *const c_char,
) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    put(method, call.method.as_ptr());
    put(url, call.url.as_ptr());
    S_OK
}

extern "win64" fn hc_set_header(
    call: *mut Call,
    name: *const c_char,
    value: *const c_char,
    _allow_tracing: u8,
) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    let (name, value) = (cstring(name), cstring(value));
    match call.headers.iter_mut().find(|(n, _)| *n == name) {
        Some(entry) => entry.1 = value,
        None => call.headers.push((name, value)),
    }
    S_OK
}

extern "win64" fn hc_get_num_headers(call: *mut Call, count: *mut u32) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    put(count, call.headers.len() as u32);
    S_OK
}

extern "win64" fn hc_get_header_at(
    call: *mut Call,
    index: u32,
    name: *mut *const c_char,
    value: *mut *const c_char,
) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    let Some((n, v)) = call.headers.get(index as usize) else {
        return E_INVALIDARG;
    };
    put(name, n.as_ptr());
    put(value, v.as_ptr());
    S_OK
}

extern "win64" fn hc_set_body_bytes(call: *mut Call, body: *const u8, size: u32) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    call.body = if body.is_null() {
        Vec::new()
    } else {
        // SAFETY: the program's buffer holds `size` bytes.
        unsafe { std::slice::from_raw_parts(body, size as usize) }.to_vec()
    };
    S_OK
}

extern "win64" fn hc_set_body_string(call: *mut Call, body: *const c_char) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    call.body = cstring(body).into_bytes();
    S_OK
}

extern "win64" fn hc_get_body_bytes(call: *mut Call, body: *mut *const u8, size: *mut u32) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    put(body, call.body.as_ptr());
    put(size, call.body.len() as u32);
    S_OK
}

extern "win64" fn hc_set_retry_allowed(call: *mut Call, allowed: u8) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    call.retry_allowed = u8::from(allowed != 0);
    S_OK
}

extern "win64" fn hc_get_retry_allowed(call: *mut Call, allowed: *mut u8) -> i32 {
    let Some(call) = call_mut(call) else {
        return E_INVALIDARG;
    };
    put(allowed, call.retry_allowed);
    S_OK
}

macro_rules! number_setting {
    ($set:ident, $get:ident, $field:ident) => {
        extern "win64" fn $set(call: *mut Call, value: u32) -> i32 {
            let Some(call) = call_mut(call) else {
                return E_INVALIDARG;
            };
            call.$field = value;
            S_OK
        }

        extern "win64" fn $get(call: *mut Call, value: *mut u32) -> i32 {
            let Some(call) = call_mut(call) else {
                return E_INVALIDARG;
            };
            put(value, call.$field);
            S_OK
        }
    };
}

number_setting!(hc_set_retry_cache_id, hc_get_retry_cache_id, retry_cache_id);
number_setting!(hc_set_retry_delay, hc_get_retry_delay, retry_delay);
number_setting!(hc_set_timeout, hc_get_timeout, timeout);
number_setting!(hc_set_timeout_window, hc_get_timeout_window, timeout_window);

extern "win64" fn hc_response_network_error(
    call: *mut Call,
    error: *mut i32,
    platform_error: *mut u32,
) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(error, E_HC_NO_NETWORK);
    put(platform_error, ERROR_WINHTTP_CANNOT_CONNECT);
    S_OK
}

extern "win64" fn hc_response_network_message(call: *mut Call, text: *mut *const c_char) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(text, NO_NETWORK_TEXT.as_ptr() as *const c_char);
    S_OK
}

extern "win64" fn hc_response_status(call: *mut Call, status: *mut u32) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(status, 0);
    S_OK
}

extern "win64" fn hc_response_string(call: *mut Call, text: *mut *const c_char) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(text, NO_TEXT.as_ptr() as *const c_char);
    S_OK
}

extern "win64" fn hc_response_header(
    call: *mut Call,
    _name: *const c_char,
    value: *mut *const c_char,
) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(value, std::ptr::null());
    S_OK
}

extern "win64" fn hc_response_num_headers(call: *mut Call, count: *mut u32) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(count, 0);
    S_OK
}

extern "win64" fn hc_response_header_at() -> i32 {
    E_INVALIDARG
}

extern "win64" fn hc_response_body_size(call: *mut Call, size: *mut usize) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(size, 0);
    S_OK
}

extern "win64" fn hc_response_body(
    call: *mut Call,
    _capacity: usize,
    _buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    if call.is_null() {
        return E_INVALIDARG;
    }
    put(used, 0);
    S_OK
}

fn entry(dll: &'static str, name: &'static str, address: usize) -> Shim {
    Shim { dll, name, address }
}

pub fn shims() -> Vec<Shim> {
    const WS2: &str = "WS2_32.dll";
    const HTTP: &str = "WINHTTP.dll";
    const HC: &str = "libHttpClient.GDK.dll";
    macro_rules! table {
        ($dll:expr, $($name:literal => $function:expr),* $(,)?) => {
            [$(entry($dll, $name, $function as *const () as usize)),*]
        };
    }
    let mut shims = Vec::new();
    shims.extend(table!(WS2,
        "WSAStartup" => wsa_startup as extern "win64" fn(u16, *mut WsaData) -> i32,
        "WSAGetLastError" => wsa_get_last_error as extern "win64" fn() -> i32,
        "socket" => socket as extern "win64" fn() -> usize,
        "accept" => accept as extern "win64" fn() -> usize,
        "closesocket" => closesocket as extern "win64" fn() -> i32,
        "bind" => failing_call as extern "win64" fn() -> i32,
        "connect" => failing_call as extern "win64" fn() -> i32,
        "listen" => failing_call as extern "win64" fn() -> i32,
        "shutdown" => failing_call as extern "win64" fn() -> i32,
        "ioctlsocket" => failing_call as extern "win64" fn() -> i32,
        "setsockopt" => failing_call as extern "win64" fn() -> i32,
        "getsockopt" => failing_call as extern "win64" fn() -> i32,
        "getsockname" => failing_call as extern "win64" fn() -> i32,
        "recv" => failing_call as extern "win64" fn() -> i32,
        "recvfrom" => failing_call as extern "win64" fn() -> i32,
        "send" => failing_call as extern "win64" fn() -> i32,
        "sendto" => failing_call as extern "win64" fn() -> i32,
        "select" => failing_call as extern "win64" fn() -> i32,
        "__WSAFDIsSet" => fd_is_set as extern "win64" fn() -> i32,
        "htons" => htons as extern "win64" fn(u16) -> u16,
        "inet_addr" => inet_addr as extern "win64" fn(*const c_char) -> u32,
        "gethostname" => gethostname as extern "win64" fn(*mut u8, i32) -> i32,
        "getaddrinfo" => getaddrinfo as extern "win64" fn(*const c_char, *const c_char, *const c_void, *mut usize) -> i32,
        "freeaddrinfo" => freeaddrinfo as extern "win64" fn(),
        "gethostbyname" => gethostbyname as extern "win64" fn() -> usize,
    ));
    shims.extend(table!(HTTP,
        "WinHttpOpen" => http_open as extern "win64" fn() -> usize,
        "WinHttpConnect" => http_open as extern "win64" fn() -> usize,
        "WinHttpOpenRequest" => http_open as extern "win64" fn() -> usize,
        "WinHttpSetStatusCallback" => http_set_status_callback as extern "win64" fn() -> usize,
        "WinHttpCloseHandle" => http_close_handle as extern "win64" fn() -> i32,
        "WinHttpCrackUrl" => http_crack_url as extern "win64" fn() -> i32,
        "WinHttpAddRequestHeaders" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpQueryDataAvailable" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpQueryHeaders" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpReadData" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpReceiveResponse" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpSendRequest" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpSetOption" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpSetTimeouts" => http_wrong_handle as extern "win64" fn() -> i32,
        "WinHttpWriteData" => http_wrong_handle as extern "win64" fn() -> i32,
    ));
    shims.extend(table!(HC,
        "HCInitialize" => hc_initialize as extern "win64" fn(usize) -> i32,
        "HCCleanupAsync" => hc_cleanup as extern "win64" fn(*mut c_void) -> i32,
        "HCSettingsGetTraceLevel" => hc_settings_get_trace_level as extern "win64" fn(*mut u32) -> i32,
        "HCTraceImplMessage" => hc_trace_message as extern "win64" fn(),
        "HCTraceSetEtwEnabled" => hc_ok as extern "win64" fn() -> i32,
        "HCHttpCallSetTracing" => hc_ok as extern "win64" fn() -> i32,
        "HCHttpCallCreate" => hc_call_create as extern "win64" fn(*mut *mut Call) -> i32,
        "HCHttpCallDuplicateHandle" => hc_call_duplicate as extern "win64" fn(*mut Call) -> *mut Call,
        "HCHttpCallCloseHandle" => hc_call_close as extern "win64" fn(*mut Call) -> i32,
        "HCHttpCallPerformAsync" => hc_call_perform as extern "win64" fn(*mut Call, *mut c_void) -> i32,
        "HCHttpCallRequestSetUrl" => hc_set_url as extern "win64" fn(*mut Call, *const c_char, *const c_char) -> i32,
        "HCHttpCallRequestGetUrl" => hc_get_url as extern "win64" fn(*mut Call, *mut *const c_char, *mut *const c_char) -> i32,
        "HCHttpCallRequestSetHeader" => hc_set_header as extern "win64" fn(*mut Call, *const c_char, *const c_char, u8) -> i32,
        "HCHttpCallRequestGetNumHeaders" => hc_get_num_headers as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallRequestGetHeaderAtIndex" => hc_get_header_at as extern "win64" fn(*mut Call, u32, *mut *const c_char, *mut *const c_char) -> i32,
        "HCHttpCallRequestSetRequestBodyBytes" => hc_set_body_bytes as extern "win64" fn(*mut Call, *const u8, u32) -> i32,
        "HCHttpCallRequestSetRequestBodyString" => hc_set_body_string as extern "win64" fn(*mut Call, *const c_char) -> i32,
        "HCHttpCallRequestGetRequestBodyBytes" => hc_get_body_bytes as extern "win64" fn(*mut Call, *mut *const u8, *mut u32) -> i32,
        "HCHttpCallRequestSetRetryAllowed" => hc_set_retry_allowed as extern "win64" fn(*mut Call, u8) -> i32,
        "HCHttpCallRequestGetRetryAllowed" => hc_get_retry_allowed as extern "win64" fn(*mut Call, *mut u8) -> i32,
        "HCHttpCallRequestSetRetryCacheId" => hc_set_retry_cache_id as extern "win64" fn(*mut Call, u32) -> i32,
        "HCHttpCallRequestGetRetryCacheId" => hc_get_retry_cache_id as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallRequestSetRetryDelay" => hc_set_retry_delay as extern "win64" fn(*mut Call, u32) -> i32,
        "HCHttpCallRequestGetRetryDelay" => hc_get_retry_delay as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallRequestSetTimeout" => hc_set_timeout as extern "win64" fn(*mut Call, u32) -> i32,
        "HCHttpCallRequestGetTimeout" => hc_get_timeout as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallRequestSetTimeoutWindow" => hc_set_timeout_window as extern "win64" fn(*mut Call, u32) -> i32,
        "HCHttpCallRequestGetTimeoutWindow" => hc_get_timeout_window as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallResponseGetNetworkErrorCode" => hc_response_network_error as extern "win64" fn(*mut Call, *mut i32, *mut u32) -> i32,
        "HCHttpCallResponseGetPlatformNetworkErrorMessage" => hc_response_network_message as extern "win64" fn(*mut Call, *mut *const c_char) -> i32,
        "HCHttpCallResponseGetStatusCode" => hc_response_status as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallResponseGetResponseString" => hc_response_string as extern "win64" fn(*mut Call, *mut *const c_char) -> i32,
        "HCHttpCallResponseGetHeader" => hc_response_header as extern "win64" fn(*mut Call, *const c_char, *mut *const c_char) -> i32,
        "HCHttpCallResponseGetNumHeaders" => hc_response_num_headers as extern "win64" fn(*mut Call, *mut u32) -> i32,
        "HCHttpCallResponseGetHeaderAtIndex" => hc_response_header_at as extern "win64" fn() -> i32,
        "HCHttpCallResponseGetResponseBodyBytesSize" => hc_response_body_size as extern "win64" fn(*mut Call, *mut usize) -> i32,
        "HCHttpCallResponseGetResponseBodyBytes" => hc_response_body as extern "win64" fn(*mut Call, usize, *mut u8, *mut usize) -> i32,
    ));
    shims
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses_as_inet_addr_does() {
        assert_eq!(parse_ipv4("127.0.0.1"), Some(0x0100_007F));
        assert_eq!(parse_ipv4("10.1"), Some(0x0100_000A_u32));
        assert_eq!(parse_ipv4("0x7f.0.0.1"), Some(0x0100_007F));
        assert_eq!(parse_ipv4("010.0.0.1"), Some(0x0100_0008));
        assert_eq!(parse_ipv4("256.1.1.1"), None);
        assert_eq!(parse_ipv4("localhost"), None);
        assert_eq!(parse_ipv4("1.2.3.4.5"), None);
        assert_eq!(parse_ipv4(""), None);
    }
}
