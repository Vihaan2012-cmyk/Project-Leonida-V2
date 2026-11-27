use super::*;
use crate::root::RootVersion;

/// Writes subobjects the way d3dx12 lays them out.
#[derive(Default)]
struct Builder {
    bytes: Vec<u8>,
}

impl Builder {
    fn push(&mut self, ty: u32, alignment: usize, data: &[u8]) -> &mut Self {
        while !self.bytes.len().is_multiple_of(8) {
            self.bytes.push(0);
        }
        self.bytes.extend_from_slice(&ty.to_le_bytes());
        while !self.bytes.len().is_multiple_of(alignment) {
            self.bytes.push(0);
        }
        self.bytes.extend_from_slice(data);
        self
    }

    fn words(&mut self, ty: u32, words: &[u32]) -> &mut Self {
        let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.push(ty, 4, &data)
    }

    fn pointer_pair(&mut self, ty: u32, pointer: u64, length: u64) -> &mut Self {
        let mut data = pointer.to_le_bytes().to_vec();
        data.extend_from_slice(&length.to_le_bytes());
        self.push(ty, 8, &data)
    }
}

fn memory_with_code() -> RegionMemory {
    let mut memory = RegionMemory::default();
    memory.insert(0x1000, vec![1, 2, 3, 4]);
    memory.insert(0x2000, vec![9, 8, 7]);
    memory.insert(0x3000, b"POSITION\0TEXCOORD\0".to_vec());
    memory
}

fn parse(builder: &Builder) -> Result<PipelineStream, StreamError> {
    PipelineStream::parse(&builder.bytes, &memory_with_code(), Dialect::Pc)
}

#[test]
fn root_signature_and_shader_stages_read_their_bytes() {
    let mut b = Builder::default();
    b.push(0, 8, &0xdead_0000u64.to_le_bytes())
        .pointer_pair(1, 0x1000, 4)
        .pointer_pair(2, 0x2000, 3)
        .pointer_pair(3, 0, 0);
    let s = parse(&b).unwrap();
    assert_eq!(s.root_signature, Some(0xdead_0000));
    assert_eq!(s.vs.0, [1, 2, 3, 4]);
    assert_eq!(s.ps.0, [9, 8, 7]);
    assert!(s.ds.is_empty());
    assert_eq!(s.order, [0, 1, 2, 3]);
}

#[test]
fn every_classic_stage_type_lands_in_its_own_field() {
    let mut b = Builder::default();
    for (ty, length) in [(4u32, 4u64), (5, 3), (6, 4)] {
        b.pointer_pair(ty, if length == 3 { 0x2000 } else { 0x1000 }, length);
    }
    let s = parse(&b).unwrap();
    assert_eq!(s.hs.0.len(), 4);
    assert_eq!(s.gs.0.len(), 3);
    assert_eq!(s.cs.0.len(), 4);
}

#[test]
fn a_shader_with_a_null_pointer_or_no_length_is_absent() {
    let mut b = Builder::default();
    b.pointer_pair(1, 0, 16).pointer_pair(2, 0x1000, 0);
    let s = parse(&b).unwrap();
    assert!(s.vs.is_empty() && s.ps.is_empty());
}

#[test]
fn a_shader_pointer_that_leads_nowhere_is_an_error() {
    let mut b = Builder::default();
    b.pointer_pair(1, 0x9000, 4);
    assert_eq!(parse(&b).unwrap_err(), StreamError::BadPointer(1));
}

#[test]
fn blend_state_reads_all_eight_targets() {
    let mut words = vec![1u32, 1];
    for i in 0..8u32 {
        words.extend([1, 0, 5 + i, 6, 1, 2, 3, 4, 11]);
        words.push(0x0000_000f - i);
    }
    assert_eq!(words.len() * 4, 8 + 8 * 40);
    let mut b = Builder::default();
    b.words(8, &words);
    let s = parse(&b).unwrap();
    let blend = s.blend.unwrap();
    assert!(blend.alpha_to_coverage && blend.independent);
    assert_eq!(blend.targets[3].src, 8);
    assert_eq!(blend.targets[3].write_mask, 12);
    assert!(blend.targets[7].blend_enable && !blend.targets[7].logic_enable);
    assert_eq!(blend.targets[7].logic_op, 11);
}

