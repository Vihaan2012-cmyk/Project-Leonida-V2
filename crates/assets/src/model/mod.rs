//! Models: drawables, fragments, skeletons and skinning, decoded from a resource's system and
//! graphics pages and exported as binary glTF with a JSON sidecar.

mod decode;
mod gltf;
mod pages;
mod sidecar;
mod skeleton;
mod types;
mod vertex;

#[cfg(test)]
mod real_data;
#[cfg(test)]
mod testdata;
#[cfg(test)]
mod tests;

pub use gltf::{ExportOptions, ExportedModel, LodSelection, export};
pub use sidecar::parameter_name;
pub use skeleton::joaat;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("the resource is not a drawable, fragment or drawable dictionary")]
    NotAModel,
    #[error("{what}: out of bounds")]
    Bounds { what: &'static str },
    #[error("{what}: {value:#x} is not a pointer into the resource")]
    Pointer { what: &'static str, value: u64 },
    #[error("invalid model data: {0}")]
    Invalid(String),
    #[error("not decoded: {0}")]
    Unsupported(String),
}

/// Decode a resource held as its system pages followed by its graphics pages.
pub fn decode(data: &[u8], info: &ResourceInfo) -> Result<ModelSet, ModelError> {
    if info.system_len > data.len() {
        return Err(ModelError::Bounds {
            what: "system length",
        });
    }
    let (system, graphics) = data.split_at(info.system_len);
    decode_pages(system, graphics, info.name.as_deref())
}

/// Decode a resource from its separate system and graphics pages.
pub fn decode_pages(
    system: &[u8],
    graphics: &[u8],
    name: Option<&str>,
) -> Result<ModelSet, ModelError> {
    decode::decode_set(pages::Pages { system, graphics }, name)
}
