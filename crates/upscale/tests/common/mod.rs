#![allow(dead_code)]

use std::sync::Arc;

use upscale::SharedCompute;

pub fn gpu() -> Option<SharedCompute> {
    match gpu::Gpu::new() {
        Ok(gpu) => Some(Arc::new(gpu)),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

pub fn rgba32(texels: &[[f32; 4]]) -> Vec<u8> {
    texels
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

pub fn r32(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn rg32(values: &[[f32; 2]]) -> Vec<u8> {
    values
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

pub fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}
