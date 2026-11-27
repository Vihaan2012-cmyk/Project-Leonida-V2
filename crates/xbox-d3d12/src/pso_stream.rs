//! Pipeline state streams (`D3D12_PIPELINE_STATE_STREAM_DESC`) read into the descriptions the pipeline builder takes.
//!
//! The game's own stream (device slot 86, the callers at RVA 0x02479760 and its filler 0x0086f090) is a 0x2b0-byte
//! block, zeroed and then written in d3dx12 layout: a 4-byte subobject type at an 8-aligned offset, the description
//! after it at the description's own alignment (4 for plain integers, 8 for anything with a pointer). It always
//! writes the tags of VS, PS, DS, HS and GS, even for a stage it has no shader for (the bytecode is then
//! null/0), and writes type 0 (root signature) at offset 0 with the signature pointer at +8. When the stage
//! object is a mesh pipeline it rewrites the VS tag to 24 and the HS tag to 23: on the console AS is 23 and MS is
//! 24, where the PC header has AS 24 and MS 25. [`Dialect`] picks the numbering.
//!
//! Pointers inside a stream (bytecode, input elements, semantic names, stream-output entries) are read through a
//! [`Memory`], so the same parser runs over the game's address space and over a synthetic one in tests.

use std::collections::HashMap;

use crate::pipeline::{
    BlendState, Bytecode, ComputePsoDesc, DepthStencilState, GraphicsPsoDesc, InputElement,
    PipelineError, RasterizerState, RenderTargetBlend, StencilFace,
};
use crate::root::RootSignature;
use std::sync::Arc;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StreamError {
    #[error("the stream ends inside a subobject of type {0}")]
    Truncated(u32),
    #[error("subobject type {0} is not known")]
    UnknownType(u32),
    #[error("subobject type {0} appears twice")]
    Duplicate(u32),
    #[error("a pointer in subobject type {0} cannot be read")]
    BadPointer(u32),
    #[error("a mesh shader cannot be combined with vertex, hull, domain or geometry shaders")]
    MeshWithClassic,
    #[error("a compute shader cannot be combined with graphics state or shaders")]
    ComputeWithGraphics,
    #[error("the stream names no shader")]
    NoShader,
    #[error("an amplification shader needs a mesh shader")]
    AmplificationWithoutMesh,
}

/// Subobject numbering. The console and the PC header agree up to 22 and disagree after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// AS 24, MS 25, DEPTH_STENCIL2 26, RASTERIZER1 27, RASTERIZER2 28.
    Pc,
    /// AS 23, MS 24 (measured in the game's stream filler); nothing above 24 is measured.
    Console,
}

/// Where the pointers of a stream lead.
pub trait Memory {
    fn read(&self, address: u64, len: usize) -> Option<Vec<u8>>;
}

/// The address space of this process, for streams the game built.
pub struct HostMemory;

impl Memory for HostMemory {
    fn read(&self, address: u64, len: usize) -> Option<Vec<u8>> {
        if address == 0 {
            return None;
        }
        // SAFETY: the address comes from a stream the title built in its own memory, which it keeps alive
        // for the duration of the creation call; a bad pointer there faults as it would on the console.
        Some(unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec())
    }
}

/// A map of regions, for tests and for replaying a recorded stream.
#[derive(Default)]
pub struct RegionMemory {
    regions: HashMap<u64, Vec<u8>>,
}

impl RegionMemory {
    pub fn insert(&mut self, address: u64, bytes: Vec<u8>) {
        self.regions.insert(address, bytes);
    }
}

