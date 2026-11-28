use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;

use ash::vk;
use smallvec::smallvec;

use super::bridge::*;
use super::indirect::IndirectLayout;
use super::pipelines::PendingPipeline;
use crate::com::{self, ComPtr, Kind};
use crate::execute::barrier::{COPY_DEST, PIXEL_SHADER_RESOURCE, RENDER_TARGET, UNORDERED_ACCESS};
use crate::execute::{
    Barrier, BindPoint, BufferFootprint, Command, ImageRef, IndirectKind, Resource, Subresource,
    TargetView,
};
use crate::root::{HeapBases, RootSignature, RootVersion};
use crate::stream::{
    self, Box3, CommandStream, CopyLocation, Footprint, RootView, VertexBufferView,
};

fn object() -> ComPtr {
    let table = com::build_vtable(
        8,
        &[
            (1, com::add_ref as *const () as usize),
            (2, com::release as *const () as usize),
        ],
    );
    let made = com::new_object(Kind::Other, table, ());
    // SAFETY: a fresh object with one reference, which the pointer adopts.
    unsafe { ComPtr::adopt(made as *mut c_void).expect("non-null") }
}

fn key(p: &ComPtr) -> usize {
    p.as_ptr() as usize
}

fn image(mips: u32, layers: u32) -> ImageRef {
    ImageRef {
        image: vk::Image::null(),
        format: vk::Format::R8G8B8A8_UNORM,
        aspect: vk::ImageAspectFlags::COLOR,
        extent: [64, 32, 1],
        mips,
        layers,
    }
}

fn target() -> TargetView {
    TargetView {
        view: vk::ImageView::null(),
        format: vk::Format::R8G8B8A8_UNORM,
        extent: [64, 32],
        samples: vk::SampleCountFlags::TYPE_1,
    }
}

#[derive(Default)]
struct Fake {
    buffers: HashMap<usize, BufferRef>,
    images: HashMap<usize, ImageRef>,
    roots: HashMap<usize, Arc<RootSignature>>,
    hidden: Vec<u64>,
    targets: HashMap<u64, TargetView>,
    pipelines: HashMap<usize, Arc<dyn PendingPipeline>>,
    bundles: HashMap<usize, Arc<CommandStream>>,
    notes: RefCell<Vec<String>>,
    tables: RefCell<Vec<(u32, u64)>>,
}

impl Objects for Fake {
    fn pipeline(&self, object: &ComPtr) -> Option<Arc<dyn PendingPipeline>> {
        self.pipelines.get(&key(object)).cloned()
    }

    fn command_signature(&self, _object: &ComPtr) -> Option<IndirectLayout> {
        Some(IndirectLayout {
            kind: Some(IndirectKind::Draw),
            stride: 16,
        })
    }

    fn bundle(&self, object: &ComPtr) -> Option<Arc<CommandStream>> {
        self.bundles.get(&key(object)).cloned()
    }
}

impl Resolver for Fake {
    fn buffer(&self, resource: &ComPtr) -> Option<BufferRef> {
        self.buffers.get(&key(resource)).copied()
    }

    fn image(&self, resource: &ComPtr) -> Option<ImageRef> {
        self.images.get(&key(resource)).copied()
    }

    fn buffer_containing(&self, address: u64) -> Option<BufferRef> {
        self.buffers
            .values()
            .find(|b| (b.address..b.address + b.size).contains(&address))
            .copied()
    }

    fn root_signature(&self, object: &ComPtr) -> Option<Arc<RootSignature>> {
        self.roots.get(&key(object)).cloned()
    }

    fn heap_bases(&self, heaps: &[ComPtr]) -> HeapBases {
        HeapBases {
            resource: heaps.len() as u32,
            sampler: 0,
        }
    }

    fn color_target(&self, handle: u64) -> Option<TargetView> {
        self.targets.get(&handle).copied()
    }

