//! The smaller runtime classes: `XDisplayImpl`, `XLauncherImpl`, `XAccessibilityImpl`, `XUserDeviceImpl`,
//! `XGameUiImpl`, `XGameStreamingImpl` and `XNetworkingImpl`.
//!
//! Only the argument counts of the executable's calls were measured; the parameter lists follow the GDK
//! reference. Nothing here reaches a network: the networking class answers as a console with no connection.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use super::com::{ClassImpl, c_str, method};
use super::tasks::{self, AsyncBlock};
use super::user::new_handle;
use super::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_NETWORK_UNREACHABLE, HR_NOT_FOUND, S_OK,
};
use crate::trace::note;

type This = *mut c_void;

static TOKENS: AtomicU64 = AtomicU64::new(1 << 32);

// ---- XDisplayImpl ----

#[repr(C)]
struct HdrInfo {
    min_tone_map_luminance: f32,
    max_tone_map_luminance: f32,
    max_full_frame_tone_map_luminance: f32,
}

/// `XDisplayTryEnableHdrMode(preference, info)`: Disabled (2), as the host output is SDR; `LEONIDA_HDR=1` gives
/// Enabled (1) with the documented default tone-map luminances.
unsafe extern "win64" fn display_try_enable_hdr(
    _: This,
    _preference: u32,
    info: *mut HdrInfo,
) -> u32 {
    let hdr = std::env::var("LEONIDA_HDR").is_ok_and(|v| v == "1");
    if !info.is_null() {
        let (min, max) = if hdr { (0.01, 1000.0) } else { (0.0, 0.0) };
        // SAFETY: writable, checked.
        unsafe {
            info.write_unaligned(HdrInfo {
                min_tone_map_luminance: min,
                max_tone_map_luminance: max,
                max_full_frame_tone_map_luminance: max,
            });
        }
    }
    if hdr { 1 } else { 2 }
}

// ---- XLauncherImpl ----

unsafe extern "win64" fn launch_uri(_: This, _user: usize, uri: *const u8) -> i32 {
    // SAFETY: a string or null.
    note(format!(
        "XLaunchUri({}) refused",
        unsafe { c_str(uri) }.unwrap_or_default()
    ));
    E_FAIL
}

unsafe extern "win64" fn acquire_timeout_deferral(_: This, handle: *mut usize) -> i32 {
    if !handle.is_null() {
        // SAFETY: writable, checked.
        unsafe { *handle = 0 };
    }
    E_FAIL
}

unsafe extern "win64" fn close_timeout_deferral(_: This, _handle: usize) -> i32 {
    E_FAIL
}

// ---- XAccessibilityImpl ----

unsafe extern "win64" fn captions_set_enabled(_: This, _enabled: u8) -> i32 {
    S_OK
}

/// `XHighContrastGetMode(&mode)`: Off.
unsafe extern "win64" fn high_contrast_mode(_: This, mode: *mut u32) -> i32 {
    if mode.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *mode = 0 };
    S_OK
}

/// No voice is installed: the callback runs zero times.
unsafe extern "win64" fn enumerate_voices(_: This, _callback: usize, _context: usize) -> i32 {
    S_OK
}

unsafe extern "win64" fn speech_create(_: This, handle: *mut usize) -> i32 {
    if !handle.is_null() {
        // SAFETY: writable, checked.
        unsafe { *handle = 0 };
    }
    E_FAIL
}

/// Handle calls refuse the handle they were never given.
unsafe extern "win64" fn speech_bad_handle(_: This, _handle: usize) -> i32 {
    E_INVALIDARG
}

unsafe extern "win64" fn speech_voice(_: This, _handle: usize, _voice: usize) -> i32 {
    E_INVALIDARG
}

unsafe extern "win64" fn speech_stream_from_text(
    _: This,
    _handle: usize,
    _text: *const u8,
    stream: *mut usize,
) -> i32 {
    if !stream.is_null() {
        // SAFETY: writable, checked.
        unsafe { *stream = 0 };
    }
    E_INVALIDARG
}

unsafe extern "win64" fn speech_stream_size(_: This, _stream: usize, size: *mut usize) -> i32 {
    if !size.is_null() {
        // SAFETY: writable, checked.
        unsafe { *size = 0 };
    }
    E_INVALIDARG
}

unsafe extern "win64" fn speech_stream_data(
    _: This,
    _stream: usize,
    _size: usize,
    _buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    E_INVALIDARG
}

// ---- XUserDeviceImpl ----

/// Every device belongs to the offline user.
unsafe extern "win64" fn find_user_for_device(
    _: This,
    _device: *const u8,
    handle: *mut usize,
) -> i32 {
    if handle.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *handle = new_handle() };
    S_OK
}

