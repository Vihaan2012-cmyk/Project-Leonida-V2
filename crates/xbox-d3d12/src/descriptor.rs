//! Descriptors in the console format (T#, V#, S# words) and their registration in the translated shaders' lookup
//! tables.

pub mod format;
pub mod gfx10;
pub mod registry;

#[cfg(test)]
mod gfx10_tests;

#[cfg(test)]
mod registry_tests;