    fn depth_target(&self, handle: u64) -> Option<TargetView> {
        self.targets.get(&handle).copied()
    }

    fn table_bound(&self, _root: &RootSignature, param: u32, handle: u64) {
        self.tables.borrow_mut().push((param, handle));
    }

    fn skip_draw(&self, buffers: &[u64]) -> bool {
        buffers.iter().any(|b| self.hidden.contains(b))
    }

    fn note(&self, text: &str) {
        self.notes.borrow_mut().push(text.to_string());
    }
}

fn stream(commands: Vec<stream::Command>) -> CommandStream {
    CommandStream {
        list_type: 0,
        commands,
        uploads: Vec::new(),
    }
}

fn empty_root() -> Arc<RootSignature> {
    Arc::new(RootSignature {
        version: RootVersion::V1_1,
        parameters: Vec::new(),
        static_samplers: Vec::new(),
        flags: 0,
    })
}

#[test]
fn buffer_copies_become_copies_between_console_addresses() {
    let (a, b) = (object(), object());
    let mut fake = Fake::default();
    fake.buffers.insert(
        key(&a),
        BufferRef {
            address: 0x1_0000,
            size: 0x1_0000,
        },
    );
    fake.buffers.insert(
        key(&b),
        BufferRef {
            address: 0x8_0000,
            size: 0x1_0000,
        },
    );
    let done = convert(
        &stream(vec![stream::Command::CopyBufferRegion {
            dst: a.clone(),
            dst_offset: 0x40,
            src: b.clone(),
            src_offset: 0x100,
            bytes: 0x200,
        }]),
        &fake,
    );
    assert_eq!(done.writes, vec![(0x1_0040, 0x200)]);
    assert!(matches!(
        done.commands[..],
        [Command::CopyBuffer {
            dst: 0x1_0040,
            src: 0x8_0100,
            bytes: 0x200
        }]
    ));
}

#[test]
fn a_footprint_upload_names_the_subresource_pitch_and_block_rows() {
    let (texture, upload) = (object(), object());
    let mut fake = Fake::default();
    fake.images.insert(key(&texture), image(3, 4));
    fake.buffers.insert(
        key(&upload),
        BufferRef {
            address: 0x20_0000,
            size: 0x10_0000,
        },
    );
    let footprint = Footprint {
        offset: 0x1000,
        format: 28,
        width: 64,
        height: 32,
        depth: 1,
        row_pitch: 256,
    };
    let copy = |src_box: Option<Box3>| {
        convert(
            &stream(vec![stream::Command::CopyTextureRegion(Box::new(
                stream::TextureCopy {
                    dst: CopyLocation::Subresource {
                        resource: texture.clone(),
                        index: 7,
                    },
                    x: 4,
                    y: 8,
                    z: 0,
                    src: CopyLocation::Placed {
                        resource: upload.clone(),
                        footprint,
                    },
                    src_box,
                },
            ))]),
            &fake,
        )
    };
    let whole = copy(None);
    let Command::CopyBufferToImage {
        sub, at, src, size, ..
    } = &whole.commands[0]
    else {
        panic!("{:?}", whole.commands);
    };
    assert_eq!(*sub, Subresource { mip: 1, layer: 2 });
    assert_eq!(*at, [4, 8, 0]);
    assert_eq!(
        *src,
        BufferFootprint {
            va: 0x20_1000,
            row_pitch: 256,
            slice_rows: 32
        }
    );
    assert_eq!(*size, [64, 32, 1]);
    let boxed = copy(Some(Box3 {
        left: 0,
        top: 0,
        front: 0,
        right: 16,
        bottom: 8,
        back: 1,
    }));
    let Command::CopyBufferToImage { size, .. } = &boxed.commands[0] else {
        panic!();
    };
    assert_eq!(*size, [16, 8, 1]);
}

