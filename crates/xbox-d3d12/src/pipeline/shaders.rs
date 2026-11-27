//! Console shader bytecode to SPIR-V for one pipeline stage.
//!
//! [`ShaderSource`] is the one seam: the default [`DirectTranslation`] opens the container, lifts the RDNA
//! program and translates it. A shader store with precompiled modules can implement the trait instead and answer
//! from its tables first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use shader_container::{
    CodeRange, Fssm, Interface, Interpolant, MeshInfo, ParameterExport, Stage, WaveSize, find_part,
    interface, parts, pixel_attribute_sources, register_exports, shader_model, unpack_all,
};
use shader_translate::images::Number;
use shader_translate::{
    ComputeOptions, DepthOutput, EsOptions, FloatMode, GsInput, GsOptions, GsOutput, MeshOptions,
    PixelOptions, PixelOutput, VertexInput, VertexOptions,
};

use super::PipelineError;

const FSPX_PROLOGUE: usize = 36;

/// One stage's translation and what its pipeline needs to know about it.
#[derive(Debug, Clone)]
pub struct TranslatedStage {
    pub stage: Stage,
    /// The first half of a merged vertex-and-geometry shader, run as a vertex shader.
    pub first_half: bool,
    pub spirv: Vec<u32>,
    pub wave64: bool,
    pub user_sgprs: u32,
    /// A compute shader's group size or a mesh shader's.
    pub threads: Option<[u32; 3]>,
    pub interface: Option<Interface>,
    /// The parameter exports of a vertex, mesh or geometry shader: which `paramN` each output register leaves as.
    pub exports: Vec<ParameterExport>,
    pub interpolants: Vec<Interpolant>,
    pub vertex_inputs: Vec<VertexInput>,
    pub outputs: Vec<PixelOutput>,
    pub writes_depth: bool,
    pub mesh: Option<MeshInfo>,
    /// The program reads constants through its own address (`s_getpc_b64`), so the pipeline's code base must be
    /// its blob's.
    pub uses_code_base: bool,
    /// The program as the GPU reads it: the whole blob, which must sit in the arena because the code reads its
    /// constants at addresses relative to its own start.
    pub code: Vec<u8>,
    /// The offset in `code` of the first instruction: what `s_getpc_b64` counts from.
    pub code_start: u32,
    /// SHA-256 of `code`.
    pub hash: [u8; 32],
}

/// What a stage is linked against.
#[derive(Debug, Clone, Default)]
pub struct Link {
    /// The stage before a pixel shader, whose outputs its attributes read.
    pub previous: Option<Arc<TranslatedStage>>,
    /// Translate the first half of a vertex shader's merged program (its first blob) instead of its own.
    pub first_half: bool,
}

