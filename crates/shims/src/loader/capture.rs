//! `XAppCaptureImpl`: the console's screenshot and clip service. The title takes a screenshot on a key press; a
//! host that has no capture service answers that none can be taken, which the title shows as a failed capture
//! and goes on.
//!
//! `XAppCaptureTakeScreenshot` (slot 5) is the only slot the executable calls: it passes the user and the address
//! of a 0x11c byte result, and reads only the `HRESULT` (RVA 0x223c9b0 records 0x82323002 as one outcome and any
//! other failure as another). The result is left untouched on failure.

use std::ffi::c_void;

use super::com::{ClassImpl, method};
use super::{HR_NOT_SUPPORTED, S_OK};
use crate::trace::note;

type This = *mut c_void;

unsafe extern "win64" fn take_screenshot(_: This, _user: usize, _result: *mut c_void) -> i32 {
    note("XAppCaptureTakeScreenshot: no capture service, refused".to_string());
    HR_NOT_SUPPORTED
}

/// Recording is never on, so turning it on or off changes nothing.
unsafe extern "win64" fn record_switch(_: This) -> i32 {
    S_OK
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![ClassImpl {
        class: "XAppCaptureImpl",
        root: "IXAppCaptureImpl",
        methods: vec![
            method!("XAppCaptureTakeScreenshot", take_screenshot),
            method!("XAppCaptureEnableRecord", record_switch),
            method!("XAppCaptureDisableRecord", record_switch),
        ],
        failure: HR_NOT_SUPPORTED,
    }]
}
