//! Resources: committed, placed at a game address, reserved; their GPU addresses and console layouts.

pub mod desc;
pub mod image;
pub mod layout;
pub mod tiling;
pub mod va;

#[cfg(test)]
mod tiling_tests;

#[cfg(test)]
mod layout_tests;

#[cfg(test)]
mod desc_tests;

#[cfg(test)]
mod va_tests;

#[cfg(test)]
mod image_tests;