/// A registration is kept under a token and never fires: the association and the audio endpoint never change.
unsafe extern "win64" fn device_register(
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

unsafe extern "win64" fn device_unregister(_: This, _token: u64, _wait: u8) -> u8 {
    1
}

/// The user has no audio endpoint of its own.
unsafe extern "win64" fn default_audio_endpoint(
    _: This,
    _user: u64,
    _kind: u32,
    _count: usize,
    id: *mut u16,
    used: *mut usize,
) -> i32 {
    if !id.is_null() {
        // SAFETY: at least one element is writable for an id buffer.
        unsafe { *id = 0 };
    }
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    HR_NOT_FOUND
}

/// Finding a controller shows no interface, as on desktop.
unsafe extern "win64" fn find_controller_async(
    _: This,
    _user: usize,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XUserFindControllerForUserWithUiAsync", || {
            (S_OK, vec![0u8; 32])
        })
    }
}

unsafe extern "win64" fn find_controller_result(
    _: This,
    block: *mut AsyncBlock,
    device: *mut u8,
) -> i32 {
    if device.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block and an id buffer of 32 bytes.
    unsafe {
        tasks::one_shot_result(
            block,
            "XUserFindControllerForUserWithUiAsync",
            32,
            device as *mut c_void,
        )
    }
}

// ---- XGameUiImpl ----

/// A message dialog is noted and answered with its default button.
unsafe extern "win64" fn message_dialog_async(
    _: This,
    block: *mut AsyncBlock,
    title: *const u8,
    content: *const u8,
    first: *const u8,
    second: *const u8,
    third: *const u8,
    default_button: u32,
    _cancel_button: u32,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if default_button > 2 {
        return E_INVALIDARG;
    }
    // SAFETY: strings or null.
    let (title, content, first, second, third) = unsafe {
        (
            c_str(title),
            c_str(content),
            c_str(first),
            c_str(second),
            c_str(third),
        )
    };
    note(format!(
        "game dialog \"{}\": {} [{} / {} / {}] answered with button {default_button}",
        title.unwrap_or_default(),
        content.unwrap_or_default(),
        first.unwrap_or_default(),
        second.unwrap_or_default(),
        third.unwrap_or_default()
    ));
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameUiShowMessageDialogAsync", move || {
            (S_OK, default_button.to_le_bytes().to_vec())
        })
    }
}

unsafe extern "win64" fn message_dialog_result(
    _: This,
    block: *mut AsyncBlock,
    button: *mut u32,
) -> i32 {
    if button.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block and a result it can hold.
    unsafe {
        tasks::one_shot_result(
            block,
            "XGameUiShowMessageDialogAsync",
            4,
            button as *mut c_void,
        )
    }
}

unsafe extern "win64" fn error_dialog_async(
    _: This,
    block: *mut AsyncBlock,
    code: i32,
    context: *const u8,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    note(format!(
        "game error dialog {code:#010x} {} dismissed",
        unsafe { c_str(context) }.unwrap_or_default()
    ));
    // SAFETY: the program's block.
    unsafe { tasks::begin_one_shot(block, "XGameUiShowErrorDialogAsync", || (S_OK, Vec::new())) }
}

unsafe extern "win64" fn error_dialog_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe {
        tasks::one_shot_result(
            block,
            "XGameUiShowErrorDialogAsync",
            0,
            std::ptr::null_mut(),
        )
    }
}

unsafe extern "win64" fn notification_hint(_: This, _position: u32) -> i32 {
    S_OK
}

// ---- XGameStreamingImpl ----

const E_GAMESTREAMING_NOT_INITIALIZED: i32 = 0x8924_5400_u32 as i32;

unsafe extern "win64" fn streaming_initialize(_: This) -> i32 {
    S_OK
}

unsafe extern "win64" fn streaming_uninitialize(_: This) {}

unsafe extern "win64" fn streaming_is_streaming(_: This) -> u8 {
    0
}

unsafe extern "win64" fn streaming_client_count(_: This) -> u32 {
    0
}

unsafe extern "win64" fn streaming_connection_state(_: This) -> u32 {
    0
}

unsafe extern "win64" fn streaming_register(
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

unsafe extern "win64" fn streaming_unregister(_: This, _token: u64, _wait: u8) -> u8 {
    1
}

// ---- XNetworkingImpl ----

/// `XNetworkingConnectivityHint`: level, cost, interface type, then four booleans.
#[repr(C)]
struct ConnectivityHint {
    level: u32,
    cost: u32,
    interface_type: u32,
    network_initialized: u8,
    approaching_data_limit: u8,
    over_data_limit: u8,
    roaming: u8,
}

fn offline_hint() -> ConnectivityHint {
    ConnectivityHint {
        level: 1,
        cost: 0,
        interface_type: 0,
        network_initialized: 1,
        approaching_data_limit: 0,
        over_data_limit: 0,
        roaming: 0,
    }
}

unsafe extern "win64" fn net_query_port(_: This, port: *mut u16) -> i32 {
    if port.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *port = 0 };
    HR_NETWORK_UNREACHABLE
}