#[test]
fn reading_a_texture_back_records_the_range_the_gpu_writes() {
    let (texture, readback) = (object(), object());
    let mut fake = Fake::default();
    fake.images.insert(key(&texture), image(1, 1));
    fake.buffers.insert(
        key(&readback),
        BufferRef {
            address: 0x40_0000,
            size: 0x10_0000,
        },
    );
    let done = convert(
        &stream(vec![stream::Command::CopyTextureRegion(Box::new(
            stream::TextureCopy {
                dst: CopyLocation::Placed {
                    resource: readback.clone(),
                    footprint: Footprint {
                        offset: 0x100,
                        format: 28,
                        width: 64,
                        height: 32,
                        depth: 1,
                        row_pitch: 256,
                    },
                },
                x: 0,
                y: 0,
                z: 0,
                src: CopyLocation::Subresource {
                    resource: texture.clone(),
                    index: 0,
                },
                src_box: None,
            },
        ))]),
        &fake,
    );
    assert_eq!(done.writes, vec![(0x40_0100, 32 * 256)]);
    assert!(matches!(
        done.commands[0],
        Command::CopyImageToBuffer { .. }
    ));
}

#[test]
fn barriers_keep_sampled_textures_in_the_layout_the_descriptors_read() {
    let (texture, buffer) = (object(), object());
    let mut fake = Fake::default();
    fake.images.insert(key(&texture), image(2, 1));
    fake.buffers.insert(
        key(&buffer),
        BufferRef {
            address: 0x1_0000,
            size: 0x1000,
        },
    );
    let transition = |resource: &ComPtr, before, after, flags| stream::Barrier::Transition {
        resource: resource.clone(),
        subresource: 0xFFFF_FFFF,
        before,
        after,
        flags,
    };
    let done = convert(
        &stream(vec![stream::Command::ResourceBarrier(vec![
            transition(&texture, RENDER_TARGET, PIXEL_SHADER_RESOURCE, 0),
            transition(&texture, COPY_DEST, PIXEL_SHADER_RESOURCE, 1),
            transition(&buffer, COPY_DEST, UNORDERED_ACCESS, 0),
            stream::Barrier::Uav { resource: None },
        ])]),
        &fake,
    );
    let [Command::Barriers(list)] = &done.commands[..] else {
        panic!("{:?}", done.commands);
    };
    assert_eq!(
        list.len(),
        3,
        "the begin half of a split barrier is skipped"
    );
    assert!(matches!(
        list[0],
        Barrier::Transition {
            resource: Resource::Image(_),
            subresource: None,
            before: RENDER_TARGET,
            after,
        } if after == PIXEL_SHADER_RESOURCE | UNORDERED_ACCESS
    ));
    assert!(matches!(
        list[1],
        Barrier::Transition {
            resource: Resource::Buffer,
            ..
        }
    ));
    assert_eq!(list[2], Barrier::Uav);
}

#[test]
fn a_draw_that_reads_a_hidden_buffer_is_dropped_and_a_dispatch_is_not() {
    let mut fake = Fake::default();
    fake.hidden.push(0x7000);
    let list = |vertex: u64| {
        stream(vec![
            stream::Command::SetVertexBuffers {
                start: 0,
                views: smallvec![VertexBufferView {
                    location: vertex,
                    size: 64,
                    stride: 16,
                }],
            },
            stream::Command::DrawInstanced {
                vertex_count: 3,
                instance_count: 1,
                start_vertex: 0,
                start_instance: 0,
            },
            stream::Command::Dispatch {
                x: 1,
                y: 1,
                z: 1,
                extra: None,
            },
        ])
    };
    let hidden = convert(&list(0x7000), &fake);
    assert!(
        hidden
            .commands
            .iter()
            .all(|c| !matches!(c, Command::Draw { .. }))
    );
    assert!(
        hidden
            .commands
            .iter()
            .any(|c| matches!(c, Command::Dispatch(_)))
    );
    let shown = convert(&list(0x9000), &fake);
    assert!(
        shown
            .commands
            .iter()
            .any(|c| matches!(c, Command::Draw { vertices: 3, .. }))
    );
}

