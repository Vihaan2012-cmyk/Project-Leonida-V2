//! GameInput.dll: controllers, keyboard and mouse.

use crate::Shim;

pub mod abi;
pub mod config;
pub mod mapping;

pub fn shims() -> Vec<Shim> {
    Vec::new()
}
