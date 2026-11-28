//! The command stream a command list records and `Replayer` plays back on Vulkan.
//!
//! Buffers are named by console VA: every buffer lives in the arena, one `VkBuffer` whose offsets are VAs.
//! Images and views are named by the Vulkan handles the resource layer made, which stay valid until the list has
//! run.

use std::sync::Arc;

use ash::vk;
use smallvec::SmallVec;

use crate::pipeline::Pipeline;
use crate::root::{HeapBases, RootSignature};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindPoint {
    Graphics,
    Compute,
}

/// An image and what a barrier or copy needs to know of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRef {
    pub image: vk::Image,
    pub format: vk::Format,
    pub aspect: vk::ImageAspectFlags,
    pub extent: [u32; 3],
    pub mips: u32,
    pub layers: u32,
}

impl ImageRef {
    pub fn is_depth(&self) -> bool {
        self.aspect
            .intersects(vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL)
    }

    /// The size of mip `mip`.
    pub fn mip_extent(&self, mip: u32) -> [u32; 3] {
        self.extent.map(|d| (d >> mip).max(1))
    }
}

/// A render target or depth view: the view, and the size of the mip it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetView {
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: [u32; 2],
    pub samples: vk::SampleCountFlags,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexBufferView {
    pub va: u64,
    pub size: u32,
    pub stride: u32,
}

/// Rectangles, almost always none or one.
pub type Rects = SmallVec<[Rect; 1]>;

/// The viewports of one call: a pass sets one.
pub type Viewports = SmallVec<[Viewport; 2]>;

pub use crate::stream::RootValues;

/// The vertex buffers of one call: a draw reads a few streams, the limit is 16.
pub type VertexViews = SmallVec<[VertexBufferView; 2]>;

/// The colour targets of one pass.
pub type ColorTargets = SmallVec<[TargetView; 2]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBufferView {
    pub va: u64,
    pub size: u32,
    /// `DXGI_FORMAT_R16_UINT` (57) or `DXGI_FORMAT_R32_UINT` (42).
    pub format: u32,
}

/// What one `ExecuteIndirect` argument record holds, for the signatures that map to one Vulkan call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndirectKind {
    Draw,
    DrawIndexed,
    Dispatch,
    DispatchMesh,
}

/// A buffer side of a copy: the footprint of a texture in a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferFootprint {
    pub va: u64,
    /// Bytes from one row to the next.
    pub row_pitch: u32,
    /// Rows from one slice to the next; 0 for the height of the copied box.
    pub slice_rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subresource {
    pub mip: u32,
    pub layer: u32,
}