#[test]
fn root_state_follows_the_bind_point_and_tables_are_announced() {
    let (root, buffer) = (object(), object());
    let mut fake = Fake::default();
    fake.roots.insert(key(&root), empty_root());
    fake.buffers.insert(
        key(&buffer),
        BufferRef {
            address: 0x5000,
            size: 0x1000,
        },
    );
    let done = convert(
        &stream(vec![
            stream::Command::SetRootSignature {
                compute: true,
                signature: Some(root.clone()),
            },
            stream::Command::SetRootDescriptorTable {
                compute: true,
                index: 2,
                base: 0x3000_0040,
            },
            stream::Command::SetRootConstants {
                compute: false,
                index: 1,
                dest_offset: 3,
                values: smallvec![9, 8],
            },
            stream::Command::SetRootView {
                compute: true,
                view: RootView::UnorderedAccess,
                index: 0,
                address: 0x5100,
            },
        ]),
        &fake,
    );
    assert_eq!(*fake.tables.borrow(), vec![(2, 0x3000_0040)]);
    assert!(matches!(
        done.commands[0],
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            ..
        }
    ));
    assert!(matches!(
        done.commands[2],
        Command::SetRootConstants {
            bind: BindPoint::Graphics,
            param: 1,
            first: 3,
            ..
        }
    ));
    assert_eq!(done.writes, vec![(0x5000, 0x1000)]);
}

#[test]
fn a_single_range_of_render_targets_steps_by_the_descriptor_size() {
    let mut fake = Fake::default();
    for k in 0..3u64 {
        fake.targets.insert(0x1000 + k * 32, target());
    }
    let done = convert(
        &stream(vec![stream::Command::SetRenderTargets {
            rtvs: smallvec![0x1000],
            single_range: true,
            count: 3,
            dsv: None,
        }]),
        &fake,
    );
    let Command::SetRenderTargets { colors, depth } = &done.commands[0] else {
        panic!();
    };
    assert_eq!(colors.len(), 3);
    assert!(depth.is_none());
    assert!(fake.notes.borrow().is_empty());
    let missing = convert(
        &stream(vec![stream::Command::SetRenderTargets {
            rtvs: smallvec![0x1000, 0x9999],
            single_range: false,
            count: 2,
            dsv: None,
        }]),
        &fake,
    );
    let Command::SetRenderTargets { colors, .. } = &missing.commands[0] else {
        panic!();
    };
    assert_eq!(colors.len(), 1);
    assert_eq!(fake.notes.borrow().len(), 1);
}

#[test]
fn clearing_an_unordered_access_view_fills_a_buffer_or_clears_a_texture() {
    let (buffer, texture) = (object(), object());
    let mut fake = Fake::default();
    fake.buffers.insert(
        key(&buffer),
        BufferRef {
            address: 0x6000,
            size: 0x400,
        },
    );
    fake.images.insert(key(&texture), image(1, 1));
    let done = convert(
        &stream(vec![
            stream::Command::ClearUnorderedAccessViewUint {
                gpu: 0,
                cpu: 0,
                resource: buffer.clone(),
                values: [7, 0, 0, 0],
                rects: Default::default(),
            },
            stream::Command::ClearUnorderedAccessViewFloat {
                gpu: 0,
                cpu: 0,
                resource: texture.clone(),
                values: [1.0, 0.5, 0.0, 1.0],
                rects: Default::default(),
            },
        ]),
        &fake,
    );
    assert!(matches!(
        done.commands[0],
        Command::FillBuffer {
            va: 0x6000,
            bytes: 0x400,
            value: 7
        }
    ));
    let Command::ClearImage { color, float, .. } = &done.commands[1] else {
        panic!();
    };
    assert!(float);
    assert_eq!(color[1], 0.5f32.to_bits());
}

