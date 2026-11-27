//! The network imports as the executable calls them: every call fails at once, and nothing waits.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::ffi::{CStr, c_char, c_void};

use shims::network::{
    E_HC_NO_NETWORK, ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_INCORRECT_HANDLE_TYPE,
    WSAENETDOWN, WSAHOST_NOT_FOUND,
};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};

const WS2: &str = "WS2_32.dll";
const HTTP: &str = "WINHTTP.dll";
const HC: &str = "libHttpClient.GDK.dll";

const CLASSES: &str = include_str!("../src/loader/classes.txt");

const WS2_IMPORTS: &[&str] = &[
    "WSAGetLastError",
    "WSAStartup",
    "__WSAFDIsSet",
    "accept",
    "bind",
    "closesocket",
    "connect",
    "freeaddrinfo",
    "getaddrinfo",
    "gethostbyname",
    "gethostname",
    "getsockname",
    "getsockopt",
    "htons",
    "inet_addr",
    "ioctlsocket",
    "listen",
    "recv",
    "recvfrom",
    "select",
    "send",
    "sendto",
    "setsockopt",
    "shutdown",
    "socket",
];

const HTTP_IMPORTS: &[&str] = &[
    "WinHttpAddRequestHeaders",
    "WinHttpCloseHandle",
    "WinHttpConnect",
    "WinHttpCrackUrl",
    "WinHttpOpen",
    "WinHttpOpenRequest",
    "WinHttpQueryDataAvailable",
    "WinHttpQueryHeaders",
    "WinHttpReadData",
    "WinHttpReceiveResponse",
    "WinHttpSendRequest",
    "WinHttpSetOption",
    "WinHttpSetStatusCallback",
    "WinHttpSetTimeouts",
    "WinHttpWriteData",
];

const HC_IMPORTS: &[&str] = &[
    "HCCleanupAsync",
    "HCHttpCallCloseHandle",
    "HCHttpCallCreate",
    "HCHttpCallDuplicateHandle",
    "HCHttpCallPerformAsync",
    "HCHttpCallRequestGetHeaderAtIndex",
    "HCHttpCallRequestGetNumHeaders",
    "HCHttpCallRequestGetRequestBodyBytes",
    "HCHttpCallRequestGetRetryAllowed",
    "HCHttpCallRequestGetRetryCacheId",
    "HCHttpCallRequestGetRetryDelay",
    "HCHttpCallRequestGetTimeout",
    "HCHttpCallRequestGetTimeoutWindow",
    "HCHttpCallRequestGetUrl",
    "HCHttpCallRequestSetHeader",
    "HCHttpCallRequestSetRequestBodyBytes",
    "HCHttpCallRequestSetRequestBodyString",
    "HCHttpCallRequestSetRetryAllowed",
    "HCHttpCallRequestSetRetryCacheId",
    "HCHttpCallRequestSetRetryDelay",
    "HCHttpCallRequestSetTimeout",
    "HCHttpCallRequestSetTimeoutWindow",
    "HCHttpCallRequestSetUrl",
    "HCHttpCallResponseGetHeader",
    "HCHttpCallResponseGetHeaderAtIndex",
    "HCHttpCallResponseGetNetworkErrorCode",
    "HCHttpCallResponseGetNumHeaders",
    "HCHttpCallResponseGetPlatformNetworkErrorMessage",
    "HCHttpCallResponseGetResponseBodyBytes",
    "HCHttpCallResponseGetResponseBodyBytesSize",
    "HCHttpCallResponseGetResponseString",
    "HCHttpCallResponseGetStatusCode",
    "HCHttpCallSetTracing",
    "HCInitialize",
    "HCSettingsGetTraceLevel",
    "HCTraceImplMessage",
    "HCTraceSetEtwEnabled",
];

fn address(dll: &str, name: &str) -> usize {
    shims::builtin()
        .into_iter()
        .find(|s| s.dll.eq_ignore_ascii_case(dll) && s.name == name)
        .unwrap_or_else(|| panic!("{dll}!{name} registered"))
        .address
}