unsafe extern "win64" fn net_async(_: This, block: *mut AsyncBlock) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XNetworking", || {
            (HR_NETWORK_UNREACHABLE, Vec::new())
        })
    }
}

unsafe extern "win64" fn net_async_url(_: This, url: *const c_void, block: *mut AsyncBlock) -> i32 {
    if url.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe { net_async(std::ptr::null_mut(), block) }
}

unsafe extern "win64" fn net_port_result(_: This, block: *mut AsyncBlock, port: *mut u16) -> i32 {
    if block.is_null() || port.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *port = 0 };
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

unsafe extern "win64" fn net_result_size(_: This, block: *mut AsyncBlock, size: *mut usize) -> i32 {
    if block.is_null() || size.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *size = 0 };
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

unsafe extern "win64" fn net_result(
    _: This,
    block: *mut AsyncBlock,
    _size: usize,
    used: *mut usize,
    _buffer: *mut u8,
    info: *mut usize,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    if !info.is_null() {
        // SAFETY: writable, checked.
        unsafe { *info = 0 };
    }
    // SAFETY: the program's block.
    unsafe { tasks::finish_call(block) }
}

unsafe extern "win64" fn net_verify_certificate(
    _: This,
    _request: usize,
    _certificate: usize,
) -> i32 {
    HR_NETWORK_UNREACHABLE
}

unsafe extern "win64" fn net_connectivity_hint(_: This, hint: *mut ConnectivityHint) -> i32 {
    if hint.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { hint.write_unaligned(offline_hint()) };
    S_OK
}

struct Initial {
    context: usize,
    callback: usize,
    port: bool,
}

unsafe extern "win64" fn initial_notification(context: *mut c_void, canceled: u8) {
    // SAFETY: the box `net_register` handed over.
    let initial = unsafe { Box::from_raw(context as *mut Initial) };
    if canceled != 0 {
        return;
    }
    if initial.port {
        // SAFETY: the callback the program registered, with its documented shape.
        let callback: unsafe extern "win64" fn(*mut c_void, u16) =
            unsafe { std::mem::transmute(initial.callback) };
        // SAFETY: the program's own callback.
        unsafe { callback(initial.context as *mut c_void, 0) };
    } else {
        let hint = offline_hint();
        // SAFETY: as above.
        let callback: unsafe extern "win64" fn(*mut c_void, *const ConnectivityHint) =
            unsafe { std::mem::transmute(initial.callback) };
        // SAFETY: the program's own callback, handed a hint that outlives the call.
        unsafe { callback(initial.context as *mut c_void, &hint) };
    }
}

/// A change registration sends the one initial notification Microsoft documents, and none after.
unsafe fn net_register(
    queue: usize,
    context: usize,
    callback: usize,
    token: *mut u64,
    port: bool,
) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    if token.is_null() {
        return E_POINTER;
    }
    let initial = Box::into_raw(Box::new(Initial {
        context,
        callback,
        port,
    }));
    // SAFETY: a queue handle or null (the process queue); the box is taken over by the callback.
    let result = unsafe {
        tasks::submit(
            queue as *mut c_void,
            1,
            initial as usize,
            initial_notification as *const () as usize,
        )
    };
    if result < 0 {
        // SAFETY: the callback was not queued, so the box is still ours.
        drop(unsafe { Box::from_raw(initial) });
        return result;
    }
    // SAFETY: writable, checked.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) };
    S_OK
}

unsafe extern "win64" fn net_register_port(
    _: This,
    queue: usize,
    context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { net_register(queue, context, callback, token, true) }
}

unsafe extern "win64" fn net_register_hint(
    _: This,
    queue: usize,
    context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    // SAFETY: the program's arguments.
    unsafe { net_register(queue, context, callback, token, false) }
}