#[test]
fn copying_a_whole_texture_copies_every_mip_and_layer() {
    let (a, b) = (object(), object());
    let mut fake = Fake::default();
    fake.images.insert(key(&a), image(3, 2));
    fake.images.insert(key(&b), image(3, 2));
    let done = convert(
        &stream(vec![stream::Command::CopyResource {
            dst: a.clone(),
            src: b.clone(),
        }]),
        &fake,
    );
    assert_eq!(done.commands.len(), 6);
    let Command::CopyImage(copy) = &done.commands[1] else {
        panic!();
    };
    assert_eq!(copy.src_sub, Subresource { mip: 1, layer: 0 });
    assert_eq!(copy.region.size, [32, 16, 1]);
}

#[test]
fn heaps_and_unknown_calls_are_resolved_or_noted() {
    let heap = object();
    let fake = Fake::default();
    let done = convert(
        &stream(vec![
            stream::Command::SetDescriptorHeaps(smallvec![heap.clone()]),
            stream::Command::SetPipelineState(Some(heap.clone())),
            stream::Command::Unrecorded {
                slot: 71,
                method: "WriteValueX",
                args: [0; 4],
            },
        ]),
        &fake,
    );
    assert!(matches!(
        done.commands[0],
        Command::SetDescriptorHeaps(HeapBases { resource: 1, .. })
    ));
    assert_eq!(done.commands.len(), 1);
    assert_eq!(fake.notes.borrow().len(), 2);
}

#[test]
fn enhanced_texture_barriers_become_transitions_between_the_states_their_layouts_mean() {
    let texture = object();
    let mut fake = Fake::default();
    fake.images.insert(key(&texture), image(2, 1));
    let done = convert(
        &stream(vec![stream::Command::EnhancedBarrier(vec![
            stream::BarrierGroup::Texture(vec![stream::TextureBarrier {
                sync_before: 0,
                sync_after: 0,
                access_before: 0,
                access_after: 0,
                layout_before: 2,
                layout_after: 6,
                resource: Some(texture.clone()),
                subresources: [0xFFFF_FFFF, 0, 0, 0, 0, 0],
                flags: 0,
            }]),
            stream::BarrierGroup::Global(vec![stream::GlobalBarrier::default()]),
        ])]),
        &fake,
    );
    let [Command::Barriers(list)] = &done.commands[..] else {
        panic!();
    };
    assert!(matches!(
        list[0],
        Barrier::Transition {
            subresource: None,
            before: RENDER_TARGET,
            ..
        }
    ));
    assert_eq!(list[1], Barrier::Uav);
}

/// A pipeline that is not built, or failed to, and counts how often it was asked.
struct Unbuilt {
    compute: bool,
    failed: bool,
    polls: std::sync::atomic::AtomicUsize,
}

impl Unbuilt {
    fn new(compute: bool, failed: bool) -> Arc<Unbuilt> {
        Arc::new(Unbuilt {
            compute,
            failed,
            polls: Default::default(),
        })
    }

