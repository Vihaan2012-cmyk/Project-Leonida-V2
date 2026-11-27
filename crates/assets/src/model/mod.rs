//! Models: drawables, fragments, skeletons and skinning, decoded from a resource's system and
//! graphics pages and exported as binary glTF with a JSON sidecar.

mod pages;

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