/// A box of texels, `D3D12_BOX` without the sign games.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexelBox {
    pub origin: [u32; 3],
    pub size: [u32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Image(ImageRef),
    /// A buffer: barriers on buffers are one memory barrier, since all buffers share an arena.
    Buffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Barrier {
    /// A state transition with `D3D12_RESOURCE_STATES` bits before and after; `subresource` of `None` is all.
    Transition {
        resource: Resource,
        subresource: Option<Subresource>,
        before: u32,
        after: u32,
    },
    /// Unordered-access writes finish before later accesses.
    Uav,
    /// A placed resource takes over memory another one used.
    Aliasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Occlusion,
    BinaryOcclusion,
    Timestamp,
    PipelineStatistics,
}

/// An image to image copy, boxed in `Command` because two image references make it the largest command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCopy {
    pub dst: ImageRef,
    pub dst_sub: Subresource,
    pub at: [u32; 3],
    pub src: ImageRef,
    pub src_sub: Subresource,
    pub region: TexelBox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageResolve {
    pub dst: ImageRef,
    pub dst_sub: Subresource,
    pub src: ImageRef,
    pub src_sub: Subresource,
}

#[derive(Debug, Clone)]
pub enum Command {
    SetRootSignature {
        bind: BindPoint,
        root: Arc<RootSignature>,
    },
    SetPipeline(Arc<Pipeline>),
    SetRootConstants {
        bind: BindPoint,
        param: u32,
        first: u32,
        values: RootValues,
    },
    SetRootView {
        bind: BindPoint,
        param: u32,
        va: u64,
    },
    SetRootTable {
        bind: BindPoint,
        param: u32,
        /// The `D3D12_GPU_DESCRIPTOR_HANDLE`, a console VA below 4 GiB.
        handle: u64,
    },
    /// `SetDescriptorHeaps`: the 32-bit bases a root signature that indexes its heaps directly receives.
    SetDescriptorHeaps(HeapBases),
    SetViewports(Viewports),
    SetScissors(Rects),
    SetBlendFactor([f32; 4]),
    SetStencilRef(u32),
    /// `IASetPrimitiveTopology`, a `D3D_PRIMITIVE_TOPOLOGY` value.
    SetTopology(u32),
    SetVertexBuffers {
        start: u32,
        views: VertexViews,
    },
    SetIndexBuffer(Option<IndexBufferView>),
    SetRenderTargets {
        colors: ColorTargets,
        depth: Option<TargetView>,
    },
    ClearRenderTarget {
        view: TargetView,
        color: [f32; 4],
        rects: Rects,
    },
    ClearDepthStencil {
        view: TargetView,
        depth: Option<f32>,
        stencil: Option<u8>,
        rects: Rects,
    },
    /// `ClearUnorderedAccessView*` on a buffer: `value` fills it as 32-bit words.
    FillBuffer {
        va: u64,
        bytes: u64,
        value: u32,
    },
    /// `ClearUnorderedAccessView*` on a whole image in the `GENERAL` layout.
    ClearImage {
        image: ImageRef,
        color: [u32; 4],
        float: bool,
    },
    Draw {
        vertices: u32,
        instances: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        indices: u32,
        instances: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    },
    Dispatch([u32; 3]),
    DispatchMesh([u32; 3]),
    ExecuteIndirect {
        kind: IndirectKind,
        max_count: u32,
        args: u64,
        stride: u32,
        /// A buffer holding the count, which caps `max_count`.
        count: Option<u64>,
    },
    CopyBuffer {
        dst: u64,
        src: u64,
        bytes: u64,
    },
    CopyBufferToImage {
        dst: ImageRef,
        sub: Subresource,
        at: [u32; 3],
        src: BufferFootprint,
        size: [u32; 3],
    },
    CopyImageToBuffer {
        dst: BufferFootprint,
        src: ImageRef,
        sub: Subresource,
        region: TexelBox,
    },
    CopyImage(Box<ImageCopy>),
    ResolveImage(Box<ImageResolve>),
    Barriers(Vec<Barrier>),
    BeginQuery {
        pool: vk::QueryPool,
        kind: QueryKind,
        index: u32,
    },
    EndQuery {
        pool: vk::QueryPool,
        kind: QueryKind,
        index: u32,
    },
    ResolveQueries {
        pool: vk::QueryPool,
        first: u32,
        count: u32,
        dst: u64,
    },
    Marker(String),
}

impl Command {
    /// Whether the command draws or dispatches, which is what the replay's counters count.
    pub fn is_work(&self) -> bool {
        matches!(
            self,
            Command::Draw { .. }
                | Command::DrawIndexed { .. }
                | Command::Dispatch(_)
                | Command::DispatchMesh(_)
                | Command::ExecuteIndirect { .. }
        )
    }

    /// Whether the command only sets state the replay applies at the next draw, so it records nothing and may
    /// sit between barriers that are gathered into one call.
    pub fn is_lazy(&self) -> bool {
        matches!(
            self,
            Command::SetRootSignature { .. }
                | Command::SetPipeline(_)
                | Command::SetRootConstants { .. }
                | Command::SetRootView { .. }
                | Command::SetRootTable { .. }
                | Command::SetDescriptorHeaps(_)
                | Command::SetViewports(_)
                | Command::SetScissors(_)
                | Command::SetBlendFactor(_)
                | Command::SetStencilRef(_)
                | Command::SetTopology(_)
                | Command::SetVertexBuffers { .. }
                | Command::SetIndexBuffer(_)
                | Command::SetRenderTargets { .. }
                | Command::Barriers(_)
                | Command::Marker(_)
        )
    }
}
