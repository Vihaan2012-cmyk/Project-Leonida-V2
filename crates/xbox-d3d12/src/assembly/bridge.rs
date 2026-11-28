//! A closed command list (`stream::CommandStream`: what the title called, with the COM objects it named) as the
//! commands `execute::Replayer` plays (`execute::Command`: buffers by console address, images and views by Vulkan
//! handle, pipelines and root signatures as built objects).
//!
//! Everything that needs the device's state, such as which texture a COM pointer is or which image a render target
//! descriptor names, comes through `Resolver`, so the conversion itself runs without a GPU.

use std::sync::Arc;

use smallvec::SmallVec;

use super::indirect::IndirectLayout;
use crate::com::ComPtr;
use crate::execute::barrier::{
    DEPTH_READ, DEPTH_WRITE, NON_PIXEL_SHADER_RESOURCE, PIXEL_SHADER_RESOURCE, RENDER_TARGET,
    UNORDERED_ACCESS,
};
use crate::execute::{
    Barrier, BindPoint, BufferFootprint, Command, ImageRef, IndexBufferView, IndirectKind, Rect,
    Rects, Resource, Subresource, TargetView, TexelBox, VertexBufferView, VertexViews, Viewport,
};
use crate::heap;
use crate::pipeline::Pipeline;
use crate::pipeline::state::block_size;
use crate::root::{HeapBases, RootSignature};
use crate::stream::{self, CommandStream, CopyLocation, RootView};

/// A buffer resource: the console address its memory is at and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRef {
    pub address: u64,
    pub size: u64,
}

/// What the conversion asks of the device it runs for.
pub trait Resolver {
    fn buffer(&self, resource: &ComPtr) -> Option<BufferRef>;
    fn image(&self, resource: &ComPtr) -> Option<ImageRef>;
    /// The buffer whose address range holds `address`.
    fn buffer_containing(&self, address: u64) -> Option<BufferRef>;
    fn root_signature(&self, object: &ComPtr) -> Option<Arc<RootSignature>>;
    fn pipeline(&self, object: &ComPtr) -> Option<Arc<Pipeline>>;
    fn command_signature(&self, object: &ComPtr) -> Option<IndirectLayout>;
    fn heap_bases(&self, heaps: &[ComPtr]) -> HeapBases;
    /// The render target view the descriptor at this CPU handle names.
    fn color_target(&self, handle: u64) -> Option<TargetView>;
    fn depth_target(&self, handle: u64) -> Option<TargetView>;
    fn bundle(&self, object: &ComPtr) -> Option<Arc<CommandStream>>;
    /// A descriptor table was bound for `root`'s parameter `param`: the descriptors it can reach need
    /// registering before the draw runs.
    fn table_bound(&self, root: &RootSignature, param: u32, handle: u64) {
        let _ = (root, param, handle);
    }
    /// Whether a draw reading these buffer addresses is dropped (the content filter).
    fn skip_draw(&self, buffers: &[u64]) -> bool {
        let _ = buffers;
        false
    }
    fn note(&self, text: &str) {
        crate::com::note_once(text.to_string());
    }
}

/// What converting one stream produced.
#[derive(Debug, Default)]
pub struct Converted {
    pub commands: Vec<Command>,
    /// Address ranges the GPU writes, for copying back to host memory when the address space is mirrored.
    pub writes: Vec<(u64, u64)>,
}

#[derive(Default)]
struct State {
    graphics_root: Option<Arc<RootSignature>>,
    compute_root: Option<Arc<RootSignature>>,
    vertex: Vec<Option<VertexBufferView>>,
    index: Option<IndexBufferView>,
}

fn bind(compute: bool) -> BindPoint {
    if compute {
        BindPoint::Compute
    } else {
        BindPoint::Graphics
    }
}