unsafe extern "win64" fn net_unregister(_: This, _token: u64, _wait: u8) -> u8 {
    1
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![
        ClassImpl {
            class: "XDisplayImpl",
            root: "IXDisplayImpl",
            methods: vec![method!("XDisplayTryEnableHdrMode", display_try_enable_hdr)],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XLauncherImpl",
            root: "IXLauncherImpl",
            methods: vec![
                method!("XLaunchUri", launch_uri),
                method!("XDisplayAcquireTimeoutDeferral", acquire_timeout_deferral),
                method!("XDisplayCloseTimeoutDeferralHandle", close_timeout_deferral),
            ],
            failure: E_FAIL,
        },
        ClassImpl {
            class: "XAccessibilityImpl",
            root: "IXAccessibilityImpl",
            methods: vec![
                method!("XClosedCaptionSetEnabled", captions_set_enabled),
                method!("XHighContrastGetMode", high_contrast_mode),
                method!(
                    "XSpeechSynthesizerEnumerateInstalledVoices",
                    enumerate_voices
                ),
                method!("XSpeechSynthesizerCreate", speech_create),
                method!("XSpeechSynthesizerCloseHandle", speech_bad_handle),
                method!("XSpeechSynthesizerSetDefaultVoice", speech_voice),
                method!("XSpeechSynthesizerSetCustomVoice", speech_voice),
                method!(
                    "XSpeechSynthesizerCreateStreamFromText",
                    speech_stream_from_text
                ),
                method!("XSpeechSynthesizerCloseStreamHandle", speech_bad_handle),
                method!("XSpeechSynthesizerGetStreamDataSize", speech_stream_size),
                method!("XSpeechSynthesizerGetStreamData", speech_stream_data),
                method!(
                    "XSpeechSynthesizerCreateStreamFromSsml",
                    speech_stream_from_text
                ),
            ],
            failure: E_FAIL,
        },
        ClassImpl {
            class: "XUserDeviceImpl",
            root: "IXUserDeviceImpl",
            methods: vec![
                method!("XUserFindForDevice", find_user_for_device),
                method!("XUserRegisterForDeviceAssociationChanged", device_register),
                method!(
                    "XUserUnregisterForDeviceAssociationChanged",
                    device_unregister
                ),
                method!("XUserGetDefaultAudioEndpointUtf16", default_audio_endpoint),
                method!(
                    "XUserRegisterForDefaultAudioEndpointUtf16Changed",
                    device_register
                ),
                method!(
                    "XUserUnregisterForDefaultAudioEndpointUtf16Changed",
                    device_unregister
                ),
                method!(
                    "XUserFindControllerForUserWithUiAsync",
                    find_controller_async
                ),
                method!(
                    "XUserFindControllerForUserWithUiResult",
                    find_controller_result
                ),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XGameUiImpl",
            root: "IXGameUiImpl",
            methods: vec![
                method!("XGameUiShowMessageDialogAsync", message_dialog_async),
                method!("XGameUiShowMessageDialogResult", message_dialog_result),
                method!("XGameUiShowErrorDialogAsync", error_dialog_async),
                method!("XGameUiShowErrorDialogResult", error_dialog_result),
                method!("XGameUiSetNotificationPositionHint", notification_hint),
            ],
            failure: E_FAIL,
        },
        ClassImpl {
            class: "XGameStreamingImpl",
            root: "IXGameStreamingImpl",
            methods: vec![
                method!("XGameStreamingInitialize", streaming_initialize),
                method!("XGameStreamingUninitialize", streaming_uninitialize),
                method!("XGameStreamingIsStreaming", streaming_is_streaming),
                method!("XGameStreamingGetClientCount", streaming_client_count),
                method!(
                    "XGameStreamingGetConnectionState",
                    streaming_connection_state
                ),
                method!(
                    "XGameStreamingRegisterConnectionStateChanged",
                    streaming_register
                ),
                method!(
                    "XGameStreamingUnregisterConnectionStateChanged",
                    streaming_unregister
                ),
                method!(
                    "XGameStreamingUnregisterClientPropertiesChanged",
                    streaming_unregister
                ),
            ],
            failure: E_GAMESTREAMING_NOT_INITIALIZED,
        },
        ClassImpl {
            class: "XNetworkingImpl",
            root: "IXNetworkingImpl",
            methods: vec![
                method!(
                    "XNetworkingQueryPreferredLocalUdpMultiplayerPort",
                    net_query_port
                ),
                method!(
                    "XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync",
                    net_async
                ),
                method!(
                    "XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult",
                    net_port_result
                ),
                method!(
                    "XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged",
                    net_register_port
                ),
                method!(
                    "XNetworkingUnregisterPreferredLocalUdpMultiplayerPortChanged",
                    net_unregister
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlAsync",
                    net_async_url
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlAsyncResultSize",
                    net_result_size
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlAsyncResult",
                    net_result
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlUtf16Async",
                    net_async_url
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlUtf16AsyncResultSize",
                    net_result_size
                ),
                method!(
                    "XNetworkingQuerySecurityInformationForUrlUtf16AsyncResult",
                    net_result
                ),
                method!("XNetworkingVerifyServerCertificate", net_verify_certificate),
                method!("XNetworkingGetConnectivityHint", net_connectivity_hint),
                method!(
                    "XNetworkingRegisterConnectivityHintChanged",
                    net_register_hint
                ),
                method!(
                    "XNetworkingUnregisterConnectivityHintChanged",
                    net_unregister
                ),
            ],
            failure: HR_NETWORK_UNREACHABLE,
        },
    ]
}
