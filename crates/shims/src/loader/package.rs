//! `XPackageImpl`: the game's own package, as one installed package with nothing else installed.
//!
//! Measured slots: 3 the identifier, 4 the packaged-process query, 11 the user locale, 24-26 the mount path.
//! Slots whose behaviour the executable has not shown (installation monitors, chunk installs, enumeration)
//! are noted and fail.

use std::ffi::c_void;

use super::com::{ClassImpl, c_str, method, write_c_str};
use super::{
    E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_INSUFFICIENT_BUFFER, HR_NOT_FOUND, S_OK,
    package_identifier,
};
use crate::trace::note;

type This = *mut c_void;

const E_ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;

/// `XPackageGetCurrentProcessPackageIdentifier(bufferSize, buffer)`.
unsafe extern "win64" fn current_identifier(_: This, size: usize, buffer: *mut u8) -> i32 {
    if buffer.is_null() || size == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: `size` bytes are writable.
    if unsafe { write_c_str(buffer, size, &package_identifier()) } {
        S_OK
    } else {
        HR_INSUFFICIENT_BUFFER
    }
}

/// The program runs from the package folder, so it is a packaged process.
unsafe extern "win64" fn is_packaged_process(_: This) -> u8 {
    1
}

/// `XPackageGetUserLocale(localeSize, locale)`: the `language` setting (`LEONIDA_LOCALE` overrides), `en-US` otherwise.
unsafe extern "win64" fn user_locale(_: This, size: usize, buffer: *mut u8) -> i32 {
    if buffer.is_null() || size == 0 {
        return E_INVALIDARG;
    }
    let locale = super::locale::locale_tag();
    // SAFETY: `size` bytes are writable.
    if unsafe { write_c_str(buffer, size, &locale) } {
        S_OK
    } else {
        HR_INSUFFICIENT_BUFFER
    }
}

/// `XPackageMount(identifier, &mount)`: the game itself cannot be mounted, and no content package is installed.
unsafe extern "win64" fn mount(_: This, identifier: *const u8, handle: *mut usize) -> i32 {
    if handle.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *handle = 0 };
    // SAFETY: a string or null.
    let Some(identifier) = (unsafe { c_str(identifier) }) else {
        return E_INVALIDARG;
    };
    if identifier == package_identifier() {
        return E_ACCESS_DENIED;
    }
    note(format!(
        "XPackageMount({identifier}): no such content package is installed"
    ));
    HR_NOT_FOUND
}

/// No mount is ever made, so every mount handle is one the program invented.
unsafe extern "win64" fn mount_path_size(_: This, _handle: usize, size: *mut usize) -> i32 {
    if !size.is_null() {
        // SAFETY: writable, checked.
        unsafe { *size = 0 };
    }
    E_INVALIDARG
}

unsafe extern "win64" fn mount_path(
    _: This,
    _handle: usize,
    _size: usize,
    _buffer: *mut u8,
) -> i32 {
    E_INVALIDARG
}

unsafe extern "win64" fn close_mount(_: This, _handle: usize) {}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![ClassImpl {
        class: "XPackageImpl",
        root: "IXPackageImpl",
        methods: vec![
            method!(
                "XPackageGetCurrentProcessPackageIdentifier",
                current_identifier
            ),
            method!("XPackageIsPackagedProcess", is_packaged_process),
            method!("XPackageGetUserLocale", user_locale),
            method!("XPackageMount", mount),
            method!("XPackageGetMountPathSize", mount_path_size),
            method!("XPackageGetMountPath", mount_path),
            method!("XPackageCloseMountHandle", close_mount),
        ],
        failure: E_NOTIMPL,
    }]
}