/// `D3D12_RESOURCE_STATES` that put an image in the layout the bindless image set reads it in: a state that
/// samples or reads a texture also names unordered access, which maps to `GENERAL`.
pub fn adjust_state(state: u32) -> u32 {
    let sampled = state & (PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE) != 0;
    let attachment = state & (RENDER_TARGET | DEPTH_WRITE | DEPTH_READ) != 0;
    if sampled && !attachment {
        state | UNORDERED_ACCESS
    } else {
        state
    }
}

fn subresource(image: &ImageRef, index: u32) -> Subresource {
    let mips = image.mips.max(1);
    Subresource {
        mip: index % mips,
        layer: index / mips,
    }
}

fn texel_box(b: &stream::Box3) -> TexelBox {
    TexelBox {
        origin: [b.left, b.top, b.front],
        size: [
            b.right.saturating_sub(b.left),
            b.bottom.saturating_sub(b.top),
            b.back.saturating_sub(b.front),
        ],
    }
}

fn rects(list: &[stream::Rect]) -> Rects {
    list.iter()
        .map(|r| Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        })
        .collect()
}

/// The `D3D12_BARRIER_LAYOUT` of the enhanced barrier API as the legacy state that means the same.
fn layout_state(layout: u32) -> u32 {
    use crate::execute::barrier::*;
    match layout {
        0 | 0xFFFF_FFFF => 0,
        1 => {
            VERTEX_AND_CONSTANT_BUFFER
                | INDEX_BUFFER
                | NON_PIXEL_SHADER_RESOURCE
                | PIXEL_SHADER_RESOURCE
                | COPY_SOURCE
        }
        2 => RENDER_TARGET,
        3 => UNORDERED_ACCESS,
        4 => DEPTH_WRITE,
        5 => DEPTH_READ,
        6 => NON_PIXEL_SHADER_RESOURCE | PIXEL_SHADER_RESOURCE,
        7 => COPY_SOURCE,
        8 => COPY_DEST,
        9 => RESOLVE_SOURCE,
        10 => RESOLVE_DEST,
        _ => 0,
    }
}

pub fn convert(stream: &CommandStream, resolver: &dyn Resolver) -> Converted {
    let mut out = Converted::default();
    let mut state = State::default();
    convert_into(stream, resolver, &mut state, &mut out, 0);
    out
}