#[test]
fn blend_description_sits_at_offset_four_after_its_type() {
    let words = vec![0u32; 82];
    let mut b = Builder::default();
    b.words(8, &words).words(9, &[0xffff_0000]);
    assert_eq!(b.bytes.len(), 344);
    let s = parse(&b).unwrap();
    assert_eq!(s.sample_mask, Some(0xffff_0000));
}

#[test]
fn rasterizer_reads_integer_bias() {
    let mut b = Builder::default();
    b.words(
        10,
        &[
            2,
            1,
            1,
            (-4i32) as u32,
            0.5f32.to_bits(),
            2.0f32.to_bits(),
            0,
            1,
            1,
            4,
            1,
        ],
    );
    let s = parse(&b).unwrap();
    let r = s.rasterizer.unwrap();
    assert_eq!((r.fill, r.cull, r.front_ccw), (2, 1, true));
    assert_eq!(r.depth_bias, -4);
    assert_eq!(r.depth_bias_clamp, 0.5);
    assert_eq!(r.slope_scaled_bias, 2.0);
    assert!(!r.depth_clip && r.multisample && r.conservative);
    assert_eq!(r.forced_sample_count, 4);
    assert!(s.rasterizer_extra.antialiased_line);
}

#[test]
fn rasterizer1_truncates_the_float_bias_and_keeps_it() {
    let mut b = Builder::default();
    b.words(27, &[3, 3, 0, 2.75f32.to_bits(), 0, 0, 1, 0, 0, 0, 0]);
    let s = parse(&b).unwrap();
    assert_eq!(s.rasterizer.unwrap().depth_bias, 2);
    assert_eq!(s.rasterizer_extra.depth_bias_float, Some(2.75));
    assert!(s.unsupported().contains(&"fractional depth bias"));
}

#[test]
fn rasterizer2_has_ten_words_and_a_line_mode() {
    let mut b = Builder::default();
    b.words(28, &[3, 2, 1, 0, 0, 0, 1, 3, 8, 1]);
    let s = parse(&b).unwrap();
    let r = s.rasterizer.unwrap();
    assert_eq!((r.cull, r.front_ccw, r.forced_sample_count), (2, true, 8));
    assert!(r.conservative);
    assert_eq!(s.rasterizer_extra.line_mode, Some(3));
}

fn depth_words(extra: Option<u32>) -> Vec<u32> {
    let mut words = vec![1, 1, 4, 1, 0x0000_aa55];
    words.extend([3, 2, 4, 8]);
    words.extend([1, 1, 1, 2]);
    words.extend(extra);
    words
}

#[test]
fn depth_stencil_reads_masks_and_both_faces() {
    let mut b = Builder::default();
    b.words(11, &depth_words(None));
    let s = parse(&b).unwrap();
    let d = s.depth_stencil.unwrap();
    assert!(d.depth_enable && d.depth_write && d.stencil_enable);
    assert_eq!(d.depth_func, 4);
    assert_eq!((d.stencil_read_mask, d.stencil_write_mask), (0x55, 0xaa));
    assert_eq!(
        d.front,
        StencilFace {
            fail: 3,
            depth_fail: 2,
            pass: 4,
            func: 8
        }
    );
    assert_eq!(
        d.back,
        StencilFace {
            fail: 1,
            depth_fail: 1,
            pass: 1,
            func: 2
        }
    );
}

#[test]
fn depth_stencil1_adds_the_bounds_test() {
    let mut b = Builder::default();
    b.words(21, &depth_words(Some(1)));
    let s = parse(&b).unwrap();
    assert!(s.depth_stencil_extra.depth_bounds_test);
    assert_eq!(s.unsupported(), ["depth bounds test"]);
}