pub trait ShaderSource: Send + Sync {
    /// Translates `bytecode` as a shader of `stage`: a DXBC container, a bare FSSM blob, or for compute a
    /// container or bare payload carrying an FSPX program.
    fn translate(
        &self,
        bytecode: &[u8],
        stage: Stage,
        link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError>;
}

/// Translates in this process, remembering what it has translated.
pub struct DirectTranslation {
    float: FloatMode,
    done: Mutex<HashMap<[u8; 32], Arc<TranslatedStage>>>,
}

impl DirectTranslation {
    pub fn new(float: FloatMode) -> DirectTranslation {
        DirectTranslation {
            float,
            done: Mutex::new(HashMap::new()),
        }
    }
}

impl ShaderSource for DirectTranslation {
    fn translate(
        &self,
        bytecode: &[u8],
        stage: Stage,
        link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        let mut key = Sha256::new();
        key.update(bytecode);
        key.update([stage.short_name().len() as u8, u8::from(link.first_half)]);
        key.update(stage.short_name().as_bytes());
        if let Some(previous) = &link.previous {
            key.update(previous.hash);
            key.update([u8::from(previous.first_half)]);
        }
        let key: [u8; 32] = key.finalize().into();
        if let Some(found) = self.done.lock().expect("translation table").get(&key) {
            return Ok(found.clone());
        }
        let built = Arc::new(translate_stage(bytecode, stage, link, self.float)?);
        self.done
            .lock()
            .expect("translation table")
            .insert(key, built.clone());
        Ok(built)
    }
}

/// The program inside whatever bytecode a pipeline description carried.
enum Program {
    Fssm {
        blob: Vec<u8>,
        interface: Option<Interface>,
    },
    Fspx {
        code: Vec<u8>,
        state: Vec<u8>,
    },
}

fn open(bytecode: &[u8], stage: Stage, first_half: bool) -> Result<Program, PipelineError> {
    if bytecode.starts_with(b"FSSM") {
        return Ok(Program::Fssm {
            blob: bytecode.to_vec(),
            interface: None,
        });
    }
    if bytecode.starts_with(b"FSPX") {
        return fspx(bytecode);
    }
    let list = parts(bytecode).map_err(|e| PipelineError::Bytecode(e.to_string()))?;
    if let Some(payload) = find_part(&list, b"ACCC").filter(|p| p.starts_with(b"FSPX")) {
        return fspx(payload);
    }
    let found = interface(&list, None).ok();
    if let Ok(Some((declared, _))) = shader_model(&list)
        && declared != stage
    {
        return Err(PipelineError::WrongStage {
            wanted: stage.short_name(),
            found: declared.short_name(),
        });
    }
    let blobs = unpack_all(bytecode).map_err(|e| PipelineError::Bytecode(e.to_string()))?;
    let blob = if first_half {
        if blobs.len() < 2 {
            return Err(PipelineError::Bytecode(
                "a vertex shader without a first half".into(),
            ));
        }
        blobs.into_iter().next()
    } else {
        blobs.into_iter().next_back()
    }
    .ok_or_else(|| PipelineError::Bytecode("no program in the container".into()))?;
    Ok(Program::Fssm {
        blob,
        interface: found,
    })
}

/// An FSPX payload: `FSPX`, version, count, total, then count x { kind, offset, size, 0 }. The last entry of
/// each kind wins, and entries outside the payload are skipped.
fn fspx(payload: &[u8]) -> Result<Program, PipelineError> {
    let word = |at: usize| {
        payload
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    let bad = || PipelineError::Bytecode("a malformed FSPX payload".into());
    let count = word(8).ok_or_else(bad)?;
    let total = word(12).ok_or_else(bad)?.min(payload.len());
    if 16 + 16 * count > total {
        return Err(bad());
    }
    let (mut state, mut code) = (None, None);
    for i in 0..count {
        let at = 16 + 16 * i;
        let (kind, offset, size) = (
            word(at).ok_or_else(bad)?,
            word(at + 4).ok_or_else(bad)?,
            word(at + 8).ok_or_else(bad)?,
        );
        let end = offset.checked_add(size).ok_or_else(bad)?;
        if offset < 16 + 16 * count || end > total {
            continue;
        }
        match kind {
            0 => state = Some(payload[offset..end].to_vec()),
            6 => code = Some(payload[offset..end].to_vec()),
            _ => {}
        }
    }
    match (state, code) {
        (Some(state), Some(code)) if !state.is_empty() && !code.is_empty() => {
            Ok(Program::Fspx { code, state })
        }
        _ => Err(bad()),
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn number(component_type: u32) -> Number {
    match component_type {
        1 => Number::Uint,
        2 => Number::Sint,
        _ => Number::Float,
    }
}

/// Whether the function takes its own address anywhere, found by streaming its debug text through a matcher so
/// a large shader is not rendered into one string.
fn reads_own_address(function: &shader_ir::Function) -> bool {
    use std::fmt::Write;
    const NEEDLE: &[u8] = b"CodeBase";
    struct Find(bool, Vec<u8>);
    impl Write for Find {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if self.0 {
                return Ok(());
            }
            self.1.extend_from_slice(text.as_bytes());
            if self.1.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
                self.0 = true;
            }
            let keep = self.1.len().saturating_sub(NEEDLE.len() - 1);
            self.1.drain(..keep);
            Ok(())
        }
    }
    let mut find = Find(false, Vec::new());
    let _ = write!(find, "{:?}", function.blocks);
    find.0
}

fn lift(code: &[u8], wave64: bool) -> Result<shader_ir::Function, PipelineError> {
    shader_ir::lift(code, wave64).map_err(|e| PipelineError::Translate(e.to_string()))
}

fn emit(result: Result<Vec<u32>, shader_translate::EmitError>) -> Result<Vec<u32>, PipelineError> {
    result.map_err(|e| PipelineError::Translate(e.to_string()))
}

fn translate_stage(
    bytecode: &[u8],
    stage: Stage,
    link: &Link,
    float: FloatMode,
) -> Result<TranslatedStage, PipelineError> {
    match open(bytecode, stage, link.first_half)? {
        Program::Fspx { code, state } => {
            if stage != Stage::Compute {
                return Err(PipelineError::WrongStage {
                    wanted: stage.short_name(),
                    found: "an FSPX compute program".into(),
                });
            }
            translate_fspx(code, &state, float)
        }
        Program::Fssm { blob, interface } => translate_fssm(blob, interface, stage, link, float),
    }
}

/// Kind 0 of an FSPX payload: words 3 and 4 hold the group size and the wave size bit, word 6 the RSRC2.
fn translate_fspx(
    code: Vec<u8>,
    state: &[u8],
    float: FloatMode,
) -> Result<TranslatedStage, PipelineError> {
    let word = |n: usize| {
        state
            .get(4 * n..4 * n + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
            .ok_or_else(|| PipelineError::Bytecode("FSPX dispatch state is too short".into()))
    };
    let (w3, w4, rsrc2) = (word(3)?, word(4)?, word(6)?);
    let threads = [w3 & 0xFFFF, w3 >> 16, w4 & 0xFFFF];
    let wave64 = w4 & (1 << 20) == 0;
    let end = code
        .chunks_exact(4)
        .rposition(|w| w == shader_container::ENDPGM.as_slice())
        .map(|i| 4 * i + 4)
        .ok_or_else(|| PipelineError::Bytecode("an FSPX program without s_endpgm".into()))?;
    if end <= FSPX_PROLOGUE {
        return Err(PipelineError::Bytecode(
            "an FSPX program with no code".into(),
        ));
    }
    let function = lift(&code[FSPX_PROLOGUE..end], wave64)?;
    let options = ComputeOptions::from_rsrc2(rsrc2, threads, float);
    let spirv = emit(shader_translate::compute(&function, &options))?;
    Ok(TranslatedStage {
        stage: Stage::Compute,
        first_half: false,
        spirv,
        wave64,
        user_sgprs: options.user_sgprs,
        threads: Some(threads),
        interface: None,
        exports: Vec::new(),
        interpolants: Vec::new(),
        vertex_inputs: Vec::new(),
        outputs: Vec::new(),
        writes_depth: false,
        mesh: None,
        uses_code_base: reads_own_address(&function),
        hash: digest(&code),
        code,
        code_start: FSPX_PROLOGUE as u32,
    })
}

fn translate_fssm(
    blob: Vec<u8>,
    found: Option<Interface>,
    stage: Stage,
    link: &Link,
    float: FloatMode,
) -> Result<TranslatedStage, PipelineError> {
    let fssm = Fssm::parse(&blob).map_err(|e| PipelineError::Bytecode(e.to_string()))?;
    let CodeRange::Header { start, end } = fssm.code_range() else {
        return Err(PipelineError::Bytecode(
            "no code range in the header".into(),
        ));
    };
    let wave64 = fssm.wave_size() == WaveSize::Wave64;
    let code = blob
        .get(start..end)
        .ok_or_else(|| PipelineError::Bytecode("code past the end of the blob".into()))?;
    let function = lift(code, wave64)?;
    let mut out = TranslatedStage {
        stage,
        first_half: link.first_half,
        spirv: Vec::new(),
        wave64,
        user_sgprs: fssm.user_sgprs(),
        threads: None,
        interface: found.clone(),
        exports: Vec::new(),
        interpolants: Vec::new(),
        vertex_inputs: Vec::new(),
        outputs: Vec::new(),
        writes_depth: false,
        mesh: None,
        uses_code_base: reads_own_address(&function),
        hash: digest(&blob),
        code: Vec::new(),
        code_start: start as u32,
    };
    let inputs = |found: &Option<Interface>| -> Vec<VertexInput> {
        found
            .iter()
            .flat_map(|f| f.inputs.iter())
            .filter(|e| e.system_value == 0 && e.register < 16)
            .map(|e| VertexInput {
                register: e.register as u8,
                number: number(e.component_type),
            })
            .collect()
    };
    let item_bytes = |elements: &[shader_container::SignatureElement]| {
        elements.iter().map(|e| e.register + 1).max().unwrap_or(0) * 16
    };
    match stage {
        Stage::Compute => {
            let (rsrc2, group) = fssm
                .compute_setup()
                .ok_or_else(|| PipelineError::Bytecode("no compute setup in the header".into()))?;
            let options = ComputeOptions::from_rsrc2(rsrc2, group, float);
            out.user_sgprs = options.user_sgprs;
            out.threads = Some(group);
            out.spirv = emit(shader_translate::compute(&function, &options))?;
        }
        Stage::Vertex if link.first_half => {
            out.vertex_inputs = inputs(&found);
            let options = EsOptions {
                user_sgprs: out.user_sgprs,
                inputs: out.vertex_inputs.clone(),
                item_bytes: item_bytes(found.as_ref().map_or(&[], |f| &f.outputs)),
                float,
            };
            out.spirv = emit(shader_translate::es(&function, &options))?;
        }
        Stage::Vertex => {
            out.vertex_inputs = inputs(&found);
            out.exports = fssm.parameter_exports();
            let options = VertexOptions {
                user_sgprs: out.user_sgprs,
                inputs: out.vertex_inputs.clone(),
                float,
            };
            out.spirv = emit(shader_translate::vertex(&function, &options))?;
        }
        Stage::Pixel => {
            let setup = fssm
                .pixel_setup()
                .ok_or_else(|| PipelineError::Bytecode("no pixel setup in the header".into()))?;
            out.user_sgprs = setup.user_sgprs;
            out.outputs = found
                .iter()
                .flat_map(|f| f.outputs.iter())
                .filter(|e| e.system_value == 64 && e.register < 8)
                .map(|e| PixelOutput {
                    target: e.register as u8,
                    number: number(e.component_type),
                })
                .collect();
            let depth = DepthOutput::from_control(setup.depth_control);
            out.writes_depth = depth.is_some();
            let attribute_locations = match (&link.previous, &found) {
                (Some(previous), Some(found)) => previous
                    .interface
                    .as_ref()
                    .map(|p| {
                        pixel_attribute_sources(
                            &p.outputs,
                            &previous.exports,
                            &found.inputs,
                            &setup.interpolants,
                        )
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            out.interpolants = setup.interpolants.clone();
            let options = PixelOptions {
                user_sgprs: setup.user_sgprs,
                inputs: setup.inputs,
                outputs: out.outputs.clone(),
                depth,
                attribute_locations,
                float,
            };
            out.spirv = emit(shader_translate::pixel(&function, &options))?;
        }
        Stage::Geometry => {
            let found = found
                .as_ref()
                .ok_or_else(|| PipelineError::Bytecode("no geometry signatures".into()))?;
            let info = found
                .pipeline
                .as_ref()
                .and_then(|p| p.geometry)
                .ok_or_else(|| PipelineError::Bytecode("no geometry pipeline state".into()))?;
            let input = match info.input_primitive {
                1 => GsInput::Points,
                2 => GsInput::Lines,
                3 => GsInput::Triangles,
                6 => GsInput::LinesAdjacency,
                7 => GsInput::TrianglesAdjacency,
                _ => {
                    return Err(PipelineError::Unsupported(
                        "a geometry shader input primitive",
                    ));
                }
            };
            let output = match info.output_topology {
                1 => GsOutput::Points,
                3 => GsOutput::LineStrip,
                5 => GsOutput::TriangleStrip,
                _ => {
                    return Err(PipelineError::Unsupported(
                        "a geometry shader output topology",
                    ));
                }
            };
            if info.stream_mask > 1 {
                return Err(PipelineError::Unsupported(
                    "geometry output to a second stream",
                ));
            }
            out.exports = register_exports(&found.outputs);
            let options = GsOptions {
                user_sgprs: out.user_sgprs,
                input,
                item_bytes: item_bytes(&found.inputs),
                output,
                max_vertices: u32::from(info.max_vertices),
                vertex_bytes: item_bytes(&found.outputs),
                position: found
                    .outputs
                    .iter()
                    .find(|e| e.system_value == 1)
                    .map(|e| e.register as u8),
                float,
            };
            out.spirv = emit(shader_translate::gs(&function, &options))?;
        }
        Stage::Mesh => {
            let pipeline = found
                .as_ref()
                .and_then(|f| f.pipeline.as_ref())
                .ok_or_else(|| PipelineError::Bytecode("no mesh pipeline state".into()))?;
            let (Some(threads), Some(mesh)) = (pipeline.threads, pipeline.mesh) else {
                return Err(PipelineError::Bytecode("no mesh pipeline state".into()));
            };
            if mesh.topology != 2 {
                return Err(PipelineError::Unsupported("a mesh shader that draws lines"));
            }
            out.exports = fssm.parameter_exports();
            out.threads = Some(threads);
            out.mesh = Some(mesh);
            let options = MeshOptions {
                user_sgprs: out.user_sgprs,
                group_size: threads.iter().product(),
                max_vertices: u32::from(mesh.max_vertices),
                max_primitives: u32::from(mesh.max_primitives),
                lds_bytes: mesh.shared_bytes,
                float,
            };
            out.spirv = emit(shader_translate::mesh(&function, &options))?;
        }
        _ => {
            return Err(PipelineError::Unsupported(
                "a shader stage the game does not use",
            ));
        }
    }
    out.code = blob;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_container::testutil::container;

    fn fssm_compute(code: &[u8]) -> Vec<u8> {
        let mut blob = vec![0u8; 0x228];
        blob[..4].copy_from_slice(b"FSSM");
        blob[0xC0..0xC4].copy_from_slice(&0x200u32.to_le_bytes());
        blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
        for (at, value) in [(0x138, 4u32 << 1), (0x13C, 32), (0x140, 2), (0x144, 1)] {
            blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        blob.extend_from_slice(code);
        blob
    }

    fn translate(bytes: &[u8], stage: Stage) -> Result<Arc<TranslatedStage>, PipelineError> {
        DirectTranslation::new(FloatMode::default()).translate(bytes, stage, &Link::default())
    }

    #[test]
    fn a_compute_container_translates_with_its_header_setup() {
        let code = rdna::assemble("s_mov_b32 s20, 1\ns_endpgm", true).unwrap();
        let bytes = container(5, &[fssm_compute(&code)]);
        let stage = translate(&bytes, Stage::Compute).unwrap();
        assert_eq!(stage.threads, Some([32, 2, 1]));
        assert_eq!(stage.user_sgprs, 4);
        assert_eq!(stage.code_start, 0x228);
        assert!(!stage.wave64);
        assert!(!stage.uses_code_base);
        assert_eq!(stage.spirv[0], 0x0723_0203);
        let again = translate(&bytes, Stage::Compute).unwrap();
        assert_eq!(again.hash, stage.hash);
    }

    #[test]
    fn a_bare_fssm_blob_is_a_program_too() {
        let code = rdna::assemble("s_endpgm", true).unwrap();
        let stage = translate(&fssm_compute(&code), Stage::Compute).unwrap();
        assert_eq!(stage.threads, Some([32, 2, 1]));
    }

    #[test]
    fn the_wrong_stage_or_a_missing_half_is_refused() {
        let code = rdna::assemble("s_endpgm", true).unwrap();
        let bytes = container(0, &[fssm_compute(&code)]);
        assert!(matches!(
            translate(&bytes, Stage::Compute),
            Err(PipelineError::WrongStage { .. })
        ));
        let bytes = container(1, &[fssm_compute(&code)]);
        let link = Link {
            previous: None,
            first_half: true,
        };
        let source = DirectTranslation::new(FloatMode::default());
        assert!(matches!(
            source.translate(&bytes, Stage::Vertex, &link),
            Err(PipelineError::Bytecode(_))
        ));
    }

    fn payload(parts: &[(u32, &[u8])]) -> Vec<u8> {
        let directory = 16 + 16 * parts.len();
        let mut out = Vec::new();
        let total = directory + parts.iter().map(|p| p.1.len()).sum::<usize>();
        for word in [
            u32::from_le_bytes(*b"FSPX"),
            1,
            parts.len() as u32,
            total as u32,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        let mut offset = directory;
        for (kind, bytes) in parts {
            for word in [*kind, offset as u32, bytes.len() as u32, 0] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            offset += bytes.len();
        }
        for (_, bytes) in parts {
            out.extend_from_slice(bytes);
        }
        out
    }

    fn state(x: u32, wave32: bool, rsrc2: u32) -> Vec<u8> {
        let mut state = vec![0u8; 28];
        state[12..16].copy_from_slice(&(x | 1 << 16).to_le_bytes());
        state[16..20].copy_from_slice(&(1 | u32::from(wave32) << 20).to_le_bytes());
        state[24..28].copy_from_slice(&rsrc2.to_le_bytes());
        state
    }

    #[test]
    fn an_fspx_program_skips_its_prologue_and_takes_its_dispatch_state() {
        let mut program = vec![0u8; FSPX_PROLOGUE];
        program.extend(rdna::assemble("s_getpc_b64 s[10:11]\ns_endpgm", true).unwrap());
        program.extend([0u8; 8]);
        let bytes = payload(&[(0, &state(64, true, 8 << 1)), (6, &program)]);
        let stage = translate(&bytes, Stage::Compute).unwrap();
        assert_eq!(stage.threads, Some([64, 1, 1]));
        assert_eq!(stage.user_sgprs, 8);
        assert_eq!(stage.code_start, 36);
        assert!(stage.uses_code_base);
        assert_eq!(stage.code.len(), program.len());
        // The same program inside a container's ACCC part is found the same way.
        let wrapped = shader_container::testutil::dxbc(&[(b"ACCC", &bytes)]);
        assert_eq!(
            translate(&wrapped, Stage::Compute).unwrap().hash,
            stage.hash
        );
        let other = translate(&bytes, Stage::Pixel);
        assert!(matches!(other, Err(PipelineError::WrongStage { .. })));
    }

    #[test]
    fn a_damaged_fspx_payload_is_refused() {
        let state = state(64, true, 0);
        assert!(translate(&payload(&[(0, &state)]), Stage::Compute).is_err());
        assert!(translate(&payload(&[(6, &[0u8; 64])]), Stage::Compute).is_err());
        let mut bytes = payload(&[(0, &state), (6, &[0u8; 64])]);
        bytes[8] = 99;
        assert!(translate(&bytes, Stage::Compute).is_err());
        assert!(translate(&bytes[..10], Stage::Compute).is_err());
    }
}