fn convert_into(
    stream: &CommandStream,
    r: &dyn Resolver,
    state: &mut State,
    out: &mut Converted,
    depth: u32,
) {
    use stream::Command as C;
    for command in &stream.commands {
        match command {
            C::ClearState { pipeline } => {
                *state = State::default();
                if let Some(p) = pipeline.as_ref().and_then(|p| r.pipeline(p)) {
                    out.commands.push(Command::SetPipeline(p));
                }
            }
            C::SetPipelineState(pipeline) => match pipeline {
                Some(p) => match r.pipeline(p) {
                    Some(p) => out.commands.push(Command::SetPipeline(p)),
                    None => r.note("SetPipelineState named an object that is not a pipeline state"),
                },
                None => r.note("SetPipelineState(null) was recorded and ignored"),
            },
            C::DrawInstanced {
                vertex_count,
                instance_count,
                start_vertex,
                start_instance,
            } => {
                if !skipped(state, r) {
                    out.commands.push(Command::Draw {
                        vertices: *vertex_count,
                        instances: *instance_count,
                        first_vertex: *start_vertex,
                        first_instance: *start_instance,
                    });
                }
            }
            C::DrawIndexedInstanced {
                index_count,
                instance_count,
                start_index,
                base_vertex,
                start_instance,
            } => {
                if !skipped(state, r) {
                    out.commands.push(Command::DrawIndexed {
                        indices: *index_count,
                        instances: *instance_count,
                        first_index: *start_index,
                        base_vertex: *base_vertex,
                        first_instance: *start_instance,
                    });
                }
            }
            C::Dispatch { x, y, z, .. } => out.commands.push(Command::Dispatch([*x, *y, *z])),
            C::CopyBufferRegion {
                dst,
                dst_offset,
                src,
                src_offset,
                bytes,
            } => match (r.buffer(dst), r.buffer(src)) {
                (Some(d), Some(s)) => {
                    out.writes.push((d.address + dst_offset, *bytes));
                    out.commands.push(Command::CopyBuffer {
                        dst: d.address + dst_offset,
                        src: s.address + src_offset,
                        bytes: *bytes,
                    });
                }
                _ => r.note("CopyBufferRegion between resources that are not both buffers"),
            },
            C::CopyTextureRegion {
                dst,
                x,
                y,
                z,
                src,
                src_box,
            } => copy_texture(r, out, dst, [*x, *y, *z], src, src_box.as_ref()),
            C::CopyResource { dst, src } => copy_resource(r, out, dst, src),
            C::ResolveSubresource {
                dst,
                dst_subresource,
                src,
                src_subresource,
                ..
            } => match (r.image(dst), r.image(src)) {
                (Some(d), Some(s)) => out.commands.push(Command::ResolveImage {
                    dst: d,
                    dst_sub: subresource(&d, *dst_subresource),
                    src: s,
                    src_sub: subresource(&s, *src_subresource),
                }),
                _ => r.note("ResolveSubresource between resources that are not both textures"),
            },
            C::SetPrimitiveTopology(topology) => out.commands.push(Command::SetTopology(*topology)),
            C::SetViewports(list) => out.commands.push(Command::SetViewports(
                list.iter()
                    .map(|v| Viewport {
                        x: v.x,
                        y: v.y,
                        width: v.width,
                        height: v.height,
                        min_depth: v.min_depth,
                        max_depth: v.max_depth,
                    })
                    .collect(),
            )),
            C::SetScissorRects(list) => out.commands.push(Command::SetScissors(rects(list))),
            C::SetBlendFactor(factor) => out
                .commands
                .push(Command::SetBlendFactor(factor.unwrap_or([1.0; 4]))),
            C::SetStencilRef(value) => out.commands.push(Command::SetStencilRef(*value)),
            C::ResourceBarrier(list) => resource_barriers(r, out, list),
            C::EnhancedBarrier(groups) => enhanced_barriers(r, out, groups),
            C::SetDescriptorHeaps(heaps) => out
                .commands
                .push(Command::SetDescriptorHeaps(r.heap_bases(heaps))),
            C::SetRootSignature { compute, signature } => {
                let Some(root) = signature.as_ref().and_then(|s| r.root_signature(s)) else {
                    r.note("SetRootSignature named an object that is not a root signature");
                    continue;
                };
                if *compute {
                    state.compute_root = Some(root.clone());
                } else {
                    state.graphics_root = Some(root.clone());
                }
                out.commands.push(Command::SetRootSignature {
                    bind: bind(*compute),
                    root,
                });
            }
            C::SetRootDescriptorTable {
                compute,
                index,
                base,
            } => {
                let root = if *compute {
                    &state.compute_root
                } else {
                    &state.graphics_root
                };
                if let Some(root) = root {
                    r.table_bound(root, *index, *base);
                }
                out.commands.push(Command::SetRootTable {
                    bind: bind(*compute),
                    param: *index,
                    handle: *base,
                });
            }
            C::SetRootConstants {
                compute,
                index,
                dest_offset,
                values,
            } => out.commands.push(Command::SetRootConstants {
                bind: bind(*compute),
                param: *index,
                first: *dest_offset,
                values: values.clone(),
            }),
            C::SetRootView {
                compute,
                view,
                index,
                address,
            } => {
                if *view == RootView::UnorderedAccess
                    && let Some(b) = r.buffer_containing(*address)
                {
                    out.writes.push((b.address, b.size));
                }
                out.commands.push(Command::SetRootView {
                    bind: bind(*compute),
                    param: *index,
                    va: *address,
                });
            }
            C::SetIndexBuffer(view) => {
                state.index = view.map(|v| IndexBufferView {
                    va: v.location,
                    size: v.size,
                    format: v.format,
                });
                out.commands.push(Command::SetIndexBuffer(state.index));
            }
            C::SetVertexBuffers { start, views } => {
                let views: VertexViews = views
                    .iter()
                    .map(|v| VertexBufferView {
                        va: v.location,
                        size: v.size,
                        stride: v.stride,
                    })
                    .collect();
                let end = *start as usize + views.len();
                if state.vertex.len() < end {
                    state.vertex.resize(end, None);
                }
                for (k, v) in views.iter().enumerate() {
                    state.vertex[*start as usize + k] = Some(*v);
                }
                out.commands.push(Command::SetVertexBuffers {
                    start: *start,
                    views,
                });
            }
            C::SetStreamOutputTargets { .. } => {
                r.note("SOSetTargets was recorded; stream output is not replayed");
            }
            C::SetRenderTargets {
                rtvs,
                single_range,
                count,
                dsv,
            } => {
                let step = u64::from(heap::increment_size(heap::TYPE_RTV));
                let handles: SmallVec<[u64; 8]> = if *single_range {
                    rtvs.first()
                        .map(|first| (0..u64::from(*count)).map(|k| first + k * step).collect())
                        .unwrap_or_default()
                } else {
                    rtvs.clone()
                };
                let colors: SmallVec<[TargetView; 2]> =
                    handles.iter().filter_map(|h| r.color_target(*h)).collect();
                if colors.len() != handles.len() {
                    r.note("a render target view names no texture; it is left out");
                }
                let depth = dsv.and_then(|h| r.depth_target(h));
                out.commands
                    .push(Command::SetRenderTargets { colors, depth });
            }
            C::ClearDepthStencilView {
                dsv,
                flags,
                depth,
                stencil,
                rects: list,
            } => match r.depth_target(*dsv) {
                Some(view) => out.commands.push(Command::ClearDepthStencil {
                    view,
                    depth: (flags & 1 != 0).then_some(*depth),
                    stencil: (flags & 2 != 0).then_some(*stencil),
                    rects: rects(list),
                }),
                None => r.note("ClearDepthStencilView names no depth texture"),
            },
            C::ClearRenderTargetView {
                rtv,
                color,
                rects: list,
            } => match r.color_target(*rtv) {
                Some(view) => out.commands.push(Command::ClearRenderTarget {
                    view,
                    color: *color,
                    rects: rects(list),
                }),
                None => r.note("ClearRenderTargetView names no texture"),
            },
            C::ClearUnorderedAccessViewUint {
                resource, values, ..
            } => clear_uav(r, out, resource, *values, false),
            C::ClearUnorderedAccessViewFloat {
                resource, values, ..
            } => clear_uav(r, out, resource, values.map(f32::to_bits), true),
            C::DiscardResource { .. } | C::GpuEvent { .. } | C::SetScratchBuffer { .. } => {}
            C::ExecuteIndirect {
                signature,
                max_count,
                arguments,
                arguments_offset,
                count,
                count_offset,
            } => {
                let (Some(layout), Some(args)) =
                    (r.command_signature(signature), r.buffer(arguments))
                else {
                    r.note("ExecuteIndirect named a signature or buffer this layer does not know");
                    continue;
                };
                let Some(kind) = layout.kind else {
                    r.note("ExecuteIndirect with a command signature that has no draw or dispatch");
                    continue;
                };
                let count = count
                    .as_ref()
                    .and_then(|c| r.buffer(c))
                    .map(|c| c.address + count_offset);
                if kind != IndirectKind::Dispatch && skipped(state, r) {
                    continue;
                }
                out.commands.push(Command::ExecuteIndirect {
                    kind,
                    max_count: *max_count,
                    args: args.address + arguments_offset,
                    stride: layout.stride,
                    count,
                });
            }
            C::ExecuteBundle(bundle) => {
                if depth > 4 {
                    r.note("bundles nested deeper than four levels were not followed");
                } else if let Some(inner) = r.bundle(bundle) {
                    convert_into(&inner, r, state, out, depth + 1);
                }
            }
            C::Unrecorded { slot, method, .. } => {
                r.note(&format!(
                    "{method} (slot {slot}) was recorded and not replayed"
                ));
            }
        }
    }
}

