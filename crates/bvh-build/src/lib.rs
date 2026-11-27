//! Ray-tracing acceleration structures in the console's node format: the 64-byte units the translated
//! `image_bvh_intersect_ray` reads (pointer type in the low three bits, unit index above them, counted from
//! the structure's header), built from triangles or instances on the CPU or, for large builds, in WGSL
//! compute kernels, with refit and compaction.
//!
//! A structure is its header (64 bytes), the element records from offset 128 (64-byte triangle leaves or
//! 128-byte instance records, in input order), the box nodes, one zeroed unit after BOX16 nodes, and the parent
//! table. The header's `rootNodePtr` is the runtime root.

mod cpu;
mod half;
mod input;
mod layout;
mod refit;
mod structure;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use cpu::Cpu;
pub use input::{
    BlasInfo, GEOMETRY_OPAQUE, Geometry, Instance, Triangle, TriangleSource, index_bytes,
    read_triangles, triangle_count, vertex_bytes,
};
pub use layout::{
    BLAS_FLAGS, BOX16, BOX32, EMPTY_BOX, ElementKind, HEADER_BYTES, Header, INVALID, Layout,
    NodeKind, TLAS_FLAGS, layout, max_nodes, pointer,
};
pub use structure::{
    ElementSet, blas_info, build_blas, build_elements, build_tlas, compact, instance_elements,
    prebuild_size, refit_blas, refit_tlas, triangle_elements,
};

pub const TRI0: u32 = 0;
pub const INSTANCE: u32 = 6;

/// D3D12 acceleration structure build flags.
pub mod flags {
    pub const ALLOW_UPDATE: u32 = 0x01;
    pub const ALLOW_COMPACTION: u32 = 0x02;
    pub const PREFER_FAST_TRACE: u32 = 0x04;
    pub const PREFER_FAST_BUILD: u32 = 0x08;
    pub const MINIMIZE_MEMORY: u32 = 0x10;
    pub const PERFORM_UPDATE: u32 = 0x20;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("malformed input: {0}")]
    Malformed(&'static str),
    #[error("the structure would not fit in 4 GiB")]
    TooLarge,
    #[error("GPU build failed: {0}")]
    Gpu(String),
}

pub type Result<T> = std::result::Result<T, BuildError>;

/// Where a build's nodes go, as unit indices counted from the structure's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeParams {
    pub element_base: u32,
    pub element_stride: u32,
    pub element_type: u32,
    pub node_base: u32,
    pub node_units: u32,
    pub node_type: u32,
    /// Parent table entries before the first node unit.
    pub node_table_base: u32,
}

/// What a tree build produces: the node records and the parent table, one entry per unit from the first
/// element on (a node's pointer, or all ones for the root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOutput {
    pub node_count: u32,
    pub nodes: Vec<u32>,
    pub parents: Vec<u32>,
}

/// Builds the tree over at least two element boxes; an empty box has its minimum above its maximum.
pub trait NodeBuilder {
    fn build(&mut self, boxes: &[[f32; 6]], params: &NodeParams) -> Result<NodeOutput>;
}