#[test]
fn depth_stencil2_has_masks_per_face() {
    let mut b = Builder::default();
    let words = [
        1,
        0,
        2,
        1,
        3,
        3,
        3,
        8,
        0x0000_0f0f,
        1,
        1,
        1,
        5,
        0x0000_3c3c,
        0,
    ];
    b.words(26, &words);
    let s = parse(&b).unwrap();
    let d = s.depth_stencil.unwrap();
    assert!(!d.depth_write);
    assert_eq!((d.stencil_read_mask, d.stencil_write_mask), (0x0f, 0x0f));
    assert_eq!(d.back.func, 5);
    assert_eq!(
        s.depth_stencil_extra.face_masks,
        Some([(0x0f, 0x0f), (0x3c, 0x3c)])
    );
    assert!(
        s.unsupported()
            .contains(&"separate front and back stencil masks")
    );
}

#[test]
fn input_layout_follows_element_and_name_pointers() {
    let element = |name: u64, index: u32, format: u32, slot: u32, offset: u32, class: u32| {
        let mut bytes = name.to_le_bytes().to_vec();
        for word in [index, format, slot, offset, class, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    };
    let mut elements = element(0x3000, 0, 6, 0, 0xffff_ffff, 0);
    elements.extend(element(0x3009, 1, 16, 1, 8, 1));
    let mut memory = memory_with_code();
    memory.insert(0x4000, elements);
    let mut data = 0x4000u64.to_le_bytes().to_vec();
    data.extend_from_slice(&2u32.to_le_bytes());
    let mut b = Builder::default();
    b.push(12, 8, &data);
    let s = PipelineStream::parse(&b.bytes, &memory, Dialect::Pc).unwrap();
    let layout = s.input_layout.unwrap();
    assert_eq!(layout.len(), 2);
    assert_eq!(layout[0].semantic, "POSITION");
    assert_eq!(layout[0].offset, 0xffff_ffff);
    assert_eq!(layout[1].semantic, "TEXCOORD");
    assert_eq!(
        (layout[1].semantic_index, layout[1].format, layout[1].slot),
        (1, 16, 1)
    );
    assert_eq!((layout[1].offset, layout[1].class), (8, 1));
}

#[test]
fn an_empty_input_layout_is_present_and_empty() {
    let mut b = Builder::default();
    b.push(12, 8, &[0; 12]);
    assert_eq!(parse(&b).unwrap().input_layout, Some(Vec::new()));
}

#[test]
fn scalar_subobjects() {
    let mut b = Builder::default();
    b.words(13, &[2])
        .words(14, &[3])
        .words(16, &[40])
        .words(17, &[4, 1])
        .words(18, &[1])
        .words(20, &[0x2]);
    let s = parse(&b).unwrap();
    assert_eq!(s.strip_cut, Some(2));
    assert_eq!(s.topology, Some(3));
    assert_eq!(s.depth_format, Some(40));
    assert_eq!((s.sample_count, s.sample_quality), (Some(4), Some(1)));
    assert_eq!(s.node_mask, Some(1));
    assert_eq!(s.flags, Some(2));
}

#[test]
fn render_target_formats_are_cut_to_the_count() {
    let mut words: Vec<u32> = (0..8).map(|i| 28 + i).collect();
    words.push(3);
    let mut b = Builder::default();
    b.words(15, &words);
    assert_eq!(parse(&b).unwrap().render_targets, Some(vec![28, 29, 30]));
}

#[test]
fn stream_output_reads_entries_strides_and_the_rasterized_stream() {
    let mut entry = 1u32.to_le_bytes().to_vec();
    entry.extend_from_slice(&[0; 4]);
    entry.extend_from_slice(&0x3009u64.to_le_bytes());
    entry.extend_from_slice(&2u32.to_le_bytes());
    entry.extend_from_slice(&[1, 3, 0, 0]);
    let mut memory = memory_with_code();
    memory.insert(0x5000, entry);
    memory.insert(0x6000, [16u32.to_le_bytes(), 32u32.to_le_bytes()].concat());
    let mut data = 0x5000u64.to_le_bytes().to_vec();
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&0x6000u64.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    let mut b = Builder::default();
    b.push(7, 8, &data);
    let s = PipelineStream::parse(&b.bytes, &memory, Dialect::Pc).unwrap();
    let so = s.stream_output.clone().unwrap();
    assert_eq!(so.entries.len(), 1);
    assert_eq!(so.entries[0].stream, 1);
    assert_eq!(so.entries[0].semantic, "TEXCOORD");
    assert_eq!(so.entries[0].semantic_index, 2);
    assert_eq!(
        (so.entries[0].start_component, so.entries[0].component_count),
        (1, 3)
    );
    assert_eq!(so.strides, [16, 32]);
    assert_eq!(so.rasterized_stream, 1);
    assert!(s.unsupported().contains(&"stream output"));
}

#[test]
fn view_instancing_reads_locations() {
    let mut memory = memory_with_code();
    memory.insert(
        0x7000,
        [0u32, 1, 2, 3]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect(),
    );
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&0x7000u64.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&[0; 4]);
    let mut b = Builder::default();
    b.push(22, 8, &data);
    let s = PipelineStream::parse(&b.bytes, &memory, Dialect::Pc).unwrap();
    let v = s.view_instancing.unwrap();
    assert_eq!(v.locations, [(0, 1), (2, 3)]);
    assert_eq!(v.flags, 1);
}