/// Whether the draw about to be recorded reads a buffer the content filter hides.
fn skipped(state: &State, r: &dyn Resolver) -> bool {
    let mut addresses: SmallVec<[u64; 17]> = state.vertex.iter().flatten().map(|v| v.va).collect();
    if let Some(index) = state.index {
        addresses.push(index.va);
    }
    r.skip_draw(&addresses)
}

fn copy_texture(
    r: &dyn Resolver,
    out: &mut Converted,
    dst: &CopyLocation,
    at: [u32; 3],
    src: &CopyLocation,
    src_box: Option<&stream::Box3>,
) {
    match (dst, src) {
        (
            CopyLocation::Subresource {
                resource: image,
                index,
            },
            CopyLocation::Placed {
                resource: buffer,
                footprint,
            },
        ) => {
            let (Some(image), Some(buffer)) = (r.image(image), r.buffer(buffer)) else {
                r.note("CopyTextureRegion from a footprint to something that is not a texture");
                return;
            };
            let block = block_size(image.format).max(1);
            let size = match src_box {
                Some(b) => texel_box(b).size,
                None => [footprint.width, footprint.height, footprint.depth],
            };
            out.commands.push(Command::CopyBufferToImage {
                dst: image,
                sub: subresource(&image, *index),
                at,
                src: BufferFootprint {
                    va: buffer.address + footprint.offset,
                    row_pitch: footprint.row_pitch,
                    slice_rows: footprint.height.div_ceil(block),
                },
                size,
            });
        }
        (
            CopyLocation::Placed {
                resource: buffer,
                footprint,
            },
            CopyLocation::Subresource {
                resource: image,
                index,
            },
        ) => {
            let (Some(image), Some(buffer)) = (r.image(image), r.buffer(buffer)) else {
                r.note("CopyTextureRegion from a texture to something that is not a footprint");
                return;
            };
            let sub = subresource(&image, *index);
            let mip = image.mip_extent(sub.mip);
            let region = match src_box {
                Some(b) => texel_box(b),
                None => TexelBox {
                    origin: [0; 3],
                    size: mip,
                },
            };
            let block = block_size(image.format).max(1);
            let rows = u64::from(region.size[1].div_ceil(block)) * u64::from(region.size[2].max(1));
            out.writes.push((
                buffer.address + footprint.offset,
                rows * u64::from(footprint.row_pitch),
            ));
            out.commands.push(Command::CopyImageToBuffer {
                dst: BufferFootprint {
                    va: buffer.address + footprint.offset,
                    row_pitch: footprint.row_pitch,
                    slice_rows: footprint.height.div_ceil(block),
                },
                src: image,
                sub,
                region,
            });
        }
        (
            CopyLocation::Subresource {
                resource: d,
                index: di,
            },
            CopyLocation::Subresource {
                resource: s,
                index: si,
            },
        ) => {
            let (Some(d), Some(s)) = (r.image(d), r.image(s)) else {
                r.note("CopyTextureRegion between resources that are not both textures");
                return;
            };
            let src_sub = subresource(&s, *si);
            let region = match src_box {
                Some(b) => texel_box(b),
                None => TexelBox {
                    origin: [0; 3],
                    size: s.mip_extent(src_sub.mip),
                },
            };
            out.commands.push(Command::CopyImage {
                dst: d,
                dst_sub: subresource(&d, *di),
                at,
                src: s,
                src_sub,
                region,
            });
        }
        _ => r.note("CopyTextureRegion between two footprints"),
    }
}