    fn polls(&self) -> usize {
        self.polls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl PendingPipeline for Unbuilt {
    fn compute(&self) -> bool {
        self.compute
    }

    fn resolve(&self) -> crate::compile_queue::Resolved<Arc<crate::pipeline::Pipeline>> {
        self.poll()
    }

    fn poll(&self) -> crate::compile_queue::Resolved<Arc<crate::pipeline::Pipeline>> {
        use crate::compile_queue::Resolved;
        self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.failed {
            Resolved::Failed("no shader".into())
        } else {
            Resolved::NotReady
        }
    }
}

fn draw() -> stream::Command {
    stream::Command::DrawInstanced {
        vertex_count: 3,
        instance_count: 1,
        start_vertex: 0,
        start_instance: 0,
    }
}

fn dispatch() -> stream::Command {
    stream::Command::Dispatch {
        x: 1,
        y: 1,
        z: 1,
        extra: None,
    }
}

fn pipeline_object(fake: &mut Fake, pending: &Arc<Unbuilt>) -> ComPtr {
    let object = object();
    let pending: Arc<dyn PendingPipeline> = pending.clone();
    fake.pipelines.insert(key(&object), pending);
    object
}

#[test]
fn a_draw_whose_pipeline_is_not_built_is_dropped_each_time_it_is_asked() {
    let mut fake = Fake::default();
    let pending = Unbuilt::new(false, false);
    let object = pipeline_object(&mut fake, &pending);
    let done = convert(
        &stream(vec![
            stream::Command::SetPipelineState(Some(object)),
            draw(),
            draw(),
            stream::Command::SetPrimitiveTopology(4),
        ]),
        &fake,
    );
    assert!(
        matches!(done.commands[..], [Command::SetTopology(4)]),
        "{:?}",
        done.commands
    );
    assert_eq!(pending.polls(), 2);
    assert!(fake.notes.borrow().is_empty());
}

#[test]
fn a_pipeline_that_failed_is_noted_and_its_draws_are_dropped() {
    let mut fake = Fake::default();
    let pending = Unbuilt::new(false, true);
    let object = pipeline_object(&mut fake, &pending);
    let done = convert(
        &stream(vec![
            stream::Command::SetPipelineState(Some(object)),
            draw(),
        ]),
        &fake,
    );
    assert!(done.commands.is_empty());
    let notes = fake.notes.borrow();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("no shader"), "{notes:?}");
}

#[test]
fn a_dispatch_asks_for_the_compute_pipeline_and_a_draw_for_the_graphics_one() {
    let mut fake = Fake::default();
    let compute = Unbuilt::new(true, false);
    let object = pipeline_object(&mut fake, &compute);
    let done = convert(
        &stream(vec![
            stream::Command::SetPipelineState(Some(object)),
            draw(),
            dispatch(),
        ]),
        &fake,
    );
    assert!(
        matches!(done.commands[..], [Command::Draw { .. }]),
        "no graphics pipeline was set, so the draw goes through: {:?}",
        done.commands
    );
    assert_eq!(compute.polls(), 1);
}

#[test]
fn only_the_pipelines_draws_and_dispatches_use_are_waited_for_bundles_included() {
    let mut fake = Fake::default();
    let (first, second, unused, inner, replaced) = (
        Unbuilt::new(false, false),
        Unbuilt::new(true, false),
        Unbuilt::new(false, false),
        Unbuilt::new(false, false),
        Unbuilt::new(false, false),
    );
    let (first_object, second_object, unused_object, inner_object, replaced_object) = (
        pipeline_object(&mut fake, &first),
        pipeline_object(&mut fake, &second),
        pipeline_object(&mut fake, &unused),
        pipeline_object(&mut fake, &inner),
        pipeline_object(&mut fake, &replaced),
    );
    let bundle = object();
    fake.bundles.insert(
        key(&bundle),
        Arc::new(stream(vec![
            stream::Command::SetPipelineState(Some(inner_object)),
            draw(),
        ])),
    );
    let list = stream(vec![
        stream::Command::SetPipelineState(Some(replaced_object)),
        stream::Command::SetPipelineState(Some(first_object)),
        draw(),
        draw(),
        stream::Command::SetPipelineState(Some(second_object)),
        dispatch(),
        stream::Command::SetPipelineState(Some(unused_object)),
        stream::Command::ExecuteBundle(bundle),
    ]);
    let used = used_pipelines(&list, &fake);
    let polled: Vec<(bool, usize)> = used.iter().map(|p| (p.compute(), 0)).collect();
    assert_eq!(polled, vec![(false, 0), (true, 0), (false, 0)]);
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&used[0]),
        Arc::as_ptr(&first)
    ));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&used[1]),
        Arc::as_ptr(&second)
    ));
    assert!(std::ptr::addr_eq(
        Arc::as_ptr(&used[2]),
        Arc::as_ptr(&inner)
    ));
}