impl Memory for RegionMemory {
    fn read(&self, address: u64, len: usize) -> Option<Vec<u8>> {
        self.regions.iter().find_map(|(base, bytes)| {
            let start = address.checked_sub(*base)? as usize;
            bytes
                .get(start..start.checked_add(len)?)
                .map(<[u8]>::to_vec)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamOutputEntry {
    pub stream: u32,
    pub semantic: String,
    pub semantic_index: u32,
    pub start_component: u8,
    pub component_count: u8,
    pub output_slot: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamOutput {
    pub entries: Vec<StreamOutputEntry>,
    pub strides: Vec<u32>,
    pub rasterized_stream: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInstancing {
    /// `(ViewportArrayIndex, RenderTargetArrayIndex)` per view.
    pub locations: Vec<(u32, u32)>,
    pub flags: u32,
}

/// Depth-stencil state carries more than `DepthStencilState` keeps: separate masks per face and the bounds test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DepthStencilExtra {
    pub depth_bounds_test: bool,
    /// Only `DEPTH_STENCIL2` has them: `(read, write)` of the front and of the back face.
    pub face_masks: Option<[(u8, u8); 2]>,
}

/// Rasterizer fields of `RASTERIZER1` and `RASTERIZER2` that `RasterizerState` has no place for.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RasterizerExtra {
    /// The float bias; `RasterizerState::depth_bias` holds it truncated.
    pub depth_bias_float: Option<f32>,
    /// `D3D12_LINE_RASTERIZATION_MODE`, from `RASTERIZER2`.
    pub line_mode: Option<u32>,
    pub antialiased_line: bool,
}

/// Everything a stream said. A field is `None` or empty when the stream left it out, so the caller applies the
/// defaults of the description it builds.
#[derive(Debug, Clone, Default)]
pub struct PipelineStream {
    /// The `ID3D12RootSignature` pointer, as a raw address.
    pub root_signature: Option<u64>,
    pub vs: Bytecode,
    pub ps: Bytecode,
    pub ds: Bytecode,
    pub hs: Bytecode,
    pub gs: Bytecode,
    pub cs: Bytecode,
    pub amplification: Bytecode,
    pub mesh: Bytecode,
    pub stream_output: Option<StreamOutput>,
    pub blend: Option<BlendState>,
    pub sample_mask: Option<u32>,
    pub rasterizer: Option<RasterizerState>,
    pub rasterizer_extra: RasterizerExtra,
    pub depth_stencil: Option<DepthStencilState>,
    pub depth_stencil_extra: DepthStencilExtra,
    pub input_layout: Option<Vec<InputElement>>,
    pub strip_cut: Option<u32>,
    pub topology: Option<u32>,
    pub render_targets: Option<Vec<u32>>,
    pub depth_format: Option<u32>,
    pub sample_count: Option<u32>,
    pub sample_quality: Option<u32>,
    pub node_mask: Option<u32>,
    pub cached_pso: Option<Bytecode>,
    pub flags: Option<u32>,
    pub view_instancing: Option<ViewInstancing>,
    /// Types in the order they appeared.
    pub order: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    RootSignature,
    Vs,
    Ps,
    Ds,
    Hs,
    Gs,
    Cs,
    StreamOutput,
    Blend,
    SampleMask,
    Rasterizer,
    DepthStencil,
    InputLayout,
    StripCut,
    Topology,
    RenderTargets,
    DepthFormat,
    SampleDesc,
    NodeMask,
    CachedPso,
    Flags,
    DepthStencil1,
    ViewInstancing,
    Amplification,
    Mesh,
    DepthStencil2,
    Rasterizer1,
    Rasterizer2,
}

impl Kind {
    fn from_type(value: u32, dialect: Dialect) -> Option<Kind> {
        use Kind::*;
        const COMMON: [Kind; 23] = [
            RootSignature,
            Vs,
            Ps,
            Ds,
            Hs,
            Gs,
            Cs,
            StreamOutput,
            Blend,
            SampleMask,
            Rasterizer,
            DepthStencil,
            InputLayout,
            StripCut,
            Topology,
            RenderTargets,
            DepthFormat,
            SampleDesc,
            NodeMask,
            CachedPso,
            Flags,
            DepthStencil1,
            ViewInstancing,
        ];
        if let Some(kind) = COMMON.get(value as usize) {
            return Some(*kind);
        }
        match (dialect, value) {
            (Dialect::Console, 23) | (Dialect::Pc, 24) => Some(Amplification),
            (Dialect::Console, 24) | (Dialect::Pc, 25) => Some(Mesh),
            (Dialect::Pc, 26) => Some(DepthStencil2),
            (Dialect::Pc, 27) => Some(Rasterizer1),
            (Dialect::Pc, 28) => Some(Rasterizer2),
            _ => None,
        }
    }

    /// The PC runtime counts the rasterizer and depth-stencil variants as one subobject each.
    fn family(self) -> Kind {
        match self {
            Kind::Rasterizer1 | Kind::Rasterizer2 => Kind::Rasterizer,
            Kind::DepthStencil1 | Kind::DepthStencil2 => Kind::DepthStencil,
            other => other,
        }
    }

    /// `(size, alignment)` of the description.
    fn layout(self) -> (usize, usize) {
        use Kind::*;
        match self {
            RootSignature => (8, 8),
            Vs | Ps | Ds | Hs | Gs | Cs | Amplification | Mesh | CachedPso => (16, 8),
            StreamOutput => (32, 8),
            Blend => (0x148, 4),
            SampleMask | StripCut | Topology | DepthFormat | NodeMask | Flags => (4, 4),
            Rasterizer | Rasterizer1 => (44, 4),
            Rasterizer2 => (40, 4),
            DepthStencil => (52, 4),
            InputLayout => (12, 8),
            RenderTargets => (36, 4),
            SampleDesc => (8, 4),
            DepthStencil1 => (56, 4),
            ViewInstancing => (20, 8),
            DepthStencil2 => (60, 4),
        }
    }
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> u32 {
        let value = u32::from_le_bytes(self.bytes[self.at..self.at + 4].try_into().expect("4"));
        self.at += 4;
        value
    }

    fn u64(&mut self) -> u64 {
        let value = u64::from_le_bytes(self.bytes[self.at..self.at + 8].try_into().expect("8"));
        self.at += 8;
        value
    }

    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }

    fn u8(&mut self) -> u8 {
        let value = self.bytes[self.at];
        self.at += 1;
        value
    }

    fn skip(&mut self, count: usize) {
        self.at += count;
    }
}

fn bytecode(
    cursor: &mut Cursor<'_>,
    memory: &dyn Memory,
    ty: u32,
) -> Result<Bytecode, StreamError> {
    let pointer = cursor.u64();
    let length = cursor.u64() as usize;
    if pointer == 0 || length == 0 {
        return Ok(Bytecode::default());
    }
    memory
        .read(pointer, length)
        .map(Bytecode)
        .ok_or(StreamError::BadPointer(ty))
}

fn string(memory: &dyn Memory, address: u64, ty: u32) -> Result<String, StreamError> {
    let mut out = Vec::new();
    for offset in 0..128u64 {
        let byte = memory
            .read(address + offset, 1)
            .ok_or(StreamError::BadPointer(ty))?[0];
        if byte == 0 {
            return Ok(String::from_utf8_lossy(&out).into_owned());
        }
        out.push(byte);
    }
    Err(StreamError::BadPointer(ty))
}

fn stencil_face(cursor: &mut Cursor<'_>) -> StencilFace {
    StencilFace {
        fail: cursor.u32(),
        depth_fail: cursor.u32(),
        pass: cursor.u32(),
        func: cursor.u32(),
    }
}

fn blend(cursor: &mut Cursor<'_>) -> BlendState {
    let mut state = BlendState {
        alpha_to_coverage: cursor.u32() != 0,
        independent: cursor.u32() != 0,
        ..BlendState::default()
    };
    for target in &mut state.targets {
        *target = RenderTargetBlend {
            blend_enable: cursor.u32() != 0,
            logic_enable: cursor.u32() != 0,
            src: cursor.u32(),
            dst: cursor.u32(),
            op: cursor.u32(),
            src_alpha: cursor.u32(),
            dst_alpha: cursor.u32(),
            op_alpha: cursor.u32(),
            logic_op: cursor.u32(),
            write_mask: cursor.u8(),
        };
        cursor.skip(3);
    }
    state
}

impl PipelineStream {
    /// Reads `bytes` as a subobject stream. The first subobject is at offset 0.
    pub fn parse(
        bytes: &[u8],
        memory: &dyn Memory,
        dialect: Dialect,
    ) -> Result<PipelineStream, StreamError> {
        let mut out = PipelineStream::default();
        let mut at = 0usize;
        let mut seen = Vec::new();
        while at < bytes.len() {
            if at + 4 > bytes.len() {
                return Err(StreamError::Truncated(0));
            }
            let ty = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4"));
            let kind = Kind::from_type(ty, dialect).ok_or(StreamError::UnknownType(ty))?;
            let (size, alignment) = kind.layout();
            let data = align(at + 4, alignment);
            if data + size > bytes.len() {
                return Err(StreamError::Truncated(ty));
            }
            let family = kind.family();
            if seen.contains(&family) {
                return Err(StreamError::Duplicate(ty));
            }
            seen.push(family);
            out.order.push(ty);
            let mut cursor = Cursor { bytes, at: data };
            out.read_one(kind, ty, &mut cursor, memory)?;
            at = align(data + size, 8);
        }
        Ok(out)
    }

    fn read_one(
        &mut self,
        kind: Kind,
        ty: u32,
        c: &mut Cursor<'_>,
        memory: &dyn Memory,
    ) -> Result<(), StreamError> {
        match kind {
            Kind::RootSignature => {
                let pointer = c.u64();
                self.root_signature = (pointer != 0).then_some(pointer);
            }
            Kind::Vs => self.vs = bytecode(c, memory, ty)?,
            Kind::Ps => self.ps = bytecode(c, memory, ty)?,
            Kind::Ds => self.ds = bytecode(c, memory, ty)?,
            Kind::Hs => self.hs = bytecode(c, memory, ty)?,
            Kind::Gs => self.gs = bytecode(c, memory, ty)?,
            Kind::Cs => self.cs = bytecode(c, memory, ty)?,
            Kind::Amplification => self.amplification = bytecode(c, memory, ty)?,
            Kind::Mesh => self.mesh = bytecode(c, memory, ty)?,
            Kind::CachedPso => {
                let cached = bytecode(c, memory, ty)?;
                self.cached_pso = (!cached.is_empty()).then_some(cached);
            }
            Kind::StreamOutput => {
                let entries = c.u64();
                let count = c.u32() as usize;
                c.skip(4);
                let strides = c.u64();
                let stride_count = c.u32() as usize;
                let rasterized_stream = c.u32();
                let mut output = StreamOutput {
                    rasterized_stream,
                    ..StreamOutput::default()
                };
                if count > 0 {
                    let raw = memory
                        .read(entries, count * 24)
                        .ok_or(StreamError::BadPointer(ty))?;
                    let mut e = Cursor { bytes: &raw, at: 0 };
                    for _ in 0..count {
                        let stream = e.u32();
                        e.skip(4);
                        let name = e.u64();
                        let semantic_index = e.u32();
                        let start_component = e.u8();
                        let component_count = e.u8();
                        let output_slot = e.u8();
                        e.skip(1);
                        output.entries.push(StreamOutputEntry {
                            stream,
                            semantic: if name == 0 {
                                String::new()
                            } else {
                                string(memory, name, ty)?
                            },
                            semantic_index,
                            start_component,
                            component_count,
                            output_slot,
                        });
                    }
                }
                if stride_count > 0 {
                    let raw = memory
                        .read(strides, stride_count * 4)
                        .ok_or(StreamError::BadPointer(ty))?;
                    output.strides = raw
                        .chunks_exact(4)
                        .map(|w| u32::from_le_bytes(w.try_into().expect("4")))
                        .collect();
                }
                self.stream_output = Some(output);
            }
            Kind::Blend => self.blend = Some(blend(c)),
            Kind::SampleMask => self.sample_mask = Some(c.u32()),
            Kind::Rasterizer => {
                let fill = c.u32();
                let cull = c.u32();
                let front_ccw = c.u32() != 0;
                let depth_bias = c.u32() as i32;
                let depth_bias_clamp = c.f32();
                let slope_scaled_bias = c.f32();
                let depth_clip = c.u32() != 0;
                let multisample = c.u32() != 0;
                self.rasterizer_extra.antialiased_line = c.u32() != 0;
                let forced_sample_count = c.u32();
                let conservative = c.u32() != 0;
                self.rasterizer = Some(RasterizerState {
                    fill,
                    cull,
                    front_ccw,
                    depth_bias,
                    depth_bias_clamp,
                    slope_scaled_bias,
                    depth_clip,
                    multisample,
                    forced_sample_count,
                    conservative,
                });
            }
            Kind::Rasterizer1 | Kind::Rasterizer2 => {
                let fill = c.u32();
                let cull = c.u32();
                let front_ccw = c.u32() != 0;
                let bias = c.f32();
                let depth_bias_clamp = c.f32();
                let slope_scaled_bias = c.f32();
                let depth_clip = c.u32() != 0;
                let (multisample, line_mode) = if kind == Kind::Rasterizer1 {
                    let multisample = c.u32() != 0;
                    self.rasterizer_extra.antialiased_line = c.u32() != 0;
                    (multisample, None)
                } else {
                    let mode = c.u32();
                    (false, Some(mode))
                };
                let forced_sample_count = c.u32();
                let conservative = c.u32() != 0;
                self.rasterizer_extra.depth_bias_float = Some(bias);
                self.rasterizer_extra.line_mode = line_mode;
                self.rasterizer = Some(RasterizerState {
                    fill,
                    cull,
                    front_ccw,
                    depth_bias: bias as i32,
                    depth_bias_clamp,
                    slope_scaled_bias,
                    depth_clip,
                    multisample,
                    forced_sample_count,
                    conservative,
                });
            }
            Kind::DepthStencil | Kind::DepthStencil1 => {
                let depth_enable = c.u32() != 0;
                let depth_write = c.u32() != 0;
                let depth_func = c.u32();
                let stencil_enable = c.u32() != 0;
                let stencil_read_mask = c.u8();
                let stencil_write_mask = c.u8();
                c.skip(2);
                let front = stencil_face(c);
                let back = stencil_face(c);
                if kind == Kind::DepthStencil1 {
                    self.depth_stencil_extra.depth_bounds_test = c.u32() != 0;
                }
                self.depth_stencil = Some(DepthStencilState {
                    depth_enable,
                    depth_write,
                    depth_func,
                    stencil_enable,
                    stencil_read_mask,
                    stencil_write_mask,
                    front,
                    back,
                });
            }
            Kind::DepthStencil2 => {
                let depth_enable = c.u32() != 0;
                let depth_write = c.u32() != 0;
                let depth_func = c.u32();
                let stencil_enable = c.u32() != 0;
                let face = |c: &mut Cursor<'_>| {
                    let ops = stencil_face(c);
                    let read = c.u8();
                    let write = c.u8();
                    c.skip(2);
                    (ops, read, write)
                };
                let (front, front_read, front_write) = face(c);
                let (back, back_read, back_write) = face(c);
                self.depth_stencil_extra.depth_bounds_test = c.u32() != 0;
                self.depth_stencil_extra.face_masks =
                    Some([(front_read, front_write), (back_read, back_write)]);
                self.depth_stencil = Some(DepthStencilState {
                    depth_enable,
                    depth_write,
                    depth_func,
                    stencil_enable,
                    stencil_read_mask: front_read,
                    stencil_write_mask: front_write,
                    front,
                    back,
                });
            }
            Kind::InputLayout => {
                let elements = c.u64();
                let count = c.u32() as usize;
                let mut layout = Vec::with_capacity(count);
                if count > 0 {
                    let raw = memory
                        .read(elements, count * 32)
                        .ok_or(StreamError::BadPointer(ty))?;
                    let mut e = Cursor { bytes: &raw, at: 0 };
                    for _ in 0..count {
                        let name = e.u64();
                        layout.push(InputElement {
                            semantic: string(memory, name, ty)?,
                            semantic_index: e.u32(),
                            format: e.u32(),
                            slot: e.u32(),
                            offset: e.u32(),
                            class: e.u32(),
                            step_rate: e.u32(),
                        });
                    }
                }
                self.input_layout = Some(layout);
            }
            Kind::StripCut => self.strip_cut = Some(c.u32()),
            Kind::Topology => self.topology = Some(c.u32()),
            Kind::RenderTargets => {
                let formats: Vec<u32> = (0..8).map(|_| c.u32()).collect();
                let count = (c.u32() as usize).min(8);
                self.render_targets = Some(formats[..count].to_vec());
            }
            Kind::DepthFormat => self.depth_format = Some(c.u32()),
            Kind::SampleDesc => {
                self.sample_count = Some(c.u32());
                self.sample_quality = Some(c.u32());
            }
            Kind::NodeMask => self.node_mask = Some(c.u32()),
            Kind::Flags => self.flags = Some(c.u32()),
            Kind::ViewInstancing => {
                let count = c.u32() as usize;
                c.skip(4);
                let locations = c.u64();
                let flags = c.u32();
                let mut list = Vec::with_capacity(count);
                if count > 0 {
                    let raw = memory
                        .read(locations, count * 8)
                        .ok_or(StreamError::BadPointer(ty))?;
                    let mut e = Cursor { bytes: &raw, at: 0 };
                    for _ in 0..count {
                        list.push((e.u32(), e.u32()));
                    }
                }
                self.view_instancing = Some(ViewInstancing {
                    locations: list,
                    flags,
                });
            }
        }
        Ok(())
    }

    pub fn is_compute(&self) -> bool {
        !self.cs.is_empty()
    }

    pub fn is_mesh(&self) -> bool {
        !self.mesh.is_empty()
    }

    /// The checks the PC runtime makes: a stage must exist, a mesh pipeline has no classic geometry stages and
    /// a compute pipeline has nothing else.
    pub fn validate(&self) -> Result<(), StreamError> {
        let classic = [&self.vs, &self.hs, &self.ds, &self.gs];
        let any_classic = classic.iter().any(|b| !b.is_empty());
        if self.is_compute() {
            if any_classic
                || !self.ps.is_empty()
                || self.is_mesh()
                || !self.amplification.is_empty()
            {
                return Err(StreamError::ComputeWithGraphics);
            }
            return Ok(());
        }
        if self.is_mesh() {
            return if any_classic {
                Err(StreamError::MeshWithClassic)
            } else {
                Ok(())
            };
        }
        if !self.amplification.is_empty() {
            return Err(StreamError::AmplificationWithoutMesh);
        }
        if !any_classic && self.ps.is_empty() {
            return Err(StreamError::NoShader);
        }
        Ok(())
    }

    /// The graphics description this stream describes, over `root`. A stream with a compute shader is not one.
    pub fn graphics(&self, root: Arc<RootSignature>) -> Result<GraphicsPsoDesc, StreamError> {
        self.validate()?;
        if self.is_compute() {
            return Err(StreamError::ComputeWithGraphics);
        }
        let mut desc = GraphicsPsoDesc::new(root);
        desc.vs = self.vs.clone();
        desc.ps = self.ps.clone();
        desc.gs = self.gs.clone();
        desc.ms = self.mesh.clone();
        if let Some(blend) = self.blend {
            desc.blend = blend;
        }
        if let Some(mask) = self.sample_mask {
            desc.sample_mask = mask;
        }
        if let Some(raster) = self.rasterizer {
            desc.raster = raster;
        }
        if let Some(depth_stencil) = self.depth_stencil {
            desc.depth_stencil = depth_stencil;
        }
        if let Some(layout) = &self.input_layout {
            desc.input_layout = layout.clone();
        }
        if let Some(cut) = self.strip_cut {
            desc.strip_cut = cut;
        }
        if let Some(topology) = self.topology {
            desc.topology = topology;
        }
        if let Some(targets) = &self.render_targets {
            desc.render_targets = targets.clone();
        }
        if let Some(format) = self.depth_format {
            desc.depth_format = format;
        }
        if let Some(count) = self.sample_count {
            desc.sample_count = count;
        }
        Ok(desc)
    }

    /// The compute description of a stream with only a compute shader (and perhaps a root signature).
    pub fn compute(&self, root: Option<Arc<RootSignature>>) -> Result<ComputePsoDesc, StreamError> {
        self.validate()?;
        if !self.is_compute() {
            return Err(StreamError::NoShader);
        }
        Ok(ComputePsoDesc {
            root,
            cs: self.cs.clone(),
        })
    }

    /// What the pipeline builder cannot honour yet, for a trace note: each is a piece of state the stream
    /// carried that no description field holds.
    pub fn unsupported(&self) -> Vec<&'static str> {
        let mut list = Vec::new();
        if !self.amplification.is_empty() {
            list.push("amplification shader");
        }
        if self
            .stream_output
            .as_ref()
            .is_some_and(|s| !s.entries.is_empty())
        {
            list.push("stream output");
        }
        if self.depth_stencil_extra.depth_bounds_test {
            list.push("depth bounds test");
        }
        if self
            .depth_stencil_extra
            .face_masks
            .is_some_and(|m| m[0] != m[1])
        {
            list.push("separate front and back stencil masks");
        }
        if self.view_instancing.is_some() {
            list.push("view instancing");
        }
        if self
            .rasterizer_extra
            .depth_bias_float
            .is_some_and(|b| b != b.trunc())
        {
            list.push("fractional depth bias");
        }
        if self.node_mask.is_some_and(|m| m > 1) {
            list.push("node mask above 1");
        }
        list
    }
}

impl From<StreamError> for PipelineError {
    fn from(error: StreamError) -> PipelineError {
        match error {
            StreamError::UnknownType(_) | StreamError::Truncated(_) => {
                PipelineError::Invalid("pipeline state stream")
            }
            _ => PipelineError::Invalid("pipeline state stream combination"),
        }
    }
}

/// One stage of the console description at device slot 57: a program and the record that describes it, each as
/// a pointer and a size already resolved from the cache's tables (an absent stage is null and 0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStage {
    pub code: Vec<u8>,
    pub meta: Vec<u8>,
}

