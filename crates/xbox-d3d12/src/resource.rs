//! Resources: committed, placed at a game address, reserved; their GPU addresses and console layouts.

pub mod desc;
pub mod layout;
pub mod tiling;

#[cfg(test)]
mod tiling_tests;

#[cfg(test)]
mod layout_tests;

#[cfg(test)]
mod desc_tests;