fn function<T: Copy>(dll: &str, name: &str) -> T {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    let address = address(dll, name);
    // SAFETY: a function pointer type of the size of an address.
    unsafe { std::mem::transmute_copy(&address) }
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

#[repr(C)]
#[derive(Default)]
struct Block {
    queue: usize,
    context: usize,
    callback: usize,
    internal: [u8; 32],
}

/// `XAsyncGetStatus(block, wait)`: slot 3 of `XThreadingImpl`.
fn async_status(block: &mut Block) -> i32 {
    let mut object = 0usize;
    let class = class_id("XThreadingImpl");
    assert_eq!(shims::loader::query_api(&class, &class, &mut object), 0);
    // SAFETY: a COM object's first word is its vtable; slot 3 has this shape.
    unsafe {
        let vtable = *(object as *const *const usize);
        let status: unsafe extern "win64" fn(usize, *mut Block, u8) -> i32 =
            std::mem::transmute_copy(&*vtable.add(3));
        status(0, block, 1)
    }
}

#[test]
fn every_network_import_the_executable_links_has_an_answer() {
    for (dll, names) in [(WS2, WS2_IMPORTS), (HTTP, HTTP_IMPORTS), (HC, HC_IMPORTS)] {
        for name in names {
            address(dll, name);
            assert!(
                offline::import(dll, Some(name)).is_some(),
                "{dll}!{name} stays on the offline policy"
            );
        }
    }
}

#[test]
fn winsock_starts_and_every_connection_fails_at_once() {
    type Startup = unsafe extern "win64" fn(u16, *mut [u8; 408]) -> i32;
    type Last = unsafe extern "win64" fn() -> i32;
    type Socket = unsafe extern "win64" fn(i32, i32, i32) -> usize;
    type Call = unsafe extern "win64" fn(usize, usize, i32) -> i32;
    let startup: Startup = function(WS2, "WSAStartup");
    let last: Last = function(WS2, "WSAGetLastError");
    let socket: Socket = function(WS2, "socket");
    // SAFETY: the documented shapes.
    unsafe {
        let mut data = [0xAAu8; 408];
        assert_eq!(startup(2, &mut data), 0, "a failing WSAStartup is fatal");
        assert_eq!(u16::from_le_bytes([data[0], data[1]]), 2);
        assert_eq!(u16::from_le_bytes([data[2], data[3]]), 0x0202);

        assert_eq!(socket(2, 1, 6), usize::MAX);
        assert_eq!(last(), WSAENETDOWN);
        assert_eq!(socket(2, 2, 17), usize::MAX);

        for name in [
            "connect",
            "bind",
            "listen",
            "shutdown",
            "ioctlsocket",
            "setsockopt",
            "getsockopt",
            "getsockname",
            "recv",
            "recvfrom",
            "send",
            "sendto",
            "select",
        ] {
            let call: Call = function(WS2, name);
            assert_eq!(call(0, 0, 0), -1, "{name}");
            assert_eq!(last(), WSAENETDOWN, "{name}");
        }
        let accept: Socket = function(WS2, "accept");
        assert_eq!(accept(0, 0, 0), usize::MAX);
        let is_set: unsafe extern "win64" fn(usize, usize) -> i32 = function(WS2, "__WSAFDIsSet");
        assert_eq!(is_set(0, 0), 0);
    }
}

#[test]
fn the_winsock_error_is_kept_per_thread() {
    let socket: unsafe extern "win64" fn(i32, i32, i32) -> usize = function(WS2, "socket");
    let last: unsafe extern "win64" fn() -> i32 = function(WS2, "WSAGetLastError");
    // SAFETY: the documented shapes.
    unsafe {
        socket(2, 1, 6);
        let other = std::thread::spawn(move || last()).join().unwrap();
        assert_eq!(other, 0);
        assert_eq!(last(), WSAENETDOWN);
    }
}

#[test]
fn name_lookups_fail_without_a_result_and_conversions_still_work() {
    type Lookup = unsafe extern "win64" fn(*const u8, *const u8, *const c_void, *mut usize) -> i32;
    let getaddrinfo: Lookup = function(WS2, "getaddrinfo");
    let freeaddrinfo: unsafe extern "win64" fn(usize) = function(WS2, "freeaddrinfo");
    let gethostbyname: unsafe extern "win64" fn(*const u8) -> usize =
        function(WS2, "gethostbyname");
    let last: unsafe extern "win64" fn() -> i32 = function(WS2, "WSAGetLastError");
    let gethostname: unsafe extern "win64" fn(*mut u8, i32) -> i32 = function(WS2, "gethostname");
    let htons: unsafe extern "win64" fn(u16) -> u16 = function(WS2, "htons");
    let inet_addr: unsafe extern "win64" fn(*const u8) -> u32 = function(WS2, "inet_addr");
    // SAFETY: the documented shapes.
    unsafe {
        let mut result = 0xDEADusize;
        let host = cstr("example.com");
        assert_eq!(
            getaddrinfo(
                host.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                &mut result
            ),
            WSAHOST_NOT_FOUND
        );
        assert_eq!(result, 0);
        freeaddrinfo(result);
        assert_eq!(gethostbyname(host.as_ptr()), 0);
        assert_eq!(last(), WSAHOST_NOT_FOUND);

        let mut name = [0u8; 32];
        assert_eq!(gethostname(name.as_mut_ptr(), 32), 0);
        assert_eq!(
            CStr::from_ptr(name.as_ptr() as *const c_char).to_bytes(),
            b"localhost"
        );
        assert_eq!(gethostname(name.as_mut_ptr(), 4), -1);

        assert_eq!(htons(0x1234), 0x3412);
        assert_eq!(inet_addr(cstr("127.0.0.1").as_ptr()), 0x0100_007F);
        assert_eq!(inet_addr(cstr("example.com").as_ptr()), u32::MAX);
    }
}

#[test]
fn winhttp_gives_no_handle_and_a_cannot_connect_error() {
    let open: unsafe extern "win64" fn(*const u16, u32, usize, usize, u32) -> usize =
        function(HTTP, "WinHttpOpen");
    let connect: unsafe extern "win64" fn(usize, *const u16, u16, u32) -> usize =
        function(HTTP, "WinHttpConnect");
    let send: unsafe extern "win64" fn(usize, usize, u32, usize, u32, u32, usize) -> i32 =
        function(HTTP, "WinHttpSendRequest");
    let callback: unsafe extern "win64" fn(usize, usize, u32, usize) -> usize =
        function(HTTP, "WinHttpSetStatusCallback");
    let crack: unsafe extern "win64" fn(*const u16, u32, u32, usize) -> i32 =
        function(HTTP, "WinHttpCrackUrl");
    // SAFETY: the documented shapes.
    unsafe {
        SetLastError(0);
        assert_eq!(open(std::ptr::null(), 4, 0, 0, 0x1000_0000), 0);
        assert_eq!(GetLastError(), ERROR_WINHTTP_CANNOT_CONNECT);
        SetLastError(0);
        assert_eq!(connect(0, std::ptr::null(), 443, 0), 0);
        assert_eq!(GetLastError(), ERROR_WINHTTP_CANNOT_CONNECT);
        assert_eq!(send(0, 0, 0, 0, 0, 0, 0), 0);
        assert_eq!(GetLastError(), ERROR_WINHTTP_INCORRECT_HANDLE_TYPE);
        assert_eq!(
            callback(0, 0, 0, 0),
            usize::MAX,
            "WINHTTP_INVALID_STATUS_CALLBACK"
        );
        assert_eq!(crack(std::ptr::null(), 0, 0, 0), 0);
    }
}

#[test]
fn a_performed_http_call_completes_and_reports_no_network() {
    type Create = unsafe extern "win64" fn(*mut usize) -> i32;
    type Perform = unsafe extern "win64" fn(usize, *mut Block) -> i32;
    type Network = unsafe extern "win64" fn(usize, *mut i32, *mut u32) -> i32;
    type Number = unsafe extern "win64" fn(usize, *mut u32) -> i32;
    type Close = unsafe extern "win64" fn(usize) -> i32;
    let create: Create = function(HC, "HCHttpCallCreate");
    let set_url: unsafe extern "win64" fn(usize, *const u8, *const u8) -> i32 =
        function(HC, "HCHttpCallRequestSetUrl");
    let get_url: unsafe extern "win64" fn(usize, *mut *const c_char, *mut *const c_char) -> i32 =
        function(HC, "HCHttpCallRequestGetUrl");
    let set_header: unsafe extern "win64" fn(usize, *const u8, *const u8, u8) -> i32 =
        function(HC, "HCHttpCallRequestSetHeader");
    let header_count: Number = function(HC, "HCHttpCallRequestGetNumHeaders");
    let retry_delay: Number = function(HC, "HCHttpCallRequestGetRetryDelay");
    let perform: Perform = function(HC, "HCHttpCallPerformAsync");
    let network: Network = function(HC, "HCHttpCallResponseGetNetworkErrorCode");
    let status: Number = function(HC, "HCHttpCallResponseGetStatusCode");
    let duplicate: unsafe extern "win64" fn(usize) -> usize =
        function(HC, "HCHttpCallDuplicateHandle");
    let close: Close = function(HC, "HCHttpCallCloseHandle");
    // SAFETY: the documented shapes.
    unsafe {
        let mut call = 0usize;
        assert_eq!(create(&mut call), 0);
        assert_ne!(call, 0);
        assert_eq!(
            set_url(
                call,
                cstr("POST").as_ptr(),
                cstr("https://x.example/a").as_ptr()
            ),
            0
        );
        let (mut method, mut url) = (std::ptr::null(), std::ptr::null());
        assert_eq!(get_url(call, &mut method, &mut url), 0);
        assert_eq!(CStr::from_ptr(method).to_bytes(), b"POST");
        assert_eq!(CStr::from_ptr(url).to_bytes(), b"https://x.example/a");
        assert_eq!(
            set_header(call, cstr("A").as_ptr(), cstr("1").as_ptr(), 1),
            0
        );
        assert_eq!(
            set_header(call, cstr("A").as_ptr(), cstr("2").as_ptr(), 1),
            0
        );
        let mut count = 9u32;
        assert_eq!(header_count(call, &mut count), 0);
        assert_eq!(count, 1);
        let mut delay = 0u32;
        assert_eq!(retry_delay(call, &mut delay), 0);
        assert_eq!(delay, 2);

        let mut block = Block::default();
        assert_eq!(perform(call, &mut block), 0);
        assert_eq!(async_status(&mut block), 0, "the call itself completes");
        let (mut error, mut platform) = (0i32, 0u32);
        assert_eq!(network(call, &mut error, &mut platform), 0);
        assert_eq!(error, E_HC_NO_NETWORK);
        assert_eq!(platform, ERROR_WINHTTP_CANNOT_CONNECT);
        let mut code = 7u32;
        assert_eq!(status(call, &mut code), 0);
        assert_eq!(code, 0);

        assert_eq!(duplicate(call), call);
        assert_eq!(close(call), 0);
        assert_eq!(close(call), 0);
        assert_eq!(perform(0, &mut block), 0x8007_0057_u32 as i32);
    }
}

#[test]
fn http_client_cleanup_completes_instead_of_waiting() {
    let initialize: unsafe extern "win64" fn(usize) -> i32 = function(HC, "HCInitialize");
    let cleanup: unsafe extern "win64" fn(*mut Block) -> i32 = function(HC, "HCCleanupAsync");
    let trace_level: unsafe extern "win64" fn(*mut u32) -> i32 =
        function(HC, "HCSettingsGetTraceLevel");
    // SAFETY: the documented shapes.
    unsafe {
        assert_eq!(initialize(0), 0);
        let mut level = 5u32;
        assert_eq!(trace_level(&mut level), 0);
        assert_eq!(level, 0);
        let mut block = Block::default();
        assert_eq!(cleanup(&mut block), 0);
        assert_eq!(async_status(&mut block), 0);
    }
}