fn copy_resource(r: &dyn Resolver, out: &mut Converted, dst: &ComPtr, src: &ComPtr) {
    if let (Some(d), Some(s)) = (r.buffer(dst), r.buffer(src)) {
        let bytes = d.size.min(s.size);
        out.writes.push((d.address, bytes));
        out.commands.push(Command::CopyBuffer {
            dst: d.address,
            src: s.address,
            bytes,
        });
        return;
    }
    let (Some(d), Some(s)) = (r.image(dst), r.image(src)) else {
        r.note("CopyResource between a buffer and a texture");
        return;
    };
    for layer in 0..d.layers.min(s.layers) {
        for mip in 0..d.mips.min(s.mips) {
            let sub = Subresource { mip, layer };
            out.commands.push(Command::CopyImage {
                dst: d,
                dst_sub: sub,
                at: [0; 3],
                src: s,
                src_sub: sub,
                region: TexelBox {
                    origin: [0; 3],
                    size: s.mip_extent(mip),
                },
            });
        }
    }
}

fn clear_uav(
    r: &dyn Resolver,
    out: &mut Converted,
    resource: &ComPtr,
    values: [u32; 4],
    float: bool,
) {
    if let Some(buffer) = r.buffer(resource) {
        out.writes.push((buffer.address, buffer.size));
        out.commands.push(Command::FillBuffer {
            va: buffer.address,
            bytes: buffer.size & !3,
            value: values[0],
        });
    } else if let Some(image) = r.image(resource) {
        out.commands.push(Command::ClearImage {
            image,
            color: values,
            float,
        });
    } else {
        r.note("ClearUnorderedAccessView on an object that is neither a buffer nor a texture");
    }
}

