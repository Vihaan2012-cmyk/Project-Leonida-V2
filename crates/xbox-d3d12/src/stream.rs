//! The backend-neutral command stream a command list records and the execute step replays.
//!
//! Everything a call refers to is copied: pointer arguments become owned values, and every COM object a
//! command names is held by a counted `ComPtr` until the stream is dropped, so the stream outlives a
//! `Reset` of its list. Descriptor handles stay raw numbers: the CPU handle of a heap entry is the host
//! address of its bytes, the GPU handle the console GPU address (`heap.rs`).

use smallvec::SmallVec;

use crate::com::ComPtr;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Box3 {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VertexBufferView {
    pub location: u64,
    pub size: u32,
    pub stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexBufferView {
    pub location: u64,
    pub size: u32,
    pub format: u32,
}

/// Rectangles, almost always none or one.
pub type Rects = SmallVec<[Rect; 1]>;

/// The viewports of one call: a pass sets one.
pub type Viewports = SmallVec<[Viewport; 2]>;

/// Root constants of one call: up to 16 dwords stay inline, the limit is 64.
pub type RootValues = SmallVec<[u32; 8]>;

/// The vertex buffers of one call: a draw reads a few streams, the limit is 16.
pub type VertexViews = SmallVec<[VertexBufferView; 2]>;

/// The render target handles of one call, at most 8.
pub type TargetHandles = SmallVec<[u64; 4]>;

/// A `D3D12_RESOURCE_BARRIER`.
#[derive(Clone, Debug)]
pub enum Barrier {
    Transition {
        resource: ComPtr,
        subresource: u32,
        before: u32,
        after: u32,
        flags: u32,
    },
    Aliasing {
        before: Option<ComPtr>,
        after: Option<ComPtr>,
    },
    Uav {
        resource: Option<ComPtr>,
    },
}

/// A `D3D12_BARRIER_GROUP` of the enhanced barrier API.
#[derive(Clone, Debug)]
pub enum BarrierGroup {
    Global(Vec<GlobalBarrier>),
    Texture(Vec<TextureBarrier>),
    Buffer(Vec<BufferBarrier>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalBarrier {
    pub sync_before: u32,
    pub sync_after: u32,
    pub access_before: u32,
    pub access_after: u32,
}

#[derive(Clone, Debug)]
pub struct TextureBarrier {
    pub sync_before: u32,
    pub sync_after: u32,
    pub access_before: u32,
    pub access_after: u32,
    pub layout_before: u32,
    pub layout_after: u32,
    pub resource: Option<ComPtr>,
    pub subresources: [u32; 6],
    pub flags: u32,
}

#[derive(Clone, Debug)]
pub struct BufferBarrier {
    pub sync_before: u32,
    pub sync_after: u32,
    pub access_before: u32,
    pub access_after: u32,
    pub resource: Option<ComPtr>,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub offset: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub row_pitch: u32,
}

/// A `D3D12_TEXTURE_COPY_LOCATION`.
#[derive(Clone, Debug)]
pub enum CopyLocation {
    Subresource {
        resource: ComPtr,
        index: u32,
    },
    Placed {
        resource: ComPtr,
        footprint: Footprint,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootView {
    ConstantBuffer,
    ShaderResource,
    UnorderedAccess,
}

/// The arguments of `CopyTextureRegion`, boxed in the command because they are twice the size of any other.
#[derive(Clone, Debug)]
pub struct TextureCopy {
    pub dst: CopyLocation,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub src: CopyLocation,
    pub src_box: Option<Box3>,
}

/// One recorded call, in the order it was made.
#[derive(Clone, Debug)]
pub enum Command {
    ClearState {
        pipeline: Option<ComPtr>,
    },
    DrawInstanced {
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    },
    DrawIndexedInstanced {
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    },
    /// `extra` is the console's fifth word of slot 80 (`0x8020` or `0x20` in the measured callers).
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
        extra: Option<u32>,
    },
    CopyBufferRegion {
        dst: ComPtr,
        dst_offset: u64,
        src: ComPtr,
        src_offset: u64,
        bytes: u64,
    },
    CopyTextureRegion(Box<TextureCopy>),
    CopyResource {
        dst: ComPtr,
        src: ComPtr,
    },
    ResolveSubresource {
        dst: ComPtr,
        dst_subresource: u32,
        src: ComPtr,
        src_subresource: u32,
        format: u32,
    },
    SetPrimitiveTopology(u32),
    SetViewports(Viewports),
    SetScissorRects(Rects),
    SetBlendFactor(Option<[f32; 4]>),
    SetStencilRef(u32),
    SetPipelineState(Option<ComPtr>),
    ResourceBarrier(Vec<Barrier>),
    EnhancedBarrier(Vec<BarrierGroup>),
    SetDescriptorHeaps(SmallVec<[ComPtr; 2]>),
    SetRootSignature {
        compute: bool,
        signature: Option<ComPtr>,
    },
    SetRootDescriptorTable {
        compute: bool,
        index: u32,
        base: u64,
    },
    SetRootConstants {
        compute: bool,
        index: u32,
        dest_offset: u32,
        values: RootValues,
    },
    SetRootView {
        compute: bool,
        view: RootView,
        index: u32,
        address: u64,
    },
    SetIndexBuffer(Option<IndexBufferView>),
    SetVertexBuffers {
        start: u32,
        views: VertexViews,
    },
    /// `D3D12_STREAM_OUTPUT_BUFFER_VIEW`: location, size, filled-size location.
    SetStreamOutputTargets {
        start: u32,
        views: Vec<[u64; 3]>,
    },
    /// With `single_range`, `rtvs` holds the first handle and the count says how many follow at the
    /// descriptor increment.
    SetRenderTargets {
        rtvs: TargetHandles,
        single_range: bool,
        count: u32,
        dsv: Option<u64>,
    },
    ClearDepthStencilView {
        dsv: u64,
        flags: u32,
        depth: f32,
        stencil: u8,
        rects: Rects,
    },
    ClearRenderTargetView {
        rtv: u64,
        color: [f32; 4],
        rects: Rects,
    },
    ClearUnorderedAccessViewUint {
        gpu: u64,
        cpu: u64,
        resource: ComPtr,
        values: [u32; 4],
        rects: Rects,
    },
    ClearUnorderedAccessViewFloat {
        gpu: u64,
        cpu: u64,
        resource: ComPtr,
        values: [f32; 4],
        rects: Rects,
    },
    DiscardResource {
        resource: ComPtr,
        rects: Rects,
        first_subresource: u32,
        subresources: u32,
    },
    ExecuteIndirect {
        signature: ComPtr,
        max_count: u32,
        arguments: ComPtr,
        arguments_offset: u64,
        count: Option<ComPtr>,
        count_offset: u64,
    },
    ExecuteBundle(ComPtr),
    /// Console slot 59: a GPU event marker, `begin` the leading byte argument and `data` the blob.
    GpuEvent {
        begin: bool,
        data: SmallVec<[u8; 32]>,
    },
    /// Console slot 141: `(2, GPU address, size, n)` binds the compute scratch buffer.
    SetScratchBuffer {
        kind: u32,
        address: u64,
        size: u64,
        value: u64,
    },
    /// A call whose slot is known but whose arguments this layer does not decode yet.
    Unrecorded {
        slot: usize,
        method: &'static str,
        args: [usize; 4],
    },
}

/// What a closed list hands to the queue.
#[derive(Debug, Default)]
pub struct CommandStream {
    pub list_type: u32,
    pub commands: Vec<Command>,
    /// Memory handed out by `AllocateUploadMemoryX` for this list; the stream keeps it alive.
    pub uploads: Vec<Box<[u8]>>,
}