/// The 0x108-byte description the game hands device slot 57 (RVA 0x024774c0 calls it; 0x0248f680 builds the
/// pieces from the 0x70-byte cache records): the root signature, two pointer/size pairs into the cache's meta
/// blob (the stage record, then the input-layout record), then seven stages of four words each, `[code pointer,
/// code size, meta pointer, meta size]`, in the order of the cache record's fields at +0x38 to +0x68.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeGraphicsDesc {
    pub root_signature: u64,
    pub stage_record: Vec<u8>,
    pub input_layout_record: Vec<u8>,
    pub stages: [NativeStage; 7],
}

impl NativeGraphicsDesc {
    pub const SIZE: usize = 0x108;

    pub fn parse(bytes: &[u8], memory: &dyn Memory) -> Result<NativeGraphicsDesc, StreamError> {
        if bytes.len() < Self::SIZE {
            return Err(StreamError::Truncated(57));
        }
        let mut c = Cursor { bytes, at: 0 };
        let pair = |c: &mut Cursor<'_>| -> Result<Vec<u8>, StreamError> {
            let pointer = c.u64();
            let size = c.u64() as usize;
            if pointer == 0 || size == 0 {
                return Ok(Vec::new());
            }
            memory
                .read(pointer, size)
                .ok_or(StreamError::BadPointer(57))
        };
        let root_signature = c.u64();
        let stage_record = pair(&mut c)?;
        let input_layout_record = pair(&mut c)?;
        let mut stages: [NativeStage; 7] = Default::default();
        for stage in &mut stages {
            stage.code = pair(&mut c)?;
            stage.meta = pair(&mut c)?;
        }
        Ok(NativeGraphicsDesc {
            root_signature,
            stage_record,
            input_layout_record,
            stages,
        })
    }
}

#[cfg(test)]
mod tests;
