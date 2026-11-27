//! The stored form of one translated program: a schema-versioned `postcard` record.

use std::io::Read;

use serde::{Deserialize, Serialize};
use shader_container::Stage;
use shader_translate::images::Number;
use shader_translate::{
    DepthOutput, EsOptions, FloatMode, GsInput, GsOptions, GsOutput, PixelOptions, PixelOutput,
    VertexInput, VertexOptions,
};

use crate::error::StoreError;
use crate::translate::Built;

/// The layout of `Record`; bump it when a field changes so older records are retranslated.
pub const SCHEMA_VERSION: u16 = 1;

/// Identifies the translator sources a record was made with.
pub fn translator_version() -> u64 {
    u64::from_str_radix(env!("LEONIDA_TRANSLATOR_HASH"), 16).expect("build.rs writes a hex number")
}

/// Which program of a container a blob is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The container's last blob: the stage's own program.
    Main,
    /// A vertex container's first blob when it holds two: the vertex half of a merged vertex-and-geometry shader.
    FirstHalf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exactness {
    Exact,
    /// Lanes 32-63 of a 64-lane program matter and a 32-wide GPU has none; one text per reason.
    Approximate(Vec<String>),
}

/// How the pipeline for a program went on one GPU and driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineOutcome {
    pub device: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The container's DXIL program kind.
    pub stage: Option<u16>,
    /// `None` when the header could not be read.
    pub wave64: Option<bool>,
    pub role: Role,
    /// Digest of what else the translation read: the stage, role and the container's signatures.
    pub context: [u8; 16],
    pub translator: u64,
    pub exactness: Exactness,
    pub options: Option<Options>,
    /// SPIR-V words as little-endian bytes, zstd-compressed; absent on failure.
    pub spirv: Option<Vec<u8>>,
    pub words: u32,
    pub error: Option<String>,
    pub pipeline: Option<PipelineOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Float {
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Num {
    Float,
    Uint,
    Sint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub register: u8,
    pub number: Num,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub target: u8,
    pub number: Num,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Depth {
    Any,
    GreaterEqual,
    LessEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    Points,
    Lines,
    Triangles,
    LinesAdjacency,
    TrianglesAdjacency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Topology {
    Points,
    LineStrip,
    TriangleStrip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pixel {
    pub user_sgprs: u32,
    pub inputs: u32,
    pub outputs: Vec<Output>,
    pub depth: Option<Depth>,
    pub attribute_locations: Vec<Option<u8>>,
    pub float: Float,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vertex {
    pub user_sgprs: u32,
    pub inputs: Vec<Input>,
    pub float: Float,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstHalf {
    pub user_sgprs: u32,
    pub inputs: Vec<Input>,
    pub item_bytes: u32,
    pub float: Float,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Geometry {
    pub user_sgprs: u32,
    pub input: Primitive,
    pub item_bytes: u32,
    pub output: Topology,
    pub max_vertices: u32,
    pub vertex_bytes: u32,
    pub position: Option<u8>,
    pub float: Float,
}

/// What a program's pipeline needs besides its SPIR-V, mirroring `Built`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Options {
    Compute,
    Mesh,
    Pixel(Pixel),
    Vertex(Vertex),
    Geometry(Geometry),
    FirstHalf(FirstHalf),
}

impl From<FloatMode> for Float {
    fn from(mode: FloatMode) -> Float {
        Float {
            nan_preserve: mode.nan_preserve,
            flush_denormals: mode.flush_denormals,
            preserve_denormals: mode.preserve_denormals,
        }
    }
}

impl From<Float> for FloatMode {
    fn from(mode: Float) -> FloatMode {
        FloatMode {
            nan_preserve: mode.nan_preserve,
            flush_denormals: mode.flush_denormals,
            preserve_denormals: mode.preserve_denormals,
        }
    }
}

impl From<Number> for Num {
    fn from(number: Number) -> Num {
        match number {
            Number::Float => Num::Float,
            Number::Uint => Num::Uint,
            Number::Sint => Num::Sint,
        }
    }
}

impl From<Num> for Number {
    fn from(number: Num) -> Number {
        match number {
            Num::Float => Number::Float,
            Num::Uint => Number::Uint,
            Num::Sint => Number::Sint,
        }
    }
}

fn inputs_to(inputs: &[VertexInput]) -> Vec<Input> {
    inputs
        .iter()
        .map(|input| Input {
            register: input.register,
            number: input.number.into(),
        })
        .collect()
}

fn inputs_from(inputs: &[Input]) -> Vec<VertexInput> {
    inputs
        .iter()
        .map(|input| VertexInput {
            register: input.register,
            number: input.number.into(),
        })
        .collect()
}

impl Options {
    fn of(built: &Built) -> Options {
        match built {
            Built::Compute(_) => Options::Compute,
            Built::Mesh(_) => Options::Mesh,
            Built::Pixel(_, o) => Options::Pixel(Pixel {
                user_sgprs: o.user_sgprs,
                inputs: o.inputs,
                outputs: o
                    .outputs
                    .iter()
                    .map(|out| Output {
                        target: out.target,
                        number: out.number.into(),
                    })
                    .collect(),
                depth: o.depth.map(|depth| match depth {
                    DepthOutput::Any => Depth::Any,
                    DepthOutput::GreaterEqual => Depth::GreaterEqual,
                    DepthOutput::LessEqual => Depth::LessEqual,
                }),
                attribute_locations: o.attribute_locations.clone(),
                float: o.float.into(),
            }),
            Built::Vertex(_, o) => Options::Vertex(Vertex {
                user_sgprs: o.user_sgprs,
                inputs: inputs_to(&o.inputs),
                float: o.float.into(),
            }),
            Built::FirstHalf(_, o) => Options::FirstHalf(FirstHalf {
                user_sgprs: o.user_sgprs,
                inputs: inputs_to(&o.inputs),
                item_bytes: o.item_bytes,
                float: o.float.into(),
            }),
            Built::Geometry(_, o) => Options::Geometry(Geometry {
                user_sgprs: o.user_sgprs,
                input: match o.input {
                    GsInput::Points => Primitive::Points,
                    GsInput::Lines => Primitive::Lines,
                    GsInput::Triangles => Primitive::Triangles,
                    GsInput::LinesAdjacency => Primitive::LinesAdjacency,
                    GsInput::TrianglesAdjacency => Primitive::TrianglesAdjacency,
                },
                item_bytes: o.item_bytes,
                output: match o.output {
                    GsOutput::Points => Topology::Points,
                    GsOutput::LineStrip => Topology::LineStrip,
                    GsOutput::TriangleStrip => Topology::TriangleStrip,
                },
                max_vertices: o.max_vertices,
                vertex_bytes: o.vertex_bytes,
                position: o.position,
                float: o.float.into(),
            }),
        }
    }

    fn into_built(self, spirv: Vec<u32>) -> Built {
        match self {
            Options::Compute => Built::Compute(spirv),
            Options::Mesh => Built::Mesh(spirv),
            Options::Pixel(o) => Built::Pixel(
                spirv,
                PixelOptions {
                    user_sgprs: o.user_sgprs,
                    inputs: o.inputs,
                    outputs: o
                        .outputs
                        .iter()
                        .map(|out| PixelOutput {
                            target: out.target,
                            number: out.number.into(),
                        })
                        .collect(),
                    depth: o.depth.map(|depth| match depth {
                        Depth::Any => DepthOutput::Any,
                        Depth::GreaterEqual => DepthOutput::GreaterEqual,
                        Depth::LessEqual => DepthOutput::LessEqual,
                    }),
                    attribute_locations: o.attribute_locations,
                    float: o.float.into(),
                },
            ),
            Options::Vertex(o) => Built::Vertex(
                spirv,
                VertexOptions {
                    user_sgprs: o.user_sgprs,
                    inputs: inputs_from(&o.inputs),
                    float: o.float.into(),
                },
            ),
            Options::FirstHalf(o) => Built::FirstHalf(
                spirv,
                EsOptions {
                    user_sgprs: o.user_sgprs,
                    inputs: inputs_from(&o.inputs),
                    item_bytes: o.item_bytes,
                    float: o.float.into(),
                },
            ),
            Options::Geometry(o) => Built::Geometry(
                spirv,
                GsOptions {
                    user_sgprs: o.user_sgprs,
                    input: match o.input {
                        Primitive::Points => GsInput::Points,
                        Primitive::Lines => GsInput::Lines,
                        Primitive::Triangles => GsInput::Triangles,
                        Primitive::LinesAdjacency => GsInput::LinesAdjacency,
                        Primitive::TrianglesAdjacency => GsInput::TrianglesAdjacency,
                    },
                    item_bytes: o.item_bytes,
                    output: match o.output {
                        Topology::Points => GsOutput::Points,
                        Topology::LineStrip => GsOutput::LineStrip,
                        Topology::TriangleStrip => GsOutput::TriangleStrip,
                    },
                    max_vertices: o.max_vertices,
                    vertex_bytes: o.vertex_bytes,
                    position: o.position,
                    float: o.float.into(),
                },
            ),
        }
    }
}

/// The DXIL program kind of a stage, the inverse of `Stage::from_dxil_kind`.
pub fn stage_kind(stage: Stage) -> u16 {
    match stage {
        Stage::Pixel => 0,
        Stage::Vertex => 1,
        Stage::Geometry => 2,
        Stage::Hull => 3,
        Stage::Domain => 4,
        Stage::Compute => 5,
        Stage::Library => 6,
        Stage::Mesh => 13,
        Stage::Amplification => 14,
        Stage::Other(kind) => kind,
    }
}

impl Record {
    /// A record with no translation: the program failed with `error`.
    pub fn failed(stage: Option<Stage>, role: Role, context: [u8; 16], error: String) -> Record {
        Record {
            stage: stage.map(stage_kind),
            wave64: None,
            role,
            context,
            translator: translator_version(),
            exactness: Exactness::Exact,
            options: None,
            spirv: None,
            words: 0,
            error: Some(error),
            pipeline: None,
        }
    }

    pub fn stage(&self) -> Option<Stage> {
        self.stage.map(Stage::from_dxil_kind)
    }

    /// Fills in the translation of a program.
    pub fn with_built(mut self, built: &Built) -> Record {
        self.options = Some(Options::of(built));
        let bytes: Vec<u8> = built.spirv().iter().flat_map(|w| w.to_le_bytes()).collect();
        self.spirv = Some(ruzstd::encoding::compress_to_vec(
            &bytes[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
        self.words = built.spirv().len() as u32;
        self
    }

    /// The program as the pipeline builder wants it, or the translation error.
    pub fn built(&self) -> Result<Built, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let (Some(options), Some(packed)) = (&self.options, &self.spirv) else {
            return Err("the stored program holds no SPIR-V".into());
        };
        let mut bytes = Vec::with_capacity(self.words as usize * 4);
        ruzstd::decoding::StreamingDecoder::new(&packed[..])
            .map_err(|e| format!("stored SPIR-V: {e}"))?
            .read_to_end(&mut bytes)
            .map_err(|e| format!("stored SPIR-V: {e}"))?;
        if bytes.len() != self.words as usize * 4 {
            return Err("stored SPIR-V has the wrong length".into());
        }
        let words = bytes
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
            .collect();
        Ok(options.clone().into_built(words))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = SCHEMA_VERSION.to_le_bytes().to_vec();
        bytes.extend(postcard::to_allocvec(self).expect("a record serializes"));
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Record, StoreError> {
        let Some((version, body)) = bytes.split_first_chunk::<2>() else {
            return Err(StoreError::Corrupt("shorter than its version".into()));
        };
        let found = u16::from_le_bytes(*version);
        if found != SCHEMA_VERSION {
            return Err(StoreError::Schema {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        postcard::from_bytes(body).map_err(|e| StoreError::Corrupt(e.to_string()))
    }
}
