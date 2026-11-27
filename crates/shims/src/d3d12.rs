//! The console graphics entry point: `D3D12XboxCreateDevice` creates the assembled device of the `xbox-d3d12` crate
//! (one Vulkan device behind every part: replay, pipelines, memory, descriptors, presentation). Methods and
//! interfaces that crate does not know are noted in the trace, by interface, slot and method name from
//! `d3d12_slots.txt`, with the caller's address, and fail with `E_NOTIMPL`.
//!
//! The entry point's shape, `(adapter, params, riid, out)` with a 24-word parameter block, was measured in the
//! executable, not documented. `D3D12SerializeVersionedRootSignature` is the public function; the arguments of
//! `D3D12ConfigureVirtualMemoryX` are not known, so it succeeds and records what it was given.

use std::ffi::c_void;

use config::{AspectMode, WindowMode};
use xbox_d3d12::assembly::{self, AssemblyConfig, InputEvent, Output, WindowOptionsConfig};
use xbox_d3d12::present::ScaleMode;

use crate::gameinput::config::MouseButton;
use crate::gameinput::{RawInput, submit_input};
use crate::trace::{image_offset, note};
use crate::{Shim, presentation};

fn trace_note(text: String) {
    note(text);
}

fn forward_input(event: InputEvent) {
    let button = |b: xbox_d3d12::assembly::MouseButton| match b {
        xbox_d3d12::assembly::MouseButton::Left => MouseButton::Left,
        xbox_d3d12::assembly::MouseButton::Right => MouseButton::Right,
        xbox_d3d12::assembly::MouseButton::Middle => MouseButton::Middle,
        xbox_d3d12::assembly::MouseButton::X1 => MouseButton::X1,
        xbox_d3d12::assembly::MouseButton::X2 => MouseButton::X2,
    };
    submit_input(match event {
        InputEvent::Key {
            virtual_key,
            scan_code,
            down,
        } => RawInput::Key {
            virtual_key,
            scan_code,
            down,
        },
        InputEvent::MouseMove { dx, dy } => RawInput::MouseMove { dx, dy },
        InputEvent::MouseButton { button: b, down } => RawInput::MouseButton {
            button: button(b),
            down,
        },
        InputEvent::MouseWheel { dx, dy } => RawInput::MouseWheel { dx, dy },
        InputEvent::Focus(focus) => RawInput::Focus(focus),
    });
}

/// The window and presenter the player chose (`presentation::get()`), as the assembly takes them.
pub fn assembly_config() -> AssemblyConfig {
    let chosen = presentation::get();
    let graphics = &chosen.graphics;
    let mut config = AssemblyConfig::windowed();
    config.frame_limit = chosen.frame_limit;
    config.output = Output::Window(WindowOptionsConfig {
        size: (graphics.window_width.max(1), graphics.window_height.max(1)),
        borderless: graphics.window_mode != WindowMode::Windowed,
        ..WindowOptionsConfig::default()
    });
    config.present.prefer_hdr = graphics.hdr;
    config.present.scale = match graphics.aspect_mode {
        AspectMode::Stretch => ScaleMode::Stretch,
        AspectMode::Letterbox | AspectMode::Native => ScaleMode::Fit,
    };
    config
}

/// Builds the assembled device (and with it the address space the `XMem*` shims report to) before the game
/// allocates. `leonida run` may call it early; `D3D12XboxCreateDevice` calls it when nobody did.
pub fn prepare() -> Result<(), xbox_d3d12::assembly::AssemblyError> {
    xbox_d3d12::com::set_trace(trace_note);
    xbox_d3d12::com::set_address_format(image_offset);
    assembly::set_input_sink(forward_input);
    assembly::set_frame_limit(presentation::get().frame_limit);
    assembly::init(assembly_config()).map(|_| ())
}

/// # Safety
/// The measured console shape: `params` points at 24 readable words or is null, `riid` at an IID, `out` is
/// writable or null.
unsafe extern "win64" fn create_device(
    _adapter: *mut c_void,
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if let Err(e) = prepare() {
        note(format!("the assembled device could not be built: {e}"));
        return 0x887A_0004;
    }
    // SAFETY: as the caller promises.
    unsafe { assembly::create_assembled_device(params, riid, out) }
}

/// `D3D12SerializeVersionedRootSignature(desc, blob, error)`.
///
/// # Safety
/// The public shape: `desc` is a readable versioned description, `blob` writable, `error` writable or null.
unsafe extern "win64" fn serialize_versioned_root_signature(
    desc: *const u8,
    blob: *mut *mut c_void,
    error: *mut *mut c_void,
) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { assembly::serialize_versioned_root_signature(desc, blob, error) }
}

/// `D3D12ConfigureVirtualMemoryX`: measured as one pointer argument, a zeroed 32-byte block whose dword at +4 is a
/// count; the title only tests the result for failure. Its meaning is not measured, so it succeeds and the block it
/// was given is recorded.
///
/// # Safety
/// `block` is null or 32 readable bytes.
unsafe extern "win64" fn configure_virtual_memory(block: *const u32) -> u32 {
    let words = if block.is_null() {
        "null".to_string()
    } else {
        // SAFETY: as the caller promises.
        unsafe { std::slice::from_raw_parts(block, 8) }
            .iter()
            .map(|w| format!("{w:08x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    note(format!(
        "D3D12ConfigureVirtualMemoryX was called with {words}; its meaning is not measured, so it succeeded"
    ));
    0
}

pub fn shims() -> Vec<Shim> {
    vec![
        Shim {
            dll: "d3d12_xs.dll",
            name: "D3D12XboxCreateDevice",
            address: create_device as *const () as usize,
        },
        Shim {
            dll: "d3d12_xs.dll",
            name: "D3D12SerializeVersionedRootSignature",
            address: serialize_versioned_root_signature as *const () as usize,
        },
        Shim {
            dll: "d3d12_xs.dll",
            name: "D3D12ConfigureVirtualMemoryX",
            address: configure_virtual_memory as *const () as usize,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shims_name_the_device_and_the_two_exports_the_dry_run_listed() {
        let names: Vec<&str> = shims().iter().map(|s| s.name).collect();
        assert!(names.contains(&"D3D12SerializeVersionedRootSignature"));
        assert!(names.contains(&"D3D12ConfigureVirtualMemoryX"));
        assert!(names.contains(&"D3D12XboxCreateDevice"));
    }

    #[test]
    fn the_unmeasured_virtual_memory_call_succeeds_with_or_without_a_block() {
        let block = [0u32; 8];
        // SAFETY: a null pointer and 32 readable bytes.
        unsafe {
            assert_eq!(configure_virtual_memory(std::ptr::null()), 0);
            assert_eq!(configure_virtual_memory(block.as_ptr()), 0);
        }
    }
}
