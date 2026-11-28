//! GameInput.dll: controllers, keyboard and mouse.
//!
//! The title imports one function, ordinal 1 of `GameInput.dll` (`GameInputCreate`), and gets an `IGameInput`
//! back. Everything behind it lives in [`hub::Hub`]: devices from gilrs (controllers) and from raw input the
//! window owner forwards with [`submit_input`], their reading histories and callbacks. A keyboard-and-mouse
//! gamepad is made from the raw input according to [`config::InputConfig`].
//!
//! Only the v0 interfaces exist. Which version the executable was built against is unmeasured; interface ids
//! it asks for are noted in the trace.

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use crate::Shim;

pub mod abi;
pub mod analog;
mod backend;
mod com;
pub mod config;
pub mod curve;
mod device;
pub mod direct;
pub mod hub;
pub mod mapping;
pub mod settings;
#[cfg(test)]
mod tests;

pub use config::InputConfig;
pub use hub::{Hub, RawInput};

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

/// The hub every `GameInputCreate` shares.
pub fn global() -> &'static Arc<Hub> {
    HUB.get_or_init(|| Hub::new(InputConfig::default()))
}

/// Replaces the mapping and output settings.
pub fn set_config(config: InputConfig) {
    global().set_config(config);
}

/// Keyboard and mouse input from the game window, called by the window owner for each event.
pub fn submit_input(event: RawInput) {
    global().input(event);
}

/// `HRESULT GameInputCreate(IGameInput** gameInput)`.
extern "win64" fn game_input_create(out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return abi::E_POINTER;
    }
    let hub = global();
    backend::ensure_running(hub);
    // SAFETY: `out` is the caller's out pointer, checked above.
    unsafe { *out = com::new_game_input(hub.clone()) };
    abi::S_OK
}

pub fn shims() -> Vec<Shim> {
    let address = game_input_create as *const () as usize;
    vec![
        Shim {
            dll: "GameInput.dll",
            name: "GameInputCreate",
            address,
        },
        Shim {
            dll: "GameInput.dll",
            name: "#1",
            address,
        },
    ]
}
