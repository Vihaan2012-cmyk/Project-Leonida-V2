//! GameInput.dll: controllers, keyboard and mouse.

use crate::Shim;

pub mod abi;
mod com;
pub mod config;
mod device;
pub mod hub;
pub mod mapping;

pub fn shims() -> Vec<Shim> {
    Vec::new()
}