#[test]
fn cached_pso_blob_is_read_and_an_empty_one_is_absent() {
    let mut b = Builder::default();
    b.pointer_pair(19, 0x1000, 4);
    assert_eq!(parse(&b).unwrap().cached_pso.unwrap().0, [1, 2, 3, 4]);
    let mut b = Builder::default();
    b.pointer_pair(19, 0, 0);
    assert!(parse(&b).unwrap().cached_pso.is_none());
}

#[test]
fn mesh_numbering_differs_between_the_console_and_the_pc() {
    let mut b = Builder::default();
    b.pointer_pair(24, 0x1000, 4).pointer_pair(23, 0x2000, 3);
    let console = PipelineStream::parse(&b.bytes, &memory_with_code(), Dialect::Console).unwrap();
    assert_eq!(console.mesh.0.len(), 4);
    assert_eq!(console.amplification.0.len(), 3);
    let mut b = Builder::default();
    b.pointer_pair(25, 0x1000, 4).pointer_pair(24, 0x2000, 3);
    let pc = parse(&b).unwrap();
    assert_eq!(pc.mesh.0.len(), 4);
    assert_eq!(pc.amplification.0.len(), 3);
}

#[test]
fn console_does_not_know_the_pc_only_types() {
    let mut b = Builder::default();
    b.words(26, &[0; 15]);
    assert_eq!(
        PipelineStream::parse(&b.bytes, &memory_with_code(), Dialect::Console).unwrap_err(),
        StreamError::UnknownType(26)
    );
}

#[test]
fn bad_streams_are_refused() {
    let mut b = Builder::default();
    b.words(9, &[1]).words(9, &[2]);
    assert_eq!(parse(&b).unwrap_err(), StreamError::Duplicate(9));
    let mut b = Builder::default();
    b.words(10, &[0; 11]).words(27, &[0; 11]);
    assert_eq!(parse(&b).unwrap_err(), StreamError::Duplicate(27));
    let mut b = Builder::default();
    b.words(99, &[0]);
    assert_eq!(parse(&b).unwrap_err(), StreamError::UnknownType(99));
    let mut b = Builder::default();
    b.words(10, &[0; 5]);
    assert_eq!(parse(&b).unwrap_err(), StreamError::Truncated(10));
}

#[test]
fn stage_combinations_are_checked_like_the_pc_runtime() {
    let stage = |ty: u32| {
        let mut b = Builder::default();
        b.pointer_pair(ty, 0x1000, 4);
        b
    };
    assert!(parse(&stage(1)).unwrap().validate().is_ok());
    assert!(parse(&stage(6)).unwrap().validate().is_ok());
    assert!(parse(&stage(25)).unwrap().validate().is_ok());
    assert_eq!(
        parse(&Builder::default()).unwrap().validate(),
        Err(StreamError::NoShader)
    );
    let mut b = stage(25);
    b.pointer_pair(1, 0x1000, 4);
    assert_eq!(
        parse(&b).unwrap().validate(),
        Err(StreamError::MeshWithClassic)
    );
    let mut b = stage(6);
    b.pointer_pair(2, 0x1000, 4);
    assert_eq!(
        parse(&b).unwrap().validate(),
        Err(StreamError::ComputeWithGraphics)
    );
    assert_eq!(
        parse(&stage(24)).unwrap().validate(),
        Err(StreamError::AmplificationWithoutMesh)
    );
}

