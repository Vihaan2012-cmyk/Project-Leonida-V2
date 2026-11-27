//! Upscaling and frame generation: FSR 1 in WGSL, and optional NVIDIA DLSS through the player's own driver DLLs.
#![allow(unsafe_code)]

pub mod dlss;
pub mod framegen;
pub mod fsr1;
