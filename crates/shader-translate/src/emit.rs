use std::collections::{BTreeMap, BTreeSet, HashMap};

use rspirv::binary::Assemble;
use rspirv::dr::{Builder, Operand};
use rspirv::spirv::{
    AddressingModel, BuiltIn, Capability, Decoration, ExecutionMode, ExecutionModel,
    FunctionControl, GlslStd450Op as GLOp, LoopControl, MemoryModel, SelectionControl,
    StorageClass, Word,
};
use shader_ir::{
    Bin, Block, Cmp, Expr, Function, Image, Reg, Round, Stmt, Terminator, Tri, Ty, Un, VerifyError,
};

use crate::images::{Number, View};

mod flow;
mod geometry;
mod image;
mod loads;
mod memory;
mod merged;
mod pixel;
#[cfg(test)]
mod push_layout;
mod typed;

pub const STATE_SCC: u32 = 128;
pub const STATE_VGPR: u32 = 256;
pub const STATE_ATTRIBUTES: u32 = 256 + 256 * 64;
pub const STATE_WORDS: usize = STATE_ATTRIBUTES as usize + shader_ir::ATTRIBUTES * 12 * 64;
pub const MESSAGE_BASE: u32 = 64 * 4 * 64;
pub const MESSAGE_LIMIT: u32 = 64;
pub const EXPORT_BUFFER_WORDS: usize = MESSAGE_BASE as usize + 1 + 2 * MESSAGE_LIMIT as usize;
const EXEC: u8 = 126;
const VCC: u8 = 106;
const M0: u8 = 124;
const END: u32 = u32::MAX;
const ITERATION_LIMIT: u32 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    #[error("the IR does not verify: {0}")]
    Verify(#[from] VerifyError),
    #[error("building SPIR-V failed: {0}")]
    Build(String),
    #[error("LDS size must be a power of two number of words")]
    LdsSize,
    #[error("{0}")]
    Unsupported(&'static str),
}

fn ok<T>(result: Result<T, rspirv::dr::Error>) -> Result<T, EmitError> {
    result.map_err(|e| EmitError::Build(format!("{e:?}")))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FloatMode {
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
}

/// The push-constant block, in words: s0-s31 (the user data every stage of a pipeline shares), the heap base,
/// then one code base per stage kind, each a low word and a high word. A stage declares the block only as far
/// as its own code base, so a stage never reads another kind's words.
pub const PUSH_HEAP: u32 = 32;
/// The code base of compute, vertex, mesh and the first half of a merged vertex-and-geometry shader.
pub const PUSH_CODE: u32 = 34;
/// The code base of a geometry shader, the second half of a merged shader.
pub const PUSH_CODE_GEOMETRY: u32 = 36;
/// The code base of a pixel shader.
pub const PUSH_CODE_PIXEL: u32 = 38;
/// The size of the whole block; a pipeline's push range covers this many words.
pub const PUSH_WORDS: u32 = 40;

#[derive(Debug, Clone, Copy)]
pub struct ComputeOptions {
    pub group_size: [u32; 3],
    pub user_sgprs: u32,
    pub workgroup_ids: [bool; 3],
    pub thread_ids: u32,
    pub lds_bytes: u32,
    pub float: FloatMode,
}

impl ComputeOptions {
    /// From the FSSM's COMPUTE_PGM_RSRC2 word and thread-group size.
    pub fn from_rsrc2(rsrc2: u32, group_size: [u32; 3], float: FloatMode) -> ComputeOptions {
        ComputeOptions {
            group_size,
            user_sgprs: (rsrc2 >> 1) & 31,
            workgroup_ids: [
                rsrc2 >> 7 & 1 != 0,
                rsrc2 >> 8 & 1 != 0,
                rsrc2 >> 9 & 1 != 0,
            ],
            thread_ids: ((rsrc2 >> 11) & 3) + 1,
            lds_bytes: ((rsrc2 >> 15) & 0x1FF) * 512,
            float,
        }
    }
}

enum Entry<'a> {
    Harness(&'a HarnessOptions),
    Compute(&'a ComputeOptions),
    Pixel(&'a PixelOptions),
    Vertex(&'a VertexOptions),
    Mesh(&'a MeshOptions),
    Es(&'a EsOptions),
    Gs(&'a GsOptions),
}

impl Entry<'_> {
    fn code_word(&self) -> u32 {
        match self {
            Entry::Gs(_) => PUSH_CODE_GEOMETRY,
            Entry::Pixel(_) => PUSH_CODE_PIXEL,
            _ => PUSH_CODE,
        }
    }
}

/// One fetched vertex attribute: the input-signature register, read as four components of `number`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexInput {
    pub register: u8,
    pub number: Number,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexOptions {
    pub user_sgprs: u32,
    pub inputs: Vec<VertexInput>,
    pub float: FloatMode,
}

/// The first half of a merged vertex-and-geometry shader, run as a vertex shader. Its LDS stores become its
/// vertex's outputs: `item_bytes` (16 per output-signature register) at output locations 0 up, as raw words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsOptions {
    /// User SGPRs start at s8, as for the second half.
    pub user_sgprs: u32,
    pub inputs: Vec<VertexInput>,
    pub item_bytes: u32,
    pub float: FloatMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsInput {
    Points,
    Lines,
    Triangles,
    LinesAdjacency,
    TrianglesAdjacency,
}

impl GsInput {
    pub fn vertices(self) -> u32 {
        match self {
            GsInput::Points => 1,
            GsInput::Lines => 2,
            GsInput::Triangles => 3,
            GsInput::LinesAdjacency => 4,
            GsInput::TrianglesAdjacency => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsOutput {
    Points,
    LineStrip,
    TriangleStrip,
}

/// The second half, run as a geometry shader. Each emitted vertex is `vertex_bytes` of LDS, 16 per output
/// register, and an invocation's vertices follow one another; register `position` becomes the position and
/// every other register the raw words at the output location of its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GsOptions {
    pub user_sgprs: u32,
    pub input: GsInput,
    pub item_bytes: u32,
    pub output: GsOutput,
    pub max_vertices: u32,
    pub vertex_bytes: u32,
    pub position: Option<u8>,
    pub float: FloatMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshOptions {
    /// User SGPRs start at s8; s0-s7 hold the hardware's values.
    pub user_sgprs: u32,
    pub group_size: u32,
    pub max_vertices: u32,
    pub max_primitives: u32,
    pub lds_bytes: u32,
    pub float: FloatMode,
}

/// Which way a pixel shader's depth output may move, when it writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthOutput {
    Any,
    GreaterEqual,
    LessEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelOutput {
    pub target: u8,
    pub number: Number,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelOptions {
    pub user_sgprs: u32,
    /// SPI_PS_INPUT_ENA.
    pub inputs: u32,
    pub outputs: Vec<PixelOutput>,
    pub depth: Option<DepthOutput>,
    /// The output location of the stage before that attribute N reads, or `None` for an attribute nothing
    /// writes, which reads as zero; attributes past the list read the location of their own number.
    pub attribute_locations: Vec<Option<u8>>,
    pub float: FloatMode,
}

#[derive(Debug, Clone, Copy)]
pub struct HarnessOptions {
    pub lds_words: u32,
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
    pub code_base: u64,
}

struct Types {
    bool_: Word,
    u32_: Word,
    u64_: Word,
    f32_: Word,
    uvec3: Word,
    uvec4: Word,
    wide: Word,
    ptr_function_u32: Word,
    ptr_function_bool: Word,
    ptr_storage_u32: Word,
    ptr_workgroup_u32: Word,
    ptr_heap_u32: Word,
    ptr_push_u32: Word,
    ptr_private_u32: Word,
}

struct Emitter {
    b: Builder,
    glsl: Word,
    t: Types,
    constants: HashMap<(u8, u64), Word>,
    sgpr: BTreeMap<u8, Word>,
    vgpr: BTreeMap<u8, Word>,
    scc: Word,
    lane: Word,
    local: Word,
    lanes: u32,
    /// A 64-lane wave runs as 32 invocations that each carry two lanes: lane L in part 0, lane L + 32 in part 1.
    pair: bool,
    /// Lane pairs where part 1 holds lanes with no pixel or vertex behind them, inactive until the program
    /// turns them on, so programs that read lanes 32-63 see an empty upper half as on the console.
    phantom: bool,
    part: usize,
    lane_ids: [Word; 2],
    thread_indexes: [Word; 2],
    varying: Vec<bool>,
    values_hi: Vec<Word>,
    vgpr_hi: BTreeMap<u8, Word>,
    state: Word,
    memory: Word,
    lds_image: Word,
    exports: Word,
    messages: Word,
    lds: Word,
    lds_words: u32,
    memory_last: Word,
    constant_values: HashMap<Word, u32>,
    layout_table: Option<Word>,
    typed_cache: [Option<typed::Element>; 2],
    float64: bool,
    /// Per block and statement: `SetLane`s whose skipped lanes are never read again, so they write every lane.
    free_writes: Vec<Vec<bool>>,
    free_write: bool,
    /// Open selections and loops: each merge block and the frame of the block being written inside it.
    frames: Vec<(Word, u32)>,
    next_frame: u32,
    scalar_loads: HashMap<Word, (u32, Word)>,
    descriptor_lookups: HashMap<(usize, Vec<Word>), (u32, Word)>,
    code_base: u64,
    code_word: u32,
    heap: Option<Word>,
    code: Option<Word>,
    push: Word,
    label: Word,
    compute: Option<ComputeOptions>,
    group_ids: Word,
    thread_index: Word,
    values: Vec<Word>,
    types: Vec<Ty>,
    storage_block: Option<Word>,
    image_types: Option<image::ImageTypes>,
    views: HashMap<(View, Number, bool), image::ViewIds>,
    atomic_views: HashMap<(View, bool), image::ViewIds>,
    compare_samplers: Option<Word>,
    lookups: [Option<Word>; 2],
    encode: [Option<Word>; 2],
    image_cache: [Option<(Image, [Word; 4])>; 2],
    pixel: Option<pixel::PixelState>,
    geometry: Option<geometry::GeometryState>,
    stage_lds: Option<merged::StageLds>,
    merged: Option<merged::MergedState>,
    active_cache: [Option<(Word, Word)>; 2],
}

fn registers(function: &Function) -> (BTreeSet<u8>, BTreeSet<u8>) {
    let mut scalars = BTreeSet::from([EXEC, EXEC + 1]);
    let mut vectors = BTreeSet::new();
    let reg = |reg: &Reg, scalars: &mut BTreeSet<u8>, vectors: &mut BTreeSet<u8>| match reg {
        Reg::S(r) => {
            scalars.insert(*r);
        }
        Reg::V(r) => {
            vectors.insert(*r);
        }
        Reg::Exec => {}
        Reg::Vcc => {
            scalars.extend([VCC, VCC + 1]);
        }
        Reg::Scc => {}
        Reg::M0 => {
            scalars.insert(M0);
        }
    };
    for block in &function.blocks {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(_, Expr::Get(r)) | Stmt::Set(r, _) => reg(r, &mut scalars, &mut vectors),
                Stmt::Let(_, Expr::GetPair(r)) | Stmt::SetPair(r, _) => {
                    scalars.extend([*r, r + 1]);
                }
                Stmt::SetLane(r, _) | Stmt::WriteLane(r, _, _) => {
                    vectors.insert(*r);
                }
                Stmt::Let(_, Expr::GetRelative(base, _)) => {
                    vectors.insert(0);
                    vectors.extend(*base..=255);
                }
                Stmt::SetRelative(base, _, _) => vectors.extend(*base..=255),
                _ => {}
            }
        }
    }
    (scalars, vectors)
}

pub fn harness(function: &Function, options: &HarnessOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Harness(options))
}

/// A compute shader following the runtime contract: push constants hold s0-s31, the heap base and the code
/// base; enabled workgroup IDs follow the user SGPRs; v0-v2 hold the thread ID; memory is heap base + Xbox VA.
pub fn compute(function: &Function, options: &ComputeOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Compute(options))
}

/// A fragment shader following the runtime contract: push constants as for compute; v0-v1 hold the
/// perspective barycentrics and the enabled position and front-face inputs follow; interpolated attribute N is
/// the per-vertex input at location N, as raw words; colour target N is the output at location N.
pub fn pixel(function: &Function, options: &PixelOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Pixel(options))
}

/// A vertex shader: user SGPRs from s0 as for compute; v0 holds the vertex index and v3 the instance index;
/// fetched attribute r fills v[4 + 4r] to v[7 + 4r] from the vertex input at location r; `pos0` writes the
/// position and parameter N the raw words at output location N.
pub fn vertex(function: &Function, options: &VertexOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Vertex(options))
}

/// A mesh shader from a primitive-shader program: user SGPRs from s8, the wave's index in s3 bits 24-27 and
/// the dispatch-wide thread index in v5. Each thread's position, parameter and primitive exports fill the
/// vertex and primitive slots of its index in the group; slots no primitive export reaches are culled.
pub fn mesh(function: &Function, options: &MeshOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Mesh(options))
}

/// A vertex shader from the first half of a merged vertex-and-geometry shader; see `EsOptions`.
pub fn es(function: &Function, options: &EsOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Es(options))
}

/// A geometry shader from the second half; see `GsOptions`. The first half's vertices arrive as its raw
/// output words and sit `item_bytes` apart in the LDS the halves share.
pub fn gs(function: &Function, options: &GsOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Gs(options))
}

fn build(function: &Function, entry: Entry<'_>) -> Result<Vec<u32>, EmitError> {
    let pixel_options = match entry {
        Entry::Pixel(options) => Some(options.clone()),
        _ => None,
    };
    let vertex_options = match entry {
        Entry::Vertex(options) => Some(options.clone()),
        _ => None,
    };
    let mesh_options = match entry {
        Entry::Mesh(options) => Some(*options),
        _ => None,
    };
    let es_options = match entry {
        Entry::Es(options) => Some(options.clone()),
        _ => None,
    };
    let gs_options = match entry {
        Entry::Gs(options) => Some(*options),
        _ => None,
    };
    let pixel_mode = pixel_options.is_some()
        || vertex_options.is_some()
        || es_options.is_some()
        || gs_options.is_some();
    let lanes: u32 = if function.wave64 && (!pixel_mode || function.upper_lanes) {
        64
    } else {
        32
    };
    let (float, group_size, lds_words) = match entry {
        Entry::Harness(options) => (
            FloatMode {
                nan_preserve: options.nan_preserve,
                flush_denormals: options.flush_denormals,
                preserve_denormals: options.preserve_denormals,
            },
            [lanes, 1, 1],
            options.lds_words,
        ),
        Entry::Compute(options) => {
            let words = (options.lds_bytes / 4).max(lanes).next_power_of_two();
            if words > 12 * 1024 {
                return Err(EmitError::Unsupported("LDS larger than 48 KiB"));
            }
            (options.float, options.group_size, words)
        }
        Entry::Pixel(options) => (options.float, [lanes, 1, 1], 32),
        Entry::Vertex(options) => (options.float, [lanes, 1, 1], 32),
        Entry::Es(options) => (options.float, [lanes, 1, 1], 32),
        Entry::Gs(options) => (options.float, [lanes, 1, 1], 32),
        Entry::Mesh(options) => {
            let words = (options.lds_bytes / 4).max(lanes).next_power_of_two();
            if words > 6 * 1024 {
                return Err(EmitError::Unsupported(
                    "mesh shared memory larger than 24 KiB",
                ));
            }
            (options.float, [options.group_size, 1, 1], words)
        }
    };
    let threads: u32 = group_size.iter().product();
    let compute_entry = matches!(entry, Entry::Compute(_));
    if threads == 0 || (!compute_entry && !threads.is_multiple_of(lanes)) {
        return Err(EmitError::Unsupported(
            "thread-group size is not a whole number of waves",
        ));
    }
    let padded = threads.next_multiple_of(lanes);
    let pair = lanes == 64;
    let invocations = if pair { padded / 2 } else { padded };
    let declared_size = if compute_entry || pair {
        [invocations, 1, 1]
    } else {
        group_size
    };
    let heap_mode = !matches!(entry, Entry::Harness(_));
    if !lds_words.is_power_of_two() {
        return Err(EmitError::LdsSize);
    }
    let types = function.verify()?;
    let mut b = Builder::new();
    if mesh_options.is_some() {
        b.set_version(1, 4);
    } else {
        b.set_version(1, 3);
    }
    for capability in [
        Capability::Shader,
        Capability::Int64,
        Capability::GroupNonUniform,
        Capability::GroupNonUniformBallot,
        Capability::GroupNonUniformShuffle,
    ] {
        b.capability(capability);
    }
    if heap_mode {
        b.capability(Capability::PhysicalStorageBufferAddresses);
        b.extension("SPV_KHR_physical_storage_buffer");
    }
    if float.nan_preserve {
        b.capability(Capability::SignedZeroInfNanPreserve);
    }
    if float.flush_denormals {
        b.capability(Capability::DenormFlushToZero);
    }
    if float.preserve_denormals {
        b.capability(Capability::DenormPreserve);
    }
    if float.nan_preserve || float.flush_denormals || float.preserve_denormals {
        b.extension("SPV_KHR_float_controls");
    }
    let glsl = b.ext_inst_import("GLSL.std.450");
    let addressing = if heap_mode {
        AddressingModel::PhysicalStorageBuffer64
    } else {
        AddressingModel::Logical
    };
    b.memory_model(addressing, MemoryModel::GLSL450);
    let bool_ = b.type_bool();
    let u32_ = b.type_int(32, 0);
    let u64_ = b.type_int(64, 0);
    let f32_ = b.type_float(32, None);
    let uvec3 = b.type_vector(u32_, 3);
    let uvec4 = b.type_vector(u32_, 4);
    let wide = b.type_struct([u32_, u32_]);
    let (mut state, mut memory, mut lds_image, mut exports, mut push) = (0, 0, 0, 0, 0);
    let mut storage_block = None;
    if heap_mode {
        let count = b.constant_bit32(u32_, entry.code_word() + 2);
        let array = b.type_array(u32_, count);
        b.decorate(array, Decoration::ArrayStride, [Operand::LiteralBit32(4)]);
        let block = b.type_struct([array]);
        b.decorate(block, Decoration::Block, []);
        b.member_decorate(block, 0, Decoration::Offset, [Operand::LiteralBit32(0)]);
        let ptr_block = b.type_pointer(None, StorageClass::PushConstant, block);
        push = b.variable(ptr_block, None, StorageClass::PushConstant, None);
    } else {
        let runtime = b.type_runtime_array(u32_);
        b.decorate(runtime, Decoration::ArrayStride, [Operand::LiteralBit32(4)]);
        let block = b.type_struct([runtime]);
        b.decorate(block, Decoration::Block, []);
        b.member_decorate(block, 0, Decoration::Offset, [Operand::LiteralBit32(0)]);
        let ptr_block = b.type_pointer(None, StorageClass::StorageBuffer, block);
        storage_block = Some(ptr_block);
        let buffer = |b: &mut Builder, binding: u32| {
            let var = b.variable(ptr_block, None, StorageClass::StorageBuffer, None);
            b.decorate(var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
            b.decorate(var, Decoration::Binding, [Operand::LiteralBit32(binding)]);
            var
        };
        state = buffer(&mut b, 0);
        memory = buffer(&mut b, 1);
        lds_image = buffer(&mut b, 2);
        exports = buffer(&mut b, 3);
    }
    let lds = if pixel_mode {
        0
    } else {
        let lds_len = b.constant_bit32(u32_, lds_words);
        let lds_array = b.type_array(u32_, lds_len);
        let ptr_lds = b.type_pointer(None, StorageClass::Workgroup, lds_array);
        b.variable(ptr_lds, None, StorageClass::Workgroup, None)
    };
    let ptr_input_u32 = b.type_pointer(None, StorageClass::Input, u32_);
    let ptr_input_uvec3 = b.type_pointer(None, StorageClass::Input, uvec3);
    let input = |b: &mut Builder, ty: Word, builtin: BuiltIn| {
        let var = b.variable(ty, None, StorageClass::Input, None);
        b.decorate(var, Decoration::BuiltIn, [Operand::BuiltIn(builtin)]);
        var
    };
    let lane_input = input(&mut b, ptr_input_u32, BuiltIn::SubgroupLocalInvocationId);
    if pixel_options.is_some() {
        b.decorate(lane_input, Decoration::Flat, []);
    }
    let (wave_input, group_input) = if pixel_mode {
        (0, 0)
    } else {
        (
            input(&mut b, ptr_input_u32, BuiltIn::SubgroupId),
            input(&mut b, ptr_input_uvec3, BuiltIn::WorkgroupId),
        )
    };
    let t = Types {
        bool_,
        u32_,
        u64_,
        f32_,
        uvec3,
        uvec4,
        wide,
        ptr_function_u32: b.type_pointer(None, StorageClass::Function, u32_),
        ptr_function_bool: b.type_pointer(None, StorageClass::Function, bool_),
        ptr_storage_u32: b.type_pointer(None, StorageClass::StorageBuffer, u32_),
        ptr_workgroup_u32: b.type_pointer(None, StorageClass::Workgroup, u32_),
        ptr_heap_u32: if heap_mode {
            b.type_pointer(None, StorageClass::PhysicalStorageBuffer, u32_)
        } else {
            0
        },
        ptr_push_u32: if heap_mode {
            b.type_pointer(None, StorageClass::PushConstant, u32_)
        } else {
            0
        },
        ptr_private_u32: b.type_pointer(None, StorageClass::Private, u32_),
    };
    let typed_loads = function
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .any(|stmt| matches!(stmt, Stmt::Let(_, Expr::TypedLoad { .. })));
    let layout_table = typed_loads.then(|| typed::layout_table(&mut b, u32_));
    let void = b.type_void();
    let function_type = b.type_function(void, []);
    let main = ok(b.begin_function(void, None, FunctionControl::NONE, function_type))?;
    let entry_label = ok(b.begin_block(None))?;
    let (scalars, vectors) = registers(function);
    let sgpr: BTreeMap<u8, Word> = scalars
        .iter()
        .map(|r| {
            (
                *r,
                b.variable(t.ptr_function_u32, None, StorageClass::Function, None),
            )
        })
        .collect();
    let vgpr: BTreeMap<u8, Word> = vectors
        .iter()
        .map(|r| {
            (
                *r,
                b.variable(t.ptr_function_u32, None, StorageClass::Function, None),
            )
        })
        .collect();
    let vgpr_hi: BTreeMap<u8, Word> = if pair {
        vectors
            .iter()
            .map(|r| {
                (
                    *r,
                    b.variable(t.ptr_function_u32, None, StorageClass::Function, None),
                )
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    let scc = b.variable(t.ptr_function_bool, None, StorageClass::Function, None);
    let current = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let iterations = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let flow = flow::flow(&mut b, function, t.ptr_function_bool);
    let messages = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let alive = b.variable(t.ptr_function_bool, None, StorageClass::Function, None);
    let stage_lds = match (&es_options, &gs_options) {
        (Some(options), _) => Some(merged::first_half_lds(&mut b, u32_, options)),
        (_, Some(options)) => Some(merged::second_half_lds(&mut b, u32_, options)),
        _ => None,
    };
    let emitted = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let local = ok(b.load(u32_, None, lane_input, None, []))?;
    let subgroup = if pixel_mode {
        b.constant_bit32(u32_, 0)
    } else {
        ok(b.load(u32_, None, wave_input, None, []))?
    };
    let upper = b.constant_bit32(u32_, 32);
    let lane_ids = [local, ok(b.i_add(u32_, None, local, upper))?];
    let lanes_constant = b.constant_bit32(u32_, lanes);
    let first_lane = ok(b.i_mul(u32_, None, subgroup, lanes_constant))?;
    let mut thread_indexes = [0; 2];
    for (part, lane) in lane_ids.iter().enumerate() {
        thread_indexes[part] = ok(b.i_add(u32_, None, first_lane, *lane))?;
    }
    let group_ids = if heap_mode && !pixel_mode {
        ok(b.load(uvec3, None, group_input, None, []))?
    } else {
        0
    };
    let mut interface = vec![lane_input];
    if !pixel_mode {
        interface.push(wave_input);
    }
    if heap_mode && !pixel_mode {
        interface.push(group_input);
    }
    let compute_options = match entry {
        Entry::Compute(options) => Some(*options),
        _ => None,
    };
    let code_base = match entry {
        Entry::Harness(options) => options.code_base,
        _ => 0,
    };
    let mut e = Emitter {
        b,
        glsl,
        t,
        constants: HashMap::new(),
        sgpr,
        vgpr,
        scc,
        lane: local,
        local,
        lanes,
        pair,
        phantom: pair && pixel_mode,
        part: 0,
        lane_ids,
        thread_indexes,
        varying: function.varying(),
        values_hi: vec![0; function.temps as usize],
        vgpr_hi,
        state,
        memory,
        lds_image,
        exports,
        messages,
        lds,
        lds_words,
        memory_last: 0,
        constant_values: HashMap::new(),
        layout_table,
        typed_cache: [None, None],
        float64: false,
        free_writes: shader_ir::free_writes(function, !heap_mode),
        free_write: false,
        frames: vec![(0, 0)],
        next_frame: 0,
        scalar_loads: HashMap::new(),
        descriptor_lookups: HashMap::new(),
        code_base,
        code_word: entry.code_word(),
        heap: None,
        code: None,
        push,
        label: entry_label,
        compute: compute_options,
        group_ids,
        thread_index: thread_indexes[0],
        values: vec![0; function.temps as usize],
        types,
        storage_block,
        image_types: None,
        views: HashMap::new(),
        atomic_views: HashMap::new(),
        compare_samplers: None,
        lookups: [None; 2],
        encode: [None; 2],
        image_cache: [None, None],
        pixel: None,
        geometry: None,
        stage_lds,
        merged: None,
        active_cache: [None, None],
    };
    if function.ordered && pixel_options.is_none() {
        return Err(EmitError::Unsupported(
            "ordered pixel shading outside a pixel shader",
        ));
    }
    if let Some(options) = pixel_options.clone() {
        e.pixel_prologue(options, alive)?;
        if function.ordered {
            e.b.capability(Capability::FragmentShaderPixelInterlockEXT);
            e.b.extension("SPV_EXT_fragment_shader_interlock");
            ok(e.b.begin_invocation_interlock_ext())?;
        }
    } else if let Some(options) = vertex_options.clone() {
        e.vertex_prologue(options)?;
    } else if let Some(options) = mesh_options {
        e.mesh_prologue(options)?;
    } else if let Some(options) = &es_options {
        e.first_half_prologue(options)?;
    } else if let Some(options) = &gs_options {
        e.second_half_prologue(options, emitted)?;
    } else if heap_mode {
        e.compute_prologue()?;
    } else {
        e.prologue()?;
    }
    if e.phantom {
        let zero = e.u32c(0);
        let upper: Vec<Word> = e.vgpr_hi.values().copied().collect();
        for var in upper {
            e.store(var, zero)?;
        }
    }
    let linear = function
        .blocks
        .iter()
        .enumerate()
        .all(|(id, block)| match block.terminator {
            Terminator::Jump(target) => target == id + 1,
            Terminator::Branch(..) => false,
            Terminator::End => true,
        });
    if linear {
        for (index, block) in function.blocks.iter().enumerate() {
            e.block_stmts(index, block)?;
            if block.terminator == Terminator::End {
                break;
            }
        }
    } else {
        if let Some(flow) = &flow {
            e.structured(function, flow, iterations)?;
        } else {
            e.dispatch(function, current, iterations)?;
        }
    }
    if !heap_mode {
        e.epilogue()?;
    }
    if es_options.is_some() {
        e.first_half_epilogue()?;
    }
    if pixel_options.is_some() {
        if function.ordered {
            ok(e.b.end_invocation_interlock_ext())?;
        }
        e.pixel_epilogue()?;
    }
    ok(e.b.ret())?;
    ok(e.b.end_function())?;
    e.lookup_bodies()?;
    e.encode_bodies()?;
    if let Some(state) = &e.merged {
        interface.extend(state.interface());
        match &gs_options {
            None => {
                e.b.entry_point(ExecutionModel::Vertex, main, "main", interface)
            }
            Some(options) => {
                e.b.entry_point(ExecutionModel::Geometry, main, "main", interface);
                e.b.execution_mode(main, ExecutionMode::Invocations, [1]);
                let input = match options.input {
                    GsInput::Points => ExecutionMode::InputPoints,
                    GsInput::Lines => ExecutionMode::InputLines,
                    GsInput::Triangles => ExecutionMode::Triangles,
                    GsInput::LinesAdjacency => ExecutionMode::InputLinesAdjacency,
                    GsInput::TrianglesAdjacency => ExecutionMode::InputTrianglesAdjacency,
                };
                e.b.execution_mode(main, input, []);
                e.b.execution_mode(main, ExecutionMode::OutputVertices, [options.max_vertices]);
                let output = match options.output {
                    GsOutput::Points => ExecutionMode::OutputPoints,
                    GsOutput::LineStrip => ExecutionMode::OutputLineStrip,
                    GsOutput::TriangleStrip => ExecutionMode::OutputTriangleStrip,
                };
                e.b.execution_mode(main, output, []);
            }
        }
    } else if let Some(state) = &e.geometry {
        interface.extend(state.interface());
        match mesh_options {
            Some(options) => {
                let everything: Vec<Word> =
                    e.b.module_ref()
                        .types_global_values
                        .iter()
                        .filter(|inst| inst.class.opcode == rspirv::spirv::Op::Variable)
                        .filter_map(|inst| inst.result_id)
                        .collect();
                e.b.entry_point(ExecutionModel::MeshEXT, main, "main", everything);
                e.b.execution_mode(main, ExecutionMode::LocalSize, declared_size);
                e.b.execution_mode(main, ExecutionMode::OutputVertices, [options.max_vertices]);
                e.b.execution_mode(
                    main,
                    ExecutionMode::OutputPrimitivesEXT,
                    [options.max_primitives],
                );
                e.b.execution_mode(main, ExecutionMode::OutputTrianglesEXT, []);
            }
            None => {
                e.b.entry_point(ExecutionModel::Vertex, main, "main", interface)
            }
        }
    } else if let Some(state) = &e.pixel {
        interface.extend(state.interface());
        e.b.entry_point(ExecutionModel::Fragment, main, "main", interface);
        e.b.execution_mode(main, ExecutionMode::OriginUpperLeft, []);
        if function.ordered {
            e.b.execution_mode(main, ExecutionMode::PixelInterlockOrderedEXT, []);
        }
        if let Some(depth) = pixel_options.as_ref().and_then(|o| o.depth) {
            e.b.execution_mode(main, ExecutionMode::DepthReplacing, []);
            match depth {
                DepthOutput::Any => {}
                DepthOutput::GreaterEqual => {
                    e.b.execution_mode(main, ExecutionMode::DepthGreater, [])
                }
                DepthOutput::LessEqual => e.b.execution_mode(main, ExecutionMode::DepthLess, []),
            }
        }
    } else {
        e.b.entry_point(ExecutionModel::GLCompute, main, "main", interface);
        e.b.execution_mode(main, ExecutionMode::LocalSize, declared_size);
    }
    if float.nan_preserve {
        e.b.execution_mode(main, ExecutionMode::SignedZeroInfNanPreserve, [32]);
    }
    if float.flush_denormals {
        e.b.execution_mode(main, ExecutionMode::DenormFlushToZero, [32]);
    }
    if float.preserve_denormals {
        e.b.execution_mode(main, ExecutionMode::DenormPreserve, [32]);
    }
    Ok(e.b.module().assemble())
}

impl Emitter {
    fn u32c(&mut self, value: u32) -> Word {
        let ty = self.t.u32_;
        let id = *self
            .constants
            .entry((0, u64::from(value)))
            .or_insert_with(|| self.b.constant_bit32(ty, value));
        self.constant_values.insert(id, value);
        id
    }

    fn doubles(&mut self) -> Word {
        if !self.float64 {
            self.float64 = true;
            self.b.capability(Capability::Float64);
        }
        self.b.type_float(64, None)
    }

    fn u64c(&mut self, value: u64) -> Word {
        let ty = self.t.u64_;
        *self
            .constants
            .entry((1, value))
            .or_insert_with(|| self.b.constant_bit64(ty, value))
    }

    fn f32c(&mut self, value: f32) -> Word {
        let ty = self.t.f32_;
        *self
            .constants
            .entry((2, u64::from(value.to_bits())))
            .or_insert_with(|| self.b.constant_bit32(ty, value.to_bits()))
    }

    fn boolc(&mut self, value: bool) -> Word {
        let ty = self.t.bool_;
        *self
            .constants
            .entry((3, u64::from(value)))
            .or_insert_with(|| {
                if value {
                    self.b.constant_true(ty)
                } else {
                    self.b.constant_false(ty)
                }
            })
    }

    fn glsl(&mut self, ty: Word, op: GLOp, args: &[Word]) -> Result<Word, EmitError> {
        let glsl = self.glsl;
        ok(self.b.ext_inst(
            ty,
            None,
            glsl,
            op as u32,
            args.iter().map(|a| Operand::IdRef(*a)),
        ))
    }

    fn element(&mut self, buffer: Word, index: Word) -> Result<Word, EmitError> {
        let zero = self.u32c(0);
        let ptr = self.t.ptr_storage_u32;
        ok(self.b.access_chain(ptr, None, buffer, [zero, index]))
    }

    fn lds_element(&mut self, index: Word) -> Result<Word, EmitError> {
        let ptr = self.t.ptr_workgroup_u32;
        let lds = self.lds;
        ok(self.b.access_chain(ptr, None, lds, [index]))
    }

    fn load(&mut self, ptr: Word) -> Result<Word, EmitError> {
        let ty = self.t.u32_;
        ok(self.b.load(ty, None, ptr, None, []))
    }

    fn store(&mut self, ptr: Word, value: Word) -> Result<(), EmitError> {
        self.scalar_loads.remove(&ptr);
        ok(self.b.store(ptr, value, None, []))
    }

    fn barrier(&mut self) -> Result<(), EmitError> {
        let workgroup = self.u32c(2);
        let semantics = self.u32c(0x108);
        ok(self.b.control_barrier(workgroup, workgroup, semantics))
    }

    fn if_then(
        &mut self,
        condition: Word,
        body: impl FnOnce(&mut Self) -> Result<(), EmitError>,
    ) -> Result<(), EmitError> {
        let then = self.b.id();
        let merge = self.b.id();
        self.selection(merge)?;
        ok(self.b.branch_conditional(condition, then, merge, []))?;
        self.begin(then)?;
        body(self)?;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        Ok(())
    }

    fn state_vgpr(&mut self, reg: u8) -> Result<Word, EmitError> {
        let base = self.u32c(STATE_VGPR + 64 * u32::from(reg));
        let u32_ = self.t.u32_;
        let index = ok(self.b.i_add(u32_, None, base, self.lane))?;
        let state = self.state;
        self.element(state, index)
    }

    fn copy_lds(&mut self, inward: bool) -> Result<(), EmitError> {
        for part in self.parts() {
            self.set_part(part);
            self.copy_lds_part(inward)?;
        }
        self.set_part(0);
        Ok(())
    }

    /// The parts each invocation carries: both with lane pairs, else one.
    pub(super) fn parts(&self) -> std::ops::Range<usize> {
        0..if self.pair { 2 } else { 1 }
    }

    fn copy_lds_part(&mut self, inward: bool) -> Result<(), EmitError> {
        let u32_ = self.t.u32_;
        let lanes = self.lanes;
        for chunk in 0..self.lds_words / lanes {
            let base = self.u32c(lanes * chunk);
            let index = ok(self.b.i_add(u32_, None, base, self.lane))?;
            let image = self.lds_image;
            let outside = self.element(image, index)?;
            let inside = self.lds_element(index)?;
            if inward {
                let value = self.load(outside)?;
                self.store(inside, value)?;
            } else {
                let value = self.load(inside)?;
                self.store(outside, value)?;
            }
        }
        Ok(())
    }

    fn prologue(&mut self) -> Result<(), EmitError> {
        let zero = self.u32c(0);
        self.store(self.messages, zero)?;
        let state = self.state;
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = self.u32c(u32::from(reg));
            let ptr = self.element(state, index)?;
            let value = self.load(ptr)?;
            self.store(var, value)?;
        }
        let registers: Vec<u8> = self.vgpr.keys().copied().collect();
        for part in self.parts() {
            self.set_part(part);
            for reg in &registers {
                let ptr = self.state_vgpr(*reg)?;
                let value = self.load(ptr)?;
                self.store(self.vgpr_var(*reg), value)?;
            }
        }
        self.set_part(0);
        let index = self.u32c(STATE_SCC);
        let ptr = self.element(state, index)?;
        let raw = self.load(ptr)?;
        let zero = self.u32c(0);
        let bool_ = self.t.bool_;
        let flag = ok(self.b.i_not_equal(bool_, None, raw, zero))?;
        self.store(self.scc, flag)?;
        let u32_ = self.t.u32_;
        let memory = self.memory;
        let length = ok(self.b.array_length(u32_, None, memory, 0))?;
        let one = self.u32c(1);
        self.memory_last = ok(self.b.i_sub(u32_, None, length, one))?;
        self.copy_lds(true)?;
        self.barrier()
    }

    fn epilogue(&mut self) -> Result<(), EmitError> {
        self.barrier()?;
        self.copy_lds(false)?;
        let registers: Vec<u8> = self.vgpr.keys().copied().collect();
        for part in self.parts() {
            self.set_part(part);
            for reg in &registers {
                let value = self.load(self.vgpr_var(*reg))?;
                let ptr = self.state_vgpr(*reg)?;
                self.store(ptr, value)?;
            }
        }
        self.set_part(0);
        let zero = self.u32c(0);
        let bool_ = self.t.bool_;
        let first = ok(self.b.i_equal(bool_, None, self.lane, zero))?;
        self.if_then(first, |e| {
            let state = e.state;
            let scalars: Vec<(u8, Word)> = e.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
            for (reg, var) in scalars {
                let value = e.load(var)?;
                let index = e.u32c(u32::from(reg));
                let ptr = e.element(state, index)?;
                e.store(ptr, value)?;
            }
            let bool_ = e.t.bool_;
            let flag = ok(e.b.load(bool_, None, e.scc, None, []))?;
            let (one, zero) = (e.u32c(1), e.u32c(0));
            let u32_ = e.t.u32_;
            let value = ok(e.b.select(u32_, None, flag, one, zero))?;
            let index = e.u32c(STATE_SCC);
            let ptr = e.element(state, index)?;
            e.store(ptr, value)?;
            let count = e.load(e.messages)?;
            let index = e.u32c(MESSAGE_BASE);
            let exports = e.exports;
            let ptr = e.element(exports, index)?;
            e.store(ptr, count)
        })
    }

    fn dispatch(
        &mut self,
        function: &Function,
        current: Word,
        iterations: Word,
    ) -> Result<(), EmitError> {
        let zero = self.u32c(0);
        self.store(current, zero)?;
        self.store(iterations, zero)?;
        let (header, body, switch_merge, cont, merge) = (
            self.b.id(),
            self.b.id(),
            self.b.id(),
            self.b.id(),
            self.b.id(),
        );
        let default = self.b.id();
        let cases: Vec<Word> = function.blocks.iter().map(|_| self.b.id()).collect();
        ok(self.b.branch(header))?;
        self.begin(header)?;
        self.looping(merge, cont)?;
        ok(self.b.branch(body))?;
        self.begin(body)?;
        let selector = self.load(current)?;
        self.selection(switch_merge)?;
        let targets: Vec<(Operand, Word)> = cases
            .iter()
            .enumerate()
            .map(|(i, label)| (Operand::LiteralBit32(i as u32), *label))
            .collect();
        ok(self.b.switch(selector, default, targets))?;
        for (index, (block, label)) in function.blocks.iter().zip(&cases).enumerate() {
            self.begin(*label)?;
            self.block_stmts(index, block)?;
            let next = match block.terminator {
                Terminator::Jump(target) => self.u32c(target as u32),
                Terminator::End => self.u32c(END),
                Terminator::Branch(condition, taken, fallthrough) => {
                    let condition = self.val(condition);
                    let (taken, fallthrough) =
                        (self.u32c(taken as u32), self.u32c(fallthrough as u32));
                    let u32_ = self.t.u32_;
                    ok(self.b.select(u32_, None, condition, taken, fallthrough))?
                }
            };
            self.store(current, next)?;
            ok(self.b.branch(switch_merge))?;
        }
        self.begin(default)?;
        let end = self.u32c(END);
        self.store(current, end)?;
        ok(self.b.branch(switch_merge))?;
        self.begin(switch_merge)?;
        ok(self.b.branch(cont))?;
        self.begin(cont)?;
        let u32_ = self.t.u32_;
        let bool_ = self.t.bool_;
        let count = self.load(iterations)?;
        let one = self.u32c(1);
        let count = ok(self.b.i_add(u32_, None, count, one))?;
        self.store(iterations, count)?;
        let limit = self.u32c(ITERATION_LIMIT);
        let exhausted = ok(self.b.u_greater_than_equal(bool_, None, count, limit))?;
        let next = self.load(current)?;
        let finished = ok(self.b.i_equal(bool_, None, next, end))?;
        let leave = ok(self.b.logical_or(bool_, None, finished, exhausted))?;
        ok(self.b.branch_conditional(leave, merge, header, []))?;
        self.begin(merge)?;
        Ok(())
    }

    fn sgpr_var(&self, reg: u8) -> Word {
        self.sgpr[&reg]
    }

    fn pack(&mut self, low: Word, high: Word) -> Result<Word, EmitError> {
        let u64_ = self.t.u64_;
        let low = ok(self.b.u_convert(u64_, None, low))?;
        let high = ok(self.b.u_convert(u64_, None, high))?;
        let shift = self.u32c(32);
        let high = ok(self.b.shift_left_logical(u64_, None, high, shift))?;
        ok(self.b.bitwise_or(u64_, None, low, high))
    }

    fn get_pair(&mut self, reg: u8) -> Result<Word, EmitError> {
        let low = self.load(self.sgpr_var(reg))?;
        let high = self.load(self.sgpr_var(reg + 1))?;
        self.pack(low, high)
    }

    fn set_pair(&mut self, reg: u8, value: Word) -> Result<(), EmitError> {
        let (low, high) = self.split(value)?;
        self.store(self.sgpr_var(reg), low)?;
        self.store(self.sgpr_var(reg + 1), high)
    }

    fn split(&mut self, value: Word) -> Result<(Word, Word), EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let low = ok(self.b.u_convert(u32_, None, value))?;
        let shift = self.u32c(32);
        let high = ok(self.b.shift_right_logical(u64_, None, value, shift))?;
        let high = ok(self.b.u_convert(u32_, None, high))?;
        Ok((low, high))
    }

    fn exec_word(&mut self) -> Result<Word, EmitError> {
        let low = self.load(self.sgpr_var(EXEC))?;
        if self.lanes == 32 {
            return Ok(low);
        }
        if self.part == 1 {
            self.load(self.sgpr_var(EXEC + 1))
        } else {
            Ok(low)
        }
    }

    fn begin(&mut self, label: Word) -> Result<(), EmitError> {
        ok(self.b.begin_block(Some(label)))?;
        self.label = label;
        let open = self.frames.len() > 1;
        match self.frames.last_mut() {
            Some(top) if top.0 == label => {
                self.frames.pop();
            }
            Some(top) if open => {
                self.next_frame += 1;
                top.1 = self.next_frame;
            }
            _ => {}
        }
        self.image_cache = [None, None];
        self.active_cache = [None, None];
        Ok(())
    }

    /// Opens a selection: the blocks until `merge` neither see values made in each other nor are seen after it.
    fn selection(&mut self, merge: Word) -> Result<(), EmitError> {
        self.next_frame += 1;
        self.frames.push((merge, self.next_frame));
        ok(self.b.selection_merge(merge, SelectionControl::NONE))
    }

    /// Opens a loop; registers read before it may change before its body runs again.
    fn looping(&mut self, merge: Word, next: Word) -> Result<(), EmitError> {
        self.next_frame += 1;
        self.frames.push((merge, self.next_frame));
        self.scalar_loads.clear();
        ok(self.b.loop_merge(merge, next, LoopControl::NONE, []))
    }

    fn frame(&self) -> u32 {
        self.frames.last().map_or(0, |top| top.1)
    }

    /// Whether a value made in `frame` dominates the block being written.
    pub(super) fn visible(&self, frame: u32) -> bool {
        self.frames.iter().any(|top| top.1 == frame)
    }

    fn scalar_load(&mut self, var: Word) -> Result<Word, EmitError> {
        if let Some((frame, id)) = self.scalar_loads.get(&var).copied()
            && self.visible(frame)
        {
            return Ok(id);
        }
        let id = self.load(var)?;
        self.scalar_loads.insert(var, (self.frame(), id));
        Ok(id)
    }

    fn storage_block(&mut self) -> Word {
        if let Some(block) = self.storage_block {
            return block;
        }
        let u32_ = self.t.u32_;
        let runtime = self.b.type_runtime_array(u32_);
        self.b
            .decorate(runtime, Decoration::ArrayStride, [Operand::LiteralBit32(4)]);
        let block = self.b.type_struct([runtime]);
        self.b.decorate(block, Decoration::Block, []);
        self.b
            .member_decorate(block, 0, Decoration::Offset, [Operand::LiteralBit32(0)]);
        let ptr = self
            .b
            .type_pointer(None, StorageClass::StorageBuffer, block);
        self.storage_block = Some(ptr);
        ptr
    }

    fn push_word(&mut self, index: u32) -> Result<Word, EmitError> {
        let ptr = self.t.ptr_push_u32;
        let zero = self.u32c(0);
        let index = self.u32c(index);
        let push = self.push;
        let ptr = ok(self.b.access_chain(ptr, None, push, [zero, index]))?;
        self.load(ptr)
    }

    fn heap_pointer(&mut self, address: Word) -> Result<Word, EmitError> {
        let u64_ = self.t.u64_;
        let base = self.heap.expect("heap mode");
        let sum = ok(self.b.i_add(u64_, None, base, address))?;
        let ptr = self.t.ptr_heap_u32;
        ok(self.b.convert_u_to_ptr(ptr, None, sum))
    }

    fn heap_load(&mut self, address: Word) -> Result<Word, EmitError> {
        let ptr = self.heap_pointer(address)?;
        let u32_ = self.t.u32_;
        ok(self.b.load(
            u32_,
            None,
            ptr,
            Some(rspirv::spirv::MemoryAccess::ALIGNED),
            [Operand::LiteralBit32(4)],
        ))
    }

    fn heap_store(&mut self, address: Word, value: Word) -> Result<(), EmitError> {
        let ptr = self.heap_pointer(address)?;
        ok(self.b.store(
            ptr,
            value,
            Some(rspirv::spirv::MemoryAccess::ALIGNED),
            [Operand::LiteralBit32(4)],
        ))
    }

    fn guarded_loads(
        &mut self,
        condition: Word,
        addresses: &[Word],
    ) -> Result<Vec<Word>, EmitError> {
        let before = self.label;
        let then = self.b.id();
        let merge = self.b.id();
        self.selection(merge)?;
        ok(self.b.branch_conditional(condition, then, merge, []))?;
        self.begin(then)?;
        let mut loaded = Vec::new();
        for address in addresses {
            loaded.push(self.heap_load(*address)?);
        }
        let end = self.label;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        let zero = self.u32c(0);
        let u32_ = self.t.u32_;
        loaded
            .into_iter()
            .map(|value| ok(self.b.phi(u32_, None, [(value, end), (zero, before)])))
            .collect()
    }

    fn compute_prologue(&mut self) -> Result<(), EmitError> {
        let options = self.compute.expect("compute mode");
        let u32_ = self.t.u32_;
        let low = self.push_word(PUSH_HEAP)?;
        let high = self.push_word(PUSH_HEAP + 1)?;
        self.heap = Some(self.pack(low, high)?);
        let low = self.push_word(self.code_word)?;
        let high = self.push_word(self.code_word + 1)?;
        self.code = Some(self.pack(low, high)?);
        let mut group_slots = Vec::new();
        for (axis, enabled) in options.workgroup_ids.iter().enumerate() {
            if *enabled {
                group_slots.push(axis as u32);
            }
        }
        let threads: u32 = options.group_size.iter().product();
        let exec = if threads.is_multiple_of(self.lanes) {
            None
        } else {
            Some(self.partial_exec(threads)?)
        };
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = u32::from(reg);
            let value = if index < options.user_sgprs.min(32) {
                self.push_word(index)?
            } else if let (Some((low, high)), true) = (exec, reg == EXEC || reg == EXEC + 1) {
                if reg == EXEC { low } else { high }
            } else if let Some(axis) = index
                .checked_sub(options.user_sgprs)
                .and_then(|k| group_slots.get(k as usize))
            {
                ok(self
                    .b
                    .composite_extract(u32_, None, self.group_ids, [*axis]))?
            } else if reg == EXEC || (reg == EXEC + 1 && self.lanes == 64) {
                self.u32c(u32::MAX)
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        let registers: Vec<u8> = self.vgpr.keys().copied().collect();
        let [width, height, _] = options.group_size;
        for part in self.parts() {
            self.set_part(part);
            let flat = self.thread_index;
            let (width_c, height_c) = (self.u32c(width), self.u32c(height));
            let plane = self.u32c(width * height);
            let x = ok(self.b.u_mod(u32_, None, flat, width_c))?;
            let row = ok(self.b.u_div(u32_, None, flat, width_c))?;
            let y = ok(self.b.u_mod(u32_, None, row, height_c))?;
            let z = ok(self.b.u_div(u32_, None, flat, plane))?;
            for reg in &registers {
                let value = match reg {
                    0..=2 if u32::from(*reg) < options.thread_ids.min(3) => {
                        [x, y, z][*reg as usize]
                    }
                    _ => self.u32c(0),
                };
                self.store(self.vgpr_var(*reg), value)?;
            }
        }
        let no = self.boolc(false);
        self.store(self.scc, no)?;
        let zero = self.u32c(0);
        self.store(self.messages, zero)?;
        for part in self.parts() {
            self.set_part(part);
            for chunk in 0..self.lds_words.div_ceil(threads) {
                let base = self.u32c(threads * chunk);
                let index = ok(self.b.i_add(u32_, None, base, self.thread_index))?;
                let limit = self.u32c(self.lds_words);
                let bool_ = self.t.bool_;
                let inside = ok(self.b.u_less_than(bool_, None, index, limit))?;
                self.if_then(inside, |e| {
                    let ptr = e.lds_element(index)?;
                    let zero = e.u32c(0);
                    e.store(ptr, zero)
                })?;
            }
        }
        self.set_part(0);
        self.barrier()
    }

    /// Exec for a group that ends partway through a wave: one bit per lane holding one of the group's threads.
    fn partial_exec(&mut self, threads: u32) -> Result<(Word, Word), EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let lanes = self.u32c(self.lanes);
        let wave = ok(self.b.u_div(u32_, None, self.thread_index, lanes))?;
        let first = ok(self.b.i_mul(u32_, None, wave, lanes))?;
        let total = self.u32c(threads);
        let remaining = ok(self.b.i_sub(u32_, None, total, first))?;
        let mask = |e: &mut Self, count: Word| -> Result<Word, EmitError> {
            let thirty_two = e.u32c(32);
            let full = ok(e.b.u_greater_than_equal(bool_, None, count, thirty_two))?;
            let one = e.u32c(1);
            let bit = ok(e.b.shift_left_logical(u32_, None, one, count))?;
            let below = ok(e.b.i_sub(u32_, None, bit, one))?;
            let all = e.u32c(u32::MAX);
            ok(e.b.select(u32_, None, full, all, below))
        };
        let low = mask(self, remaining)?;
        let thirty_two = self.u32c(32);
        let over = ok(self.b.u_greater_than(bool_, None, remaining, thirty_two))?;
        let rest = ok(self.b.i_sub(u32_, None, remaining, thirty_two))?;
        let zero = self.u32c(0);
        let rest = ok(self.b.select(u32_, None, over, rest, zero))?;
        let high = if self.lanes == 64 {
            mask(self, rest)?
        } else {
            zero
        };
        Ok((low, high))
    }

    /// Whether this lane is on in exec; reused within a block until a register write might change exec.
    fn active(&mut self) -> Result<Word, EmitError> {
        if let Some((label, value)) = self.active_cache[self.part]
            && label == self.label
        {
            return Ok(value);
        }
        let exec = self.exec_word()?;
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let shifted = ok(self.b.shift_right_logical(u32_, None, exec, self.local))?;
        let one = self.u32c(1);
        let bit = ok(self.b.bitwise_and(u32_, None, shifted, one))?;
        let zero = self.u32c(0);
        let value = ok(self.b.i_not_equal(bool_, None, bit, zero))?;
        self.active_cache[self.part] = Some((self.label, value));
        Ok(value)
    }

    fn float(&mut self, value: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        ok(self.b.bitcast(f32_, None, value))
    }

    fn bits(&mut self, value: Word) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        ok(self.b.bitcast(u32_, None, value))
    }

    fn exact(&mut self, id: Word) -> Word {
        self.b.decorate(id, Decoration::NoContraction, []);
        id
    }

    fn masked_shift(&mut self, amount: Word, mask: u32) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let mask = self.u32c(mask);
        ok(self.b.bitwise_and(u32_, None, amount, mask))
    }

    fn element_words(&mut self, address: Word) -> Result<[Word; 4], EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let low = ok(self.b.u_convert(u32_, None, address))?;
        let three = self.u32c(3);
        let misaligned = ok(self.b.bitwise_and(u32_, None, low, three))?;
        let shift = ok(self.b.shift_left_logical(u32_, None, misaligned, three))?;
        let first = self.memory_index(address)?;
        let last = self.memory_last;
        let mut dwords = Vec::new();
        for k in 0..5 {
            let step = self.u32c(k);
            let index = ok(self.b.i_add(u32_, None, first, step))?;
            let index = self.glsl(u32_, GLOp::UMin, &[index, last])?;
            let memory = self.memory;
            let ptr = self.element(memory, index)?;
            dwords.push(self.load(ptr)?);
        }
        let mut words = [0; 4];
        for (k, word) in words.iter_mut().enumerate() {
            let pair = self.pack(dwords[k], dwords[k + 1])?;
            let moved = ok(self.b.shift_right_logical(u64_, None, pair, shift))?;
            *word = ok(self.b.u_convert(u32_, None, moved))?;
        }
        Ok(words)
    }

    fn half_to_single(&mut self, half: Word, quiet: bool) -> Result<Word, EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let fifteen = c(self, 15);
        let sign = ok(self.b.shift_right_logical(u32_, None, half, fifteen))?;
        let one = c(self, 1);
        let sign = ok(self.b.bitwise_and(u32_, None, sign, one))?;
        let thirty_one = c(self, 31);
        let sign = ok(self.b.shift_left_logical(u32_, None, sign, thirty_one))?;
        let ten = c(self, 10);
        let exponent = ok(self.b.shift_right_logical(u32_, None, half, ten))?;
        let five_bits = c(self, 0x1F);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, five_bits))?;
        let ten_bits = c(self, 0x3FF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, half, ten_bits))?;
        let thirteen = c(self, 13);
        let wide = ok(self.b.shift_left_logical(u32_, None, mantissa, thirteen))?;
        let bias = c(self, 112);
        let rebased = ok(self.b.i_add(u32_, None, exponent, bias))?;
        let twenty_three = c(self, 23);
        let rebased = ok(self.b.shift_left_logical(u32_, None, rebased, twenty_three))?;
        let normal = ok(self.b.bitwise_or(u32_, None, rebased, wide))?;
        let special = c(self, if quiet { 0x7FC0_0000 } else { 0x7F80_0000 });
        let empty = c(self, 0);
        let is_nan_payload = ok(self.b.i_not_equal(bool_, None, mantissa, empty))?;
        let infinity = c(self, 0x7F80_0000);
        let special = ok(self.b.select(u32_, None, is_nan_payload, special, infinity))?;
        let special = if quiet {
            special
        } else {
            ok(self.b.bitwise_or(u32_, None, special, wide))?
        };
        let float = ok(self.b.convert_u_to_f(f32_, None, mantissa))?;
        let scale = self.f32c(f32::from_bits(0x3380_0000));
        let small = ok(self.b.f_mul(f32_, None, float, scale))?;
        let small = self.exact(small);
        let small = self.bits(small)?;
        let top = ok(self.b.i_equal(bool_, None, exponent, five_bits))?;
        let bottom = ok(self.b.i_equal(bool_, None, exponent, empty))?;
        let magnitude = ok(self.b.select(u32_, None, bottom, small, normal))?;
        let magnitude = ok(self.b.select(u32_, None, top, special, magnitude))?;
        ok(self.b.bitwise_or(u32_, None, sign, magnitude))
    }

    fn single_to_half(&mut self, single: Word, mode: Round) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let thirty_one = c(self, 31);
        let top_bit = ok(self.b.shift_right_logical(u32_, None, single, thirty_one))?;
        let zero = c(self, 0);
        let negative = ok(self.b.i_not_equal(bool_, None, top_bit, zero))?;
        let sixteen = c(self, 16);
        let sign = ok(self.b.shift_right_logical(u32_, None, single, sixteen))?;
        let sign_bit = c(self, 0x8000);
        let sign = ok(self.b.bitwise_and(u32_, None, sign, sign_bit))?;
        let twenty_three = c(self, 23);
        let exponent = ok(self.b.shift_right_logical(u32_, None, single, twenty_three))?;
        let byte = c(self, 0xFF);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, byte))?;
        let fraction = c(self, 0x7F_FFFF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, single, fraction))?;
        let one = c(self, 1);
        let bias = c(self, 112);
        let biased = ok(self.b.i_sub(u32_, None, exponent, bias))?;
        let away = |e: &mut Self, rest: Word, half: Word, odd: Word| -> Result<Word, EmitError> {
            let none = e.u32c(0);
            let inexact = ok(e.b.i_not_equal(bool_, None, rest, none))?;
            Ok(match mode {
                Round::Even => {
                    let above = ok(e.b.u_greater_than(bool_, None, rest, half))?;
                    let tie = ok(e.b.i_equal(bool_, None, rest, half))?;
                    let tie = ok(e.b.logical_and(bool_, None, tie, odd))?;
                    ok(e.b.logical_or(bool_, None, above, tie))?
                }
                Round::Up => {
                    let positive = ok(e.b.logical_not(bool_, None, negative))?;
                    ok(e.b.logical_and(bool_, None, inexact, positive))?
                }
                Round::Down => ok(e.b.logical_and(bool_, None, inexact, negative))?,
                Round::Zero => e.boolc(false),
            })
        };
        let round = |e: &mut Self, value: Word, shift: Word| -> Result<Word, EmitError> {
            let quotient = ok(e.b.shift_right_logical(u32_, None, value, shift))?;
            let one = e.u32c(1);
            let bit = ok(e.b.shift_left_logical(u32_, None, one, shift))?;
            let low = ok(e.b.i_sub(u32_, None, bit, one))?;
            let rest = ok(e.b.bitwise_and(u32_, None, value, low))?;
            let half = ok(e.b.shift_right_logical(u32_, None, bit, one))?;
            let odd = ok(e.b.bitwise_and(u32_, None, quotient, one))?;
            let none = e.u32c(0);
            let odd = ok(e.b.i_not_equal(bool_, None, odd, none))?;
            let up = away(e, rest, half, odd)?;
            let step = ok(e.b.select(u32_, None, up, one, none))?;
            ok(e.b.i_add(u32_, None, quotient, step))
        };
        let (max_positive, max_negative) =
            (c(self, mode.overflow(false)), c(self, mode.overflow(true)));
        let largest = ok(self
            .b
            .select(u32_, None, negative, max_negative, max_positive))?;
        let packed = ok(self.b.shift_left_logical(u32_, None, biased, twenty_three))?;
        let packed = ok(self.b.bitwise_or(u32_, None, packed, mantissa))?;
        let thirteen = c(self, 13);
        let normal = round(self, packed, thirteen)?;
        let normal = self.glsl(u32_, GLOp::UMin, &[normal, largest])?;
        let implicit = c(self, 0x80_0000);
        let full = ok(self.b.bitwise_or(u32_, None, mantissa, implicit))?;
        let fourteen = c(self, 14);
        let shift = ok(self.b.i_sub(u32_, None, fourteen, biased))?;
        let shift = self.masked_shift(shift, 31)?;
        let tiny = round(self, full, shift)?;
        let magnitude = c(self, 0x7FFF_FFFF);
        let magnitude = ok(self.b.bitwise_and(u32_, None, single, magnitude))?;
        let nonzero = ok(self.b.i_not_equal(bool_, None, magnitude, zero))?;
        let sticky = ok(self.b.select(u32_, None, nonzero, one, zero))?;
        let never = self.boolc(false);
        let up = away(self, sticky, one, never)?;
        let vanished = ok(self.b.select(u32_, None, up, one, zero))?;
        let infinity = c(self, 0x7C00);
        let nan = c(self, 0x7E00);
        let empty = ok(self.b.i_equal(bool_, None, mantissa, zero))?;
        let special = ok(self.b.select(u32_, None, empty, infinity, nan))?;
        let top = ok(self.b.i_equal(bool_, None, exponent, byte))?;
        let overflow = ok(self.b.s_greater_than_equal(bool_, None, biased, thirty_one))?;
        let positive = ok(self.b.s_greater_than_equal(bool_, None, biased, one))?;
        let minus_ten = c(self, (-10i32) as u32);
        let underflow = ok(self.b.s_less_than(bool_, None, biased, minus_ten))?;
        let value = ok(self.b.select(u32_, None, positive, normal, tiny))?;
        let value = ok(self.b.select(u32_, None, underflow, vanished, value))?;
        let value = ok(self.b.select(u32_, None, overflow, largest, value))?;
        let value = ok(self.b.select(u32_, None, top, special, value))?;
        ok(self.b.bitwise_or(u32_, None, sign, value))
    }
    fn convert_field(
        &mut self,
        layout: &rdna::format::BufferFormat,
        channel: usize,
        field: Word,
    ) -> Result<Word, EmitError> {
        use rdna::format::Number;
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let width = u32::from(layout.widths[channel]);
        let signed = if width == 32 {
            field
        } else {
            let spare = self.u32c(32 - width);
            let up = ok(self.b.shift_left_logical(u32_, None, field, spare))?;
            ok(self.b.shift_right_arithmetic(u32_, None, up, spare))?
        };
        Ok(match layout.number {
            Number::Uint => field,
            Number::Sint => signed,
            Number::Uscaled => {
                let f = ok(self.b.convert_u_to_f(f32_, None, field))?;
                self.bits(f)?
            }
            Number::Sscaled => {
                let f = ok(self.b.convert_s_to_f(f32_, None, signed))?;
                self.bits(f)?
            }
            Number::Unorm | Number::Snorm => {
                let f64_ = self.doubles();
                let max = if layout.number == Number::Unorm {
                    (1u64 << width) - 1
                } else {
                    (1u64 << (width - 1)) - 1
                };
                let numerator = if layout.number == Number::Unorm {
                    ok(self.b.convert_u_to_f(f64_, None, field))?
                } else {
                    ok(self.b.convert_s_to_f(f64_, None, signed))?
                };
                let denominator = self.b.constant_bit64(f64_, (max as f64).to_bits());
                let quotient = ok(self.b.f_div(f64_, None, numerator, denominator))?;
                let quotient = self.exact(quotient);
                let quotient = if layout.number == Number::Snorm {
                    let floor = self.b.constant_bit64(f64_, (-1.0f64).to_bits());
                    self.glsl(f64_, GLOp::NMax, &[quotient, floor])?
                } else {
                    quotient
                };
                let narrow = ok(self.b.f_convert(f32_, None, quotient))?;
                self.bits(narrow)?
            }
            Number::Float if width == 32 => field,
            Number::Float => {
                let half = if width == 16 {
                    field
                } else {
                    let shift = self.u32c(15 - width);
                    ok(self.b.shift_left_logical(u32_, None, field, shift))?
                };
                self.half_to_single(half, false)?
            }
        })
    }

    fn memory_index(&mut self, address: Word) -> Result<Word, EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let two = self.u32c(2);
        let word = ok(self.b.shift_right_logical(u64_, None, address, two))?;
        let word = ok(self.b.u_convert(u32_, None, word))?;
        let last = self.memory_last;
        self.glsl(u32_, GLOp::UMin, &[word, last])
    }

    fn lds_index(&mut self, address: Word) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let two = self.u32c(2);
        let word = ok(self.b.shift_right_logical(u32_, None, address, two))?;
        self.masked_shift(word, self.lds_words - 1)
    }

    /// One statement; with lane pairs, per-lane work runs for part 0 and then part 1, in program order, and
    /// wave-wide values come from both parts at once.
    fn block_stmts(&mut self, index: usize, block: &Block) -> Result<(), EmitError> {
        let mut at = 0;
        while at < block.stmts.len() {
            self.free_write = self.free_writes[index][at];
            let run = self.load_run(block, at);
            if run > 1 {
                self.load_batch(&block.stmts[at..at + run])?;
                at += run;
                continue;
            }
            self.stmt(&block.stmts[at])?;
            at += 1;
        }
        self.free_write = false;
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), EmitError> {
        if !matches!(stmt, Stmt::Let(_, Expr::TypedLoad { .. })) {
            self.typed_cache = [None, None];
        }
        if !self.pair {
            return self.stmt_body(stmt);
        }
        let per_lane = match stmt {
            Stmt::Let(tmp, expr) => {
                if expr.wave_wide() {
                    self.set_part(0);
                    let id = self.pair_expr(expr)?;
                    self.values[*tmp as usize] = id;
                    self.values_hi[*tmp as usize] = id;
                    return Ok(());
                }
                self.varying[*tmp as usize]
            }
            Stmt::SetLane(..)
            | Stmt::SetRelative(..)
            | Stmt::WriteLane(..)
            | Stmt::LdsStore(..)
            | Stmt::Store { .. }
            | Stmt::TypedStore { .. }
            | Stmt::ImageStore { .. }
            | Stmt::Export { .. } => true,
            _ => false,
        };
        let parts = match stmt {
            Stmt::Export { .. } if self.phantom => 1,
            _ => 2,
        };
        if per_lane {
            for part in 0..parts {
                self.set_part(part);
                self.stmt_body(stmt)?;
            }
        } else {
            self.set_part(0);
            self.stmt_body(stmt)?;
            if let Stmt::Let(tmp, _) = stmt {
                self.values_hi[*tmp as usize] = self.values[*tmp as usize];
            }
        }
        self.set_part(0);
        Ok(())
    }

    pub(super) fn set_part(&mut self, part: usize) {
        self.part = part;
        self.lane = self.lane_ids[part];
        self.thread_index = self.thread_indexes[part];
    }

    /// A temporary's value in the current part.
    pub(super) fn val(&self, tmp: u32) -> Word {
        if self.part == 1 && self.varying[tmp as usize] {
            self.values_hi[tmp as usize]
        } else {
            self.values[tmp as usize]
        }
    }

    fn set_val(&mut self, tmp: u32, id: Word) {
        if self.part == 1 {
            self.values_hi[tmp as usize] = id;
        } else {
            self.values[tmp as usize] = id;
        }
    }

    /// A temporary's value in both parts.
    fn both(&self, tmp: u32) -> [Word; 2] {
        let hi = if self.varying[tmp as usize] {
            self.values_hi[tmp as usize]
        } else {
            self.values[tmp as usize]
        };
        [self.values[tmp as usize], hi]
    }

    pub(super) fn vgpr_var(&self, reg: u8) -> Word {
        if self.part == 1 {
            self.vgpr_hi[&reg]
        } else {
            self.vgpr[&reg]
        }
    }

    /// Wave-wide values of a lane-pair wave, read from both parts of each invocation.
    fn pair_expr(&mut self, expr: &Expr) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let scope = self.u32c(3);
        Ok(match expr {
            Expr::Ballot(condition) => {
                let conditions = self.both(*condition);
                let mut words = [0; 2];
                for part in 0..2 {
                    self.set_part(part);
                    let active = self.active()?;
                    let both = ok(self.b.logical_and(bool_, None, conditions[part], active))?;
                    let uvec4 = self.t.uvec4;
                    let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, both))?;
                    words[part] = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
                }
                self.set_part(0);
                self.pack(words[0], words[1])?
            }
            Expr::ReadLane(source, lane) => {
                let target = self.masked_shift(self.val(*lane), 63)?;
                let index = self.masked_shift(target, 31)?;
                let sources = self.both(*source);
                let mut values = [0; 2];
                for (part, source) in sources.into_iter().enumerate() {
                    values[part] = ok(self
                        .b
                        .group_non_uniform_shuffle(u32_, None, scope, source, index))?;
                }
                let thirty_two = self.u32c(32);
                let upper = ok(self.b.u_greater_than_equal(bool_, None, target, thirty_two))?;
                ok(self.b.select(u32_, None, upper, values[1], values[0]))?
            }
            _ => self.expr(expr)?,
        })
    }

    fn stmt_body(&mut self, stmt: &Stmt) -> Result<(), EmitError> {
        let v = |e: &Self, t: &u32| e.val(*t);
        match stmt {
            Stmt::Let(tmp, expr) => {
                let id = self.expr(expr)?;
                self.set_val(*tmp, id);
            }
            Stmt::Barrier if self.pixel.is_some() || self.geometry.is_some() => {}
            Stmt::Barrier if self.stage_lds.is_some() => {}
            Stmt::Barrier => self.barrier()?,
            Stmt::Set(reg, tmp) => {
                self.active_cache = [None, None];
                let value = v(self, tmp);
                match reg {
                    Reg::S(r) => self.store(self.sgpr_var(*r), value)?,
                    Reg::M0 => self.store(self.sgpr_var(M0), value)?,
                    Reg::V(r) => self.store(self.vgpr_var(*r), value)?,
                    Reg::Exec => self.set_pair(EXEC, value)?,
                    Reg::Vcc => self.set_pair(VCC, value)?,
                    Reg::Scc => self.store(self.scc, value)?,
                }
            }
            Stmt::SetPair(reg, tmp) => {
                self.active_cache = [None, None];
                self.set_pair(*reg, v(self, tmp))?
            }
            Stmt::SetLane(reg, tmp) if self.free_write => {
                self.store(self.vgpr_var(*reg), v(self, tmp))?;
            }
            Stmt::SetLane(reg, tmp) => {
                let active = self.active()?;
                let var = self.vgpr_var(*reg);
                let old = self.load(var)?;
                let u32_ = self.t.u32_;
                let value = ok(self.b.select(u32_, None, active, v(self, tmp), old))?;
                self.store(var, value)?;
            }
            Stmt::SetRelative(base, index, value) => {
                let u32_ = self.t.u32_;
                let first = self.u32c(u32::from(*base));
                let at = ok(self.b.i_add(u32_, None, first, v(self, index)))?;
                let active = self.active()?;
                let new = v(self, value);
                let merge = self.b.id();
                let labels: Vec<(u8, Word)> = (*base..=255u8).map(|r| (r, self.b.id())).collect();
                self.selection(merge)?;
                let targets: Vec<(Operand, Word)> = labels
                    .iter()
                    .map(|(r, label)| (Operand::LiteralBit32(u32::from(*r)), *label))
                    .collect();
                ok(self.b.switch(at, merge, targets))?;
                for (reg, label) in labels {
                    self.begin(label)?;
                    let var = self.vgpr_var(reg);
                    let old = self.load(var)?;
                    let chosen = ok(self.b.select(u32_, None, active, new, old))?;
                    self.store(var, chosen)?;
                    ok(self.b.branch(merge))?;
                }
                self.begin(merge)?;
            }
            Stmt::WriteLane(reg, lane, value) => {
                let target = self.masked_shift(v(self, lane), self.lanes - 1)?;
                let bool_ = self.t.bool_;
                let hit = ok(self.b.i_equal(bool_, None, target, self.lane))?;
                let var = self.vgpr_var(*reg);
                let old = self.load(var)?;
                let u32_ = self.t.u32_;
                let new = ok(self.b.select(u32_, None, hit, v(self, value), old))?;
                self.store(var, new)?;
            }
            Stmt::Export {
                target,
                values,
                done,
                compressed,
            } if self.pixel.is_some() => {
                let values = values.map(|v| v.map(|t| self.val(t)));
                self.pixel_export(*target, values, *done, *compressed)?;
            }
            Stmt::Message(..) | Stmt::FetchVertexInputs if self.pixel.is_some() => {}
            Stmt::Export { target, values, .. } if self.geometry.is_some() => {
                let values = values.map(|v| v.map(|t| self.val(t)));
                self.geometry_export(*target, values)?;
            }
            Stmt::Message(..) | Stmt::FetchVertexInputs if self.geometry.is_some() => {}
            Stmt::Message(code, _) if self.merged.is_some() => {
                if self.merged.as_ref().is_some_and(|m| m.second_half()) {
                    self.second_half_message(*code)?;
                }
            }
            Stmt::FetchVertexInputs if self.merged.is_some() => {}
            Stmt::Export { .. } if self.merged.is_some() => {
                return Err(EmitError::Unsupported(
                    "exports from a merged vertex-and-geometry shader",
                ));
            }
            Stmt::LdsStore(address, value) if self.stage_lds.is_some() => {
                let active = self.active()?;
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(active, |e| {
                    let ptr = e.merged_lds_element(address, true)?;
                    e.store(ptr, value)
                })?;
            }
            Stmt::LdsStore(..) if self.pixel.is_some() => {
                return Err(EmitError::Unsupported("LDS in a pixel shader"));
            }
            Stmt::Export { .. } | Stmt::Message(..) if self.heap.is_some() => {
                return Err(EmitError::Unsupported(
                    "exports and messages in a compute shader",
                ));
            }
            Stmt::Export { target, values, .. } => {
                let active = self.active()?;
                let values = *values;
                let target = *target;
                let resolved: Vec<Option<Word>> =
                    values.iter().map(|v| v.map(|t| self.val(t))).collect();
                self.if_then(active, |e| {
                    let exports = e.exports;
                    for (component, value) in resolved.iter().enumerate() {
                        if let Some(value) = value {
                            let base = e.u32c((u32::from(target) * 4 + component as u32) * 64);
                            let u32_ = e.t.u32_;
                            let index = ok(e.b.i_add(u32_, None, base, e.lane))?;
                            let ptr = e.element(exports, index)?;
                            e.store(ptr, *value)?;
                        }
                    }
                    Ok(())
                })?;
            }
            Stmt::FetchVertexInputs => {}
            Stmt::Message(code, m0) => {
                let m0 = v(self, m0);
                let code = *code;
                let zero = self.u32c(0);
                let bool_ = self.t.bool_;
                let first = ok(self.b.i_equal(bool_, None, self.lane, zero))?;
                let counter = self.messages;
                let count = self.load(counter)?;
                let limit = self.u32c(MESSAGE_LIMIT);
                let room = ok(self.b.u_less_than(bool_, None, count, limit))?;
                let first = ok(self.b.logical_and(bool_, None, first, room))?;
                self.if_then(first, |e| {
                    let u32_ = e.t.u32_;
                    let base = e.u32c(MESSAGE_BASE + 1);
                    let two = e.u32c(2);
                    let at = ok(e.b.i_mul(u32_, None, count, two))?;
                    let at = ok(e.b.i_add(u32_, None, at, base))?;
                    let exports = e.exports;
                    let ptr = e.element(exports, at)?;
                    let code = e.u32c(code);
                    e.store(ptr, code)?;
                    let one = e.u32c(1);
                    let next = ok(e.b.i_add(u32_, None, at, one))?;
                    let ptr = e.element(exports, next)?;
                    e.store(ptr, m0)
                })?;
                let one = self.u32c(1);
                let u32_ = self.t.u32_;
                let count = ok(self.b.i_add(u32_, None, count, one))?;
                self.store(counter, count)?;
            }
            Stmt::LdsStore(address, value) => {
                let active = self.active()?;
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(active, |e| {
                    let index = e.lds_index(address)?;
                    let ptr = e.lds_element(index)?;
                    e.store(ptr, value)
                })?;
            }
            Stmt::TypedStore {
                format,
                address,
                in_range,
                values,
                d16,
            } => {
                let values = values.map(|t| v(self, &t));
                let (format, address, in_range) =
                    (v(self, format), v(self, address), v(self, in_range));
                self.typed_store(format, address, in_range, values, *d16)?;
            }
            Stmt::ImageStore { image, values } => {
                let active = self.active()?;
                let image = (**image).clone();
                let values = *values;
                self.if_then(active, |e| e.image_store(&image, values))?;
            }
            Stmt::Store {
                when,
                address,
                value,
            } => {
                let mut condition = self.active()?;
                if let Some(when) = when {
                    let bool_ = self.t.bool_;
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(condition, |e| {
                    if e.heap.is_some() {
                        return e.heap_store(address, value);
                    }
                    let index = e.memory_index(address)?;
                    let memory = e.memory;
                    let ptr = e.element(memory, index)?;
                    e.store(ptr, value)
                })?;
            }
        }
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<Word, EmitError> {
        let v = |e: &Self, t: &u32| e.val(*t);
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        Ok(match expr {
            Expr::Const(c) => self.u32c(*c),
            Expr::Const64(c) => self.u64c(*c),
            Expr::Get(reg) => match reg {
                Reg::S(r) => self.scalar_load(self.sgpr_var(*r))?,
                Reg::M0 => self.load(self.sgpr_var(M0))?,
                Reg::V(r) => self.load(self.vgpr_var(*r))?,
                Reg::Exec => self.get_pair(EXEC)?,
                Reg::Vcc => self.get_pair(VCC)?,
                Reg::Scc => ok(self.b.load(bool_, None, self.scc, None, []))?,
            },
            Expr::GetPair(r) => self.get_pair(*r)?,
            Expr::Un(op, a) => self.unary(*op, v(self, a))?,
            Expr::Bin(op, a, b) => self.binary(*op, v(self, a), v(self, b))?,
            Expr::Tri(op, a, b, c) => self.ternary(*op, v(self, a), v(self, b), v(self, c))?,
            Expr::Cmp(op, a, b) => self.compare(*op, v(self, a), v(self, b))?,
            Expr::Select(c, a, b) => {
                let ty = match self.types[*a as usize] {
                    Ty::Bool => bool_,
                    Ty::U32 => u32_,
                    Ty::U64 => u64_,
                };
                ok(self.b.select(ty, None, v(self, c), v(self, a), v(self, b)))?
            }
            Expr::LaneId => self.lane,
            Expr::CodeBase => match self.code {
                Some(code) => code,
                None => self.u64c(self.code_base),
            },
            Expr::Active => {
                if self.lanes == 64 {
                    self.get_pair(EXEC)?
                } else {
                    let exec = self.load(self.sgpr_var(EXEC))?;
                    ok(self.b.u_convert(u64_, None, exec))?
                }
            }
            Expr::Ballot(a) => {
                let active = self.active()?;
                let both = ok(self.b.logical_and(bool_, None, v(self, a), active))?;
                let scope = self.u32c(3);
                let uvec4 = self.t.uvec4;
                let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, both))?;
                let mine = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
                ok(self.b.u_convert(u64_, None, mine))?
            }
            Expr::Shuffle(source, lane) => {
                let lane = self.masked_shift(v(self, lane), self.lanes - 1)?;
                let scope = self.u32c(3);
                if self.pair {
                    let index = self.masked_shift(lane, 31)?;
                    let mut values = [0; 2];
                    for (part, source) in self.both(*source).into_iter().enumerate() {
                        values[part] = ok(self
                            .b
                            .group_non_uniform_shuffle(u32_, None, scope, source, index))?;
                    }
                    let thirty_two = self.u32c(32);
                    let upper = ok(self.b.u_greater_than_equal(bool_, None, lane, thirty_two))?;
                    ok(self.b.select(u32_, None, upper, values[1], values[0]))?
                } else {
                    let source = v(self, source);
                    ok(self
                        .b
                        .group_non_uniform_shuffle(u32_, None, scope, source, lane))?
                }
            }
            Expr::GetRelative(base, index) => {
                let base_value = self.u32c(u32::from(*base));
                let at = ok(self.b.i_add(u32_, None, base_value, v(self, index)))?;
                let limit = self.u32c(256);
                let inside = ok(self.b.u_less_than(bool_, None, at, limit))?;
                let zero = self.u32c(0);
                let at = ok(self.b.select(u32_, None, inside, at, zero))?;
                let first = self.vgpr_var(0);
                let mut value = self.load(first)?;
                for reg in *base..=255u8 {
                    if reg == 0 {
                        continue;
                    }
                    let candidate = self.vgpr_var(reg);
                    let candidate = self.load(candidate)?;
                    let number = self.u32c(u32::from(reg));
                    let hit = ok(self.b.i_equal(bool_, None, at, number))?;
                    value = ok(self.b.select(u32_, None, hit, candidate, value))?;
                }
                value
            }
            Expr::ReadLane(source, lane) => {
                let lane = self.masked_shift(v(self, lane), 31)?;
                let scope = self.u32c(3);
                ok(self
                    .b
                    .group_non_uniform_shuffle(u32_, None, scope, v(self, source), lane))?
            }
            Expr::FirstActiveLane => {
                let exec = self.load(self.sgpr_var(EXEC))?;
                let lowest = self.glsl(u32_, GLOp::FindILsb, &[exec])?;
                let zero = self.u32c(0);
                let none = ok(self.b.i_equal(bool_, None, exec, zero))?;
                if self.lanes == 32 {
                    ok(self.b.select(u32_, None, none, zero, lowest))?
                } else {
                    let high = self.load(self.sgpr_var(EXEC + 1))?;
                    let upper = self.glsl(u32_, GLOp::FindILsb, &[high])?;
                    let thirty_two = self.u32c(32);
                    let upper = ok(self.b.i_add(u32_, None, upper, thirty_two))?;
                    let high_none = ok(self.b.i_equal(bool_, None, high, zero))?;
                    let upper = ok(self.b.select(u32_, None, high_none, zero, upper))?;
                    ok(self.b.select(u32_, None, none, upper, lowest))?
                }
            }
            Expr::LdsLoad(_) if self.pixel.is_some() => {
                return Err(EmitError::Unsupported("LDS in a pixel shader"));
            }
            Expr::LdsLoad(address) if self.stage_lds.is_some() => {
                let ptr = self.merged_lds_element(v(self, address), false)?;
                let value = self.load(ptr)?;
                let active = self.active()?;
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, active, value, zero))?
            }
            Expr::LdsLoad(address) => {
                let index = self.lds_index(v(self, address))?;
                let ptr = self.lds_element(index)?;
                let value = self.load(ptr)?;
                let active = self.active()?;
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, active, value, zero))?
            }
            Expr::Load { when, address } if self.heap.is_some() => {
                let mut condition = self.active()?;
                if let Some(when) = when {
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                self.guarded_loads(condition, &[v(self, address)])?[0]
            }
            Expr::ScalarLoad(address) if self.heap.is_some() => self.heap_load(v(self, address))?,
            Expr::Load { when, address } => {
                let index = self.memory_index(v(self, address))?;
                let memory = self.memory;
                let ptr = self.element(memory, index)?;
                let value = self.load(ptr)?;
                let mut condition = self.active()?;
                if let Some(when) = when {
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, condition, value, zero))?
            }
            Expr::TypedLoad {
                format,
                address,
                in_range,
                select,
                component,
            } => self.typed_load(*format, *address, *in_range, *select, *component)?,
            Expr::ScalarLoad(address) => {
                let index = self.memory_index(v(self, address))?;
                let memory = self.memory;
                let ptr = self.element(memory, index)?;
                self.load(ptr)?
            }
            Expr::Image(image) => self.image(image)?,
            Expr::Atomic {
                space,
                op,
                address,
                value,
                compare,
                when,
            } => {
                let compare = compare.map(|t| v(self, &t));
                let when = when.map(|t| v(self, &t));
                let (address, value) = (v(self, address), v(self, value));
                self.atomic(*space, *op, address, value, compare, when)?
            }
            Expr::Attribute {
                attr,
                channel,
                vertex,
            } if self.pixel.is_some() => self.pixel_attribute_word(*attr, *channel, *vertex)?,
            Expr::Attribute {
                attr,
                channel,
                vertex,
            } => {
                if self.heap.is_some() || self.compute.is_some() {
                    return Err(EmitError::Unsupported(
                        "interpolated attributes outside a pixel shader",
                    ));
                }
                let slot = shader_ir::attribute_slot(*attr, *channel, *vertex) as u32;
                let base = self.u32c(STATE_ATTRIBUTES + 64 * slot);
                let u32_ = self.t.u32_;
                let index = ok(self.b.i_add(u32_, None, base, self.lane))?;
                let state = self.state;
                let ptr = self.element(state, index)?;
                self.load(ptr)?
            }
        })
    }

    fn float_unary(&mut self, op: GLOp, a: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        let x = self.float(a)?;
        let r = self.glsl(f32_, op, &[x])?;
        self.bits(r)
    }

    fn saturate(
        &mut self,
        a: Word,
        low: f32,
        high: f32,
        top: u32,
        signed: bool,
    ) -> Result<Word, EmitError> {
        let (f32_, u32_, bool_) = (self.t.f32_, self.t.u32_, self.t.bool_);
        let x = self.float(a)?;
        let (lo, hi) = (self.f32c(low), self.f32c(high));
        let clamped = self.glsl(f32_, GLOp::NMax, &[x, lo])?;
        let clamped = self.glsl(f32_, GLOp::NMin, &[clamped, hi])?;
        let converted = if signed {
            ok(self.b.convert_f_to_s(u32_, None, clamped))?
        } else {
            ok(self.b.convert_f_to_u(u32_, None, clamped))?
        };
        let limit = self.f32c(if signed {
            2_147_483_648.0
        } else {
            4_294_967_296.0
        });
        let over = ok(self.b.f_ord_greater_than_equal(bool_, None, x, limit))?;
        let top = self.u32c(top);
        ok(self.b.select(u32_, None, over, top, converted))
    }

    fn unary(&mut self, op: Un, a: Word) -> Result<Word, EmitError> {
        let (u32_, u64_, f32_, bool_) = (self.t.u32_, self.t.u64_, self.t.f32_, self.t.bool_);
        Ok(match op {
            Un::Not => ok(self.b.not(u32_, None, a))?,
            Un::F16ToF32 => {
                let mask = self.u32c(0xFFFF);
                let half = ok(self.b.bitwise_and(u32_, None, a, mask))?;
                self.half_to_single(half, true)?
            }
            Un::F32ToF16(round) => self.single_to_half(a, round)?,
            Un::FCanonical => {
                let one = self.u32c(1.0f32.to_bits());
                self.float_binary(Bin::FMul, a, one)?
            }
            Un::Not64 => ok(self.b.not(u64_, None, a))?,
            Un::CvtF32F64 => {
                self.b.capability(Capability::Float64);
                let f64_ = self.b.type_float(64, None);
                let double = ok(self.b.bitcast(f64_, None, a))?;
                let single = ok(self.b.f_convert(f32_, None, double))?;
                let single = self.exact(single);
                self.bits(single)?
            }
            Un::Bfrev => ok(self.b.bit_reverse(u32_, None, a))?,
            Un::Ffbl => self.glsl(u32_, GLOp::FindILsb, &[a])?,
            Un::Ffbh => {
                let msb = self.glsl(u32_, GLOp::FindUMsb, &[a])?;
                let thirty_one = self.u32c(31);
                let zeros = ok(self.b.i_sub(u32_, None, thirty_one, msb))?;
                let zero = self.u32c(0);
                let empty = ok(self.b.i_equal(bool_, None, a, zero))?;
                let all = self.u32c(u32::MAX);
                ok(self.b.select(u32_, None, empty, all, zeros))?
            }
            Un::Popcount => ok(self.b.bit_count(u32_, None, a))?,
            Un::Popcount64 => {
                let (low, high) = self.split(a)?;
                let low = ok(self.b.bit_count(u32_, None, low))?;
                let high = ok(self.b.bit_count(u32_, None, high))?;
                ok(self.b.i_add(u32_, None, low, high))?
            }
            Un::Lo => ok(self.b.u_convert(u32_, None, a))?,
            Un::Hi => self.split(a)?.1,
            Un::Zext => ok(self.b.u_convert(u64_, None, a))?,
            Un::BoolToU32 => {
                let (one, zero) = (self.u32c(1), self.u32c(0));
                ok(self.b.select(u32_, None, a, one, zero))?
            }
            Un::CvtF32U32 => {
                let f = ok(self.b.convert_u_to_f(f32_, None, a))?;
                self.bits(f)?
            }
            Un::CvtF32I32 => {
                let f = ok(self.b.convert_s_to_f(f32_, None, a))?;
                self.bits(f)?
            }
            Un::CvtU32F32 => self.saturate(a, 0.0, 4_294_967_040.0, u32::MAX, false)?,
            Un::CvtI32F32 => {
                self.saturate(a, -2_147_483_648.0, 2_147_483_520.0, i32::MAX as u32, true)?
            }
            Un::FNeg => {
                let sign = self.u32c(0x8000_0000);
                ok(self.b.bitwise_xor(u32_, None, a, sign))?
            }
            Un::FAbs => {
                let magnitude = self.u32c(0x7FFF_FFFF);
                ok(self.b.bitwise_and(u32_, None, a, magnitude))?
            }
            Un::FClamp01 => {
                let x = self.float(a)?;
                let (zero, one) = (self.f32c(0.0), self.f32c(1.0));
                let positive = ok(self.b.f_ord_greater_than(bool_, None, x, zero))?;
                let capped = self.glsl(f32_, GLOp::NMin, &[x, one])?;
                let r = ok(self.b.select(f32_, None, positive, capped, zero))?;
                self.bits(r)?
            }
            Un::FRcp | Un::FRsq => {
                let x = self.float(a)?;
                let r = if op == Un::FRcp {
                    let one = self.f32c(1.0);
                    ok(self.b.f_div(f32_, None, one, x))?
                } else {
                    self.glsl(f32_, GLOp::InverseSqrt, &[x])?
                };
                self.bits(r)?
            }
            Un::FSqrt => self.float_unary(GLOp::Sqrt, a)?,
            Un::FExp2 => self.float_unary(GLOp::Exp2, a)?,
            Un::FLog2 => self.float_unary(GLOp::Log2, a)?,
            Un::FFloor => self.float_unary(GLOp::Floor, a)?,
            Un::FCeil => self.float_unary(GLOp::Ceil, a)?,
            Un::FTrunc => self.float_unary(GLOp::Trunc, a)?,
            Un::FRndne => self.float_unary(GLOp::RoundEven, a)?,
            Un::FSin | Un::FCos => {
                let x = self.float(a)?;
                let tau = self.f32c(std::f32::consts::TAU);
                let turns = ok(self.b.f_mul(f32_, None, x, tau))?;
                let turns = self.exact(turns);
                let op = if op == Un::FSin { GLOp::Sin } else { GLOp::Cos };
                let r = self.glsl(f32_, op, &[turns])?;
                self.bits(r)?
            }
            Un::FFract => {
                let x = self.float(a)?;
                let floor = self.glsl(f32_, GLOp::Floor, &[x])?;
                let r = ok(self.b.f_sub(f32_, None, x, floor))?;
                let r = self.exact(r);
                self.bits(r)?
            }
        })
    }

    fn float_binary(&mut self, op: Bin, a: Word, b: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        let (x, y) = (self.float(a)?, self.float(b)?);
        let r = match op {
            Bin::FAdd => ok(self.b.f_add(f32_, None, x, y))?,
            Bin::FSub => ok(self.b.f_sub(f32_, None, x, y))?,
            Bin::FMul => ok(self.b.f_mul(f32_, None, x, y))?,
            Bin::FMin => self.glsl(f32_, GLOp::NMin, &[x, y])?,
            _ => self.glsl(f32_, GLOp::NMax, &[x, y])?,
        };
        let r = self.exact(r);
        self.bits(r)
    }

    fn binary(&mut self, op: Bin, a: Word, b: Word) -> Result<Word, EmitError> {
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        Ok(match op {
            Bin::Add => ok(self.b.i_add(u32_, None, a, b))?,
            Bin::Sub => ok(self.b.i_sub(u32_, None, a, b))?,
            Bin::Mul => ok(self.b.i_mul(u32_, None, a, b))?,
            Bin::MulHiU => {
                let wide = self.t.wide;
                let both = ok(self.b.u_mul_extended(wide, None, a, b))?;
                ok(self.b.composite_extract(u32_, None, both, [1]))?
            }
            Bin::And => ok(self.b.bitwise_and(u32_, None, a, b))?,
            Bin::Or => ok(self.b.bitwise_or(u32_, None, a, b))?,
            Bin::Xor => ok(self.b.bitwise_xor(u32_, None, a, b))?,
            Bin::Shl | Bin::ShrU | Bin::ShrS => {
                let amount = self.masked_shift(b, 31)?;
                match op {
                    Bin::Shl => ok(self.b.shift_left_logical(u32_, None, a, amount))?,
                    Bin::ShrU => ok(self.b.shift_right_logical(u32_, None, a, amount))?,
                    _ => ok(self.b.shift_right_arithmetic(u32_, None, a, amount))?,
                }
            }
            Bin::MinU => self.glsl(u32_, GLOp::UMin, &[a, b])?,
            Bin::MaxU => self.glsl(u32_, GLOp::UMax, &[a, b])?,
            Bin::MinS => self.glsl(u32_, GLOp::SMin, &[a, b])?,
            Bin::MaxS => self.glsl(u32_, GLOp::SMax, &[a, b])?,
            Bin::FAdd | Bin::FSub | Bin::FMul | Bin::FMin | Bin::FMax => {
                self.float_binary(op, a, b)?
            }
            Bin::Add64 => ok(self.b.i_add(u64_, None, a, b))?,
            Bin::DAdd => {
                self.b.capability(Capability::Float64);
                let f64_ = self.b.type_float(64, None);
                let x = ok(self.b.bitcast(f64_, None, a))?;
                let y = ok(self.b.bitcast(f64_, None, b))?;
                let sum = ok(self.b.f_add(f64_, None, x, y))?;
                let sum = self.exact(sum);
                ok(self.b.bitcast(u64_, None, sum))?
            }
            Bin::Sub64 => ok(self.b.i_sub(u64_, None, a, b))?,
            Bin::And64 => ok(self.b.bitwise_and(u64_, None, a, b))?,
            Bin::Or64 => ok(self.b.bitwise_or(u64_, None, a, b))?,
            Bin::Xor64 => ok(self.b.bitwise_xor(u64_, None, a, b))?,
            Bin::Shl64 | Bin::ShrU64 => {
                let mask = self.u64c(63);
                let amount = ok(self.b.bitwise_and(u64_, None, b, mask))?;
                if op == Bin::Shl64 {
                    ok(self.b.shift_left_logical(u64_, None, a, amount))?
                } else {
                    ok(self.b.shift_right_logical(u64_, None, a, amount))?
                }
            }
            Bin::Pack64 => self.pack(a, b)?,
            Bin::LaneBit => {
                let amount = self.masked_shift(b, 63)?;
                let shifted = ok(self.b.shift_right_logical(u64_, None, a, amount))?;
                let one = self.u64c(1);
                let bit = ok(self.b.bitwise_and(u64_, None, shifted, one))?;
                let zero = self.u64c(0);
                ok(self.b.i_not_equal(bool_, None, bit, zero))?
            }
        })
    }

    fn ternary(&mut self, op: Tri, a: Word, b: Word, c: Word) -> Result<Word, EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        Ok(match op {
            Tri::FFma => {
                let (x, y, z) = (self.float(a)?, self.float(b)?, self.float(c)?);
                let r = self.glsl(f32_, GLOp::Fma, &[x, y, z])?;
                self.bits(r)?
            }
            Tri::FMad => {
                let product = self.float_binary(Bin::FMul, a, b)?;
                self.float_binary(Bin::FAdd, product, c)?
            }
            Tri::BfeU => {
                let width = self.masked_shift(c, 31)?;
                let offset = self.masked_shift(b, 31)?;
                let shifted = ok(self.b.shift_right_logical(u32_, None, a, offset))?;
                let thirty_two = self.u32c(32);
                let spare = ok(self.b.i_sub(u32_, None, thirty_two, width))?;
                let spare = self.masked_shift(spare, 31)?;
                let all = self.u32c(u32::MAX);
                let mask = ok(self.b.shift_right_logical(u32_, None, all, spare))?;
                let field = ok(self.b.bitwise_and(u32_, None, shifted, mask))?;
                let zero = self.u32c(0);
                let empty = ok(self.b.i_equal(bool_, None, width, zero))?;
                ok(self.b.select(u32_, None, empty, zero, field))?
            }
        })
    }

    fn class(&mut self, bits: Word, mask: Word) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let thirty_one = c(self, 31);
        let sign = ok(self.b.shift_right_logical(u32_, None, bits, thirty_one))?;
        let zero = c(self, 0);
        let negative = ok(self.b.i_not_equal(bool_, None, sign, zero))?;
        let twenty_three = c(self, 23);
        let exponent = ok(self.b.shift_right_logical(u32_, None, bits, twenty_three))?;
        let byte = c(self, 0xFF);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, byte))?;
        let fraction = c(self, 0x7F_FFFF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, bits, fraction))?;
        let quiet_bit = c(self, 0x40_0000);
        let quiet = ok(self.b.bitwise_and(u32_, None, bits, quiet_bit))?;
        let quiet = ok(self.b.i_not_equal(bool_, None, quiet, zero))?;
        let top = ok(self.b.i_equal(bool_, None, exponent, byte))?;
        let bottom = ok(self.b.i_equal(bool_, None, exponent, zero))?;
        let empty = ok(self.b.i_equal(bool_, None, mantissa, zero))?;
        let pick = |e: &mut Self, when: Word, yes: u32, no: u32| -> Result<Word, EmitError> {
            let (yes, no) = (e.u32c(yes), e.u32c(no));
            ok(e.b.select(u32_, None, when, yes, no))
        };
        let normal = pick(self, negative, 3, 8)?;
        let denormal = pick(self, negative, 4, 7)?;
        let zero_class = pick(self, negative, 5, 6)?;
        let infinity = pick(self, negative, 2, 9)?;
        let nan = pick(self, quiet, 1, 0)?;
        let small = ok(self.b.select(u32_, None, empty, zero_class, denormal))?;
        let large = ok(self.b.select(u32_, None, empty, infinity, nan))?;
        let index = ok(self.b.select(u32_, None, bottom, small, normal))?;
        let index = ok(self.b.select(u32_, None, top, large, index))?;
        let shifted = ok(self.b.shift_right_logical(u32_, None, mask, index))?;
        let one = c(self, 1);
        let bit = ok(self.b.bitwise_and(u32_, None, shifted, one))?;
        ok(self.b.i_not_equal(bool_, None, bit, zero))
    }

    fn compare(&mut self, op: Cmp, a: Word, b: Word) -> Result<Word, EmitError> {
        let bool_ = self.t.bool_;
        let b_ = &mut self.b;
        let integer = match op {
            Cmp::Eq | Cmp::Eq64 => Some(ok(b_.i_equal(bool_, None, a, b))?),
            Cmp::Ne | Cmp::Ne64 => Some(ok(b_.i_not_equal(bool_, None, a, b))?),
            Cmp::LtU => Some(ok(b_.u_less_than(bool_, None, a, b))?),
            Cmp::LeU => Some(ok(b_.u_less_than_equal(bool_, None, a, b))?),
            Cmp::GtU => Some(ok(b_.u_greater_than(bool_, None, a, b))?),
            Cmp::GeU => Some(ok(b_.u_greater_than_equal(bool_, None, a, b))?),
            Cmp::LtS => Some(ok(b_.s_less_than(bool_, None, a, b))?),
            Cmp::LeS => Some(ok(b_.s_less_than_equal(bool_, None, a, b))?),
            Cmp::GtS => Some(ok(b_.s_greater_than(bool_, None, a, b))?),
            Cmp::GeS => Some(ok(b_.s_greater_than_equal(bool_, None, a, b))?),
            Cmp::False => return Ok(self.boolc(false)),
            Cmp::True => return Ok(self.boolc(true)),
            Cmp::Class => return self.class(a, b),
            _ => None,
        };
        if let Some(result) = integer {
            return Ok(result);
        }
        let (x, y) = (self.float(a)?, self.float(b)?);
        let b_ = &mut self.b;
        Ok(match op {
            Cmp::FLt => ok(b_.f_ord_less_than(bool_, None, x, y))?,
            Cmp::FEq => ok(b_.f_ord_equal(bool_, None, x, y))?,
            Cmp::FLe => ok(b_.f_ord_less_than_equal(bool_, None, x, y))?,
            Cmp::FGt => ok(b_.f_ord_greater_than(bool_, None, x, y))?,
            Cmp::FLg => ok(b_.f_ord_not_equal(bool_, None, x, y))?,
            Cmp::FGe => ok(b_.f_ord_greater_than_equal(bool_, None, x, y))?,
            Cmp::FNge => ok(b_.f_unord_less_than(bool_, None, x, y))?,
            Cmp::FNlg => ok(b_.f_unord_equal(bool_, None, x, y))?,
            Cmp::FNgt => ok(b_.f_unord_less_than_equal(bool_, None, x, y))?,
            Cmp::FNle => ok(b_.f_unord_greater_than(bool_, None, x, y))?,
            Cmp::FNeq => ok(b_.f_unord_not_equal(bool_, None, x, y))?,
            Cmp::FNlt => ok(b_.f_unord_greater_than_equal(bool_, None, x, y))?,
            Cmp::FO | Cmp::FU => {
                let nan_x = ok(b_.is_nan(bool_, None, x))?;
                let nan_y = ok(b_.is_nan(bool_, None, y))?;
                let either = ok(b_.logical_or(bool_, None, nan_x, nan_y))?;
                if op == Cmp::FU {
                    either
                } else {
                    ok(b_.logical_not(bool_, None, either))?
                }
            }
            _ => unreachable!("integer comparisons handled above"),
        })
    }
}