fn empty_root() -> Arc<RootSignature> {
    Arc::new(RootSignature {
        version: RootVersion::V1_0,
        parameters: Vec::new(),
        static_samplers: Vec::new(),
        flags: 0,
    })
}

#[test]
fn graphics_description_takes_stream_state_and_keeps_defaults_for_the_rest() {
    let mut b = Builder::default();
    b.pointer_pair(1, 0x1000, 4)
        .pointer_pair(2, 0x2000, 3)
        .words(9, &[0x00ff_ffff])
        .words(14, &[3])
        .words(15, &[28, 29, 0, 0, 0, 0, 0, 0, 2])
        .words(16, &[45])
        .words(17, &[2, 0]);
    let desc = parse(&b).unwrap().graphics(empty_root()).unwrap();
    assert_eq!(desc.vs.0, [1, 2, 3, 4]);
    assert_eq!(desc.ps.0, [9, 8, 7]);
    assert_eq!(desc.sample_mask, 0x00ff_ffff);
    assert_eq!(desc.render_targets, [28, 29]);
    assert_eq!(desc.depth_format, 45);
    assert_eq!(desc.sample_count, 2);
    assert_eq!(desc.raster, RasterizerState::default());
    assert_eq!(desc.depth_stencil, DepthStencilState::default());
    assert_eq!(desc.blend, BlendState::default());
}

#[test]
fn compute_description_has_only_the_shader() {
    let mut b = Builder::default();
    b.pointer_pair(6, 0x1000, 4);
    let s = parse(&b).unwrap();
    assert_eq!(s.compute(None).unwrap().cs.0, [1, 2, 3, 4]);
    assert!(s.graphics(empty_root()).is_err());
}

/// The stream of RVA 0x02479760: 0x2b0 bytes, zeroed, with the offsets the filler at 0x0086f090 writes.
#[test]
fn the_games_own_0x2b0_stream_parses() {
    let mut bytes = vec![0u8; 0x2b0];
    let put32 = |bytes: &mut Vec<u8>, at: usize, value: u32| {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes())
    };
    let put64 = |bytes: &mut Vec<u8>, at: usize, value: u64| {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes())
    };
    for (at, ty) in [
        (0x10, 1),
        (0x28, 2),
        (0x40, 3),
        (0x58, 4),
        (0x70, 5),
        (0x88, 8),
        (0x1d8, 9),
        (0x1e0, 10),
        (0x210, 21),
        (0x250, 17),
        (0x260, 12),
        (0x278, 14),
        (0x280, 15),
        (0x2a8, 16),
    ] {
        put32(&mut bytes, at, ty);
    }
    put64(&mut bytes, 8, 0x1234_5678);
    put64(&mut bytes, 0x18, 0x1000);
    put64(&mut bytes, 0x20, 4);
    put64(&mut bytes, 0x30, 0x2000);
    put64(&mut bytes, 0x38, 3);
    put32(&mut bytes, 0x1dc, u32::MAX);
    put32(&mut bytes, 0x254, 1);
    put32(&mut bytes, 0x27c, 3);
    put32(&mut bytes, 0x284, 28);
    put32(&mut bytes, 0x2a4, 1);
    put32(&mut bytes, 0x2ac, 45);
    let s = PipelineStream::parse(&bytes, &memory_with_code(), Dialect::Console).unwrap();
    assert_eq!(
        s.order,
        [0, 1, 2, 3, 4, 5, 8, 9, 10, 21, 17, 12, 14, 15, 16]
    );
    assert_eq!(s.root_signature, Some(0x1234_5678));
    assert_eq!(s.vs.0, [1, 2, 3, 4]);
    assert_eq!(s.ps.0, [9, 8, 7]);
    assert!(s.ds.is_empty() && s.hs.is_empty() && s.gs.is_empty());
    assert_eq!(s.sample_mask, Some(u32::MAX));
    assert_eq!(s.topology, Some(3));
    assert_eq!(s.render_targets, Some(vec![28]));
    assert_eq!(s.depth_format, Some(45));
    assert_eq!(s.input_layout, Some(Vec::new()));
    assert!(s.validate().is_ok());
}