fn resource_barriers(r: &dyn Resolver, out: &mut Converted, list: &[stream::Barrier]) {
    let mut built = Vec::new();
    for barrier in list {
        match barrier {
            stream::Barrier::Transition {
                resource,
                subresource: index,
                before,
                after,
                flags,
            } => {
                // BEGIN_ONLY of a split barrier: the transition happens at its END_ONLY half.
                if flags & 1 != 0 {
                    continue;
                }
                if let Some(image) = r.image(resource) {
                    built.push(Barrier::Transition {
                        resource: Resource::Image(image),
                        subresource: (*index != 0xFFFF_FFFF).then(|| subresource(&image, *index)),
                        before: adjust_state(*before),
                        after: adjust_state(*after),
                    });
                } else if r.buffer(resource).is_some() {
                    built.push(Barrier::Transition {
                        resource: Resource::Buffer,
                        subresource: None,
                        before: *before,
                        after: *after,
                    });
                }
            }
            stream::Barrier::Aliasing { .. } => built.push(Barrier::Aliasing),
            stream::Barrier::Uav { .. } => built.push(Barrier::Uav),
        }
    }
    if !built.is_empty() {
        out.commands.push(Command::Barriers(built));
    }
}

fn enhanced_barriers(r: &dyn Resolver, out: &mut Converted, groups: &[stream::BarrierGroup]) {
    let mut built = Vec::new();
    for group in groups {
        match group {
            stream::BarrierGroup::Global(list) if !list.is_empty() => built.push(Barrier::Uav),
            stream::BarrierGroup::Buffer(list) if !list.is_empty() => built.push(Barrier::Uav),
            stream::BarrierGroup::Texture(list) => {
                for t in list {
                    let Some(image) = t.resource.as_ref().and_then(|p| r.image(p)) else {
                        built.push(Barrier::Uav);
                        continue;
                    };
                    let [first, mips, layer, layers, ..] = t.subresources;
                    let whole = if mips == 0 {
                        first == 0xFFFF_FFFF
                    } else {
                        first == 0 && mips >= image.mips && layer == 0 && layers >= image.layers
                    };
                    let sub = if mips == 0 {
                        subresource(&image, first)
                    } else {
                        Subresource { mip: first, layer }
                    };
                    built.push(Barrier::Transition {
                        resource: Resource::Image(image),
                        subresource: (!whole).then_some(sub),
                        before: adjust_state(layout_state(t.layout_before)),
                        after: adjust_state(layout_state(t.layout_after)),
                    });
                }
            }
            _ => {}
        }
    }
    if !built.is_empty() {
        out.commands.push(Command::Barriers(built));
    }
}
