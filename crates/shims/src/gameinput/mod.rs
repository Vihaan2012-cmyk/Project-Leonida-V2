//! GameInput.dll: controllers, keyboard and mouse.

use crate::Shim;

pub mod abi;

pub fn shims() -> Vec<Shim> {
    Vec::new()
}
