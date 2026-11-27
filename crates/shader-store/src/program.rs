//! Finding a container's programs and translating one into a record.

use sha2::{Digest, Sha256};
use shader_container::{CodeRange, Fssm, Interface, Stage, WaveSize, interface, parts, unpack_all};
use shader_ir::Function;

use crate::record::{Exactness, Record, Role};
use crate::translate::{Built, build, build_first_half, pixel_options};

/// A program's identity: the SHA-256 of its FSSM blob.
pub type Key = [u8; 32];

pub fn key_of(fssm: &[u8]) -> Key {
    Sha256::digest(fssm).into()
}

pub fn key_hex(key: &Key) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One program to translate: its FSSM blob and what its container says about it.
#[derive(Debug, Clone)]
pub struct Source {
    pub stage: Option<Stage>,
    pub role: Role,
    pub fssm: Vec<u8>,
    pub interface: Option<Interface>,
}

impl Source {
    pub fn key(&self) -> Key {
        key_of(&self.fssm)
    }

    /// Digest of the stage, role and signatures, which the translation reads besides the blob.
    pub fn context(&self) -> [u8; 16] {
        let text = format!("{:?}|{:?}|{:?}", self.stage, self.role, self.interface);
        Sha256::digest(text.as_bytes())[..16]
            .try_into()
            .expect("16 bytes")
    }
}

/// The programs of one shader container (its DXBC bytes): a vertex container with two blobs also holds the vertex
/// half of a merged vertex-and-geometry shader.
pub fn container_programs(stage: Option<Stage>, dxbc: &[u8]) -> Result<Vec<Source>, String> {
    let found = parts(dxbc)
        .ok()
        .and_then(|list| interface(&list, None).ok());
    let blobs = unpack_all(dxbc).map_err(|_| "unreadable container".to_string())?;
    let mut sources = Vec::new();
    if stage == Some(Stage::Vertex) && blobs.len() == 2 {
        sources.push(Source {
            stage,
            role: Role::FirstHalf,
            fssm: blobs[0].clone(),
            interface: found.clone(),
        });
    }
    if let Some(blob) = blobs.into_iter().last() {
        sources.push(Source {
            stage,
            role: Role::Main,
            fssm: blob,
            interface: found,
        });
    }
    Ok(sources)
}

/// A program translated, before it is packed into a record.
#[derive(Debug, Clone)]
pub struct Translated {
    pub built: Built,
    pub wave64: bool,
    pub exactness: Exactness,
    /// The program reads constants through its own address (`s_getpc_b64`), so its pipeline's code base must be
    /// its blob's.
    pub uses_code_base: bool,
}

/// Why a program could not be translated; `wave64` is known once the header has been read.
#[derive(Debug, Clone)]
pub struct Failure {
    pub error: String,
    pub wave64: Option<bool>,
}

/// Whether the function takes its own address anywhere, found by streaming its debug text through a matcher so
/// a large shader is not rendered into one string.
pub fn reads_own_address(function: &Function) -> bool {
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

/// Which input of a pixel shader each interpolant comes from, as the vertex stage in front of it supplies them.
pub type AttributeLocations = Vec<Option<u8>>;

fn build_linked_pixel(
    fssm: &Fssm,
    found: Option<&Interface>,
    function: &Function,
    locations: AttributeLocations,
) -> Result<Built, String> {
    let mut options = pixel_options(fssm, found).ok_or("no pixel setup in the header")?;
    options.attribute_locations = locations;
    let spirv = shader_translate::pixel(function, &options).map_err(|e| e.to_string())?;
    Ok(Built::Pixel(spirv, options))
}

/// Translates a program without packing it into a record.
pub fn translate_built(source: &Source) -> Result<Translated, Failure> {
    translate_linked(source, None)
}

/// As [`translate_built`], for a pixel shader whose inputs are wired to the vertex stage in front of it by
/// `locations`. Other stages ignore them.
pub fn translate_linked(
    source: &Source,
    locations: Option<AttributeLocations>,
) -> Result<Translated, Failure> {
    let fail = |error: String, wave64: Option<bool>| Failure { error, wave64 };
    let fssm = Fssm::parse(&source.fssm).map_err(|e| fail(e.to_string(), None))?;
    let wave64 = fssm.wave_size() == WaveSize::Wave64;
    let fail = |error: String| fail(error, Some(wave64));
    let CodeRange::Header { start, end } = fssm.code_range() else {
        return Err(fail("no code range in the header".into()));
    };
    let code = source
        .fssm
        .get(start..end)
        .ok_or_else(|| fail("code past the blob".into()))?;
    let function = shader_ir::lift(code, wave64).map_err(|e| fail(e.to_string()))?;
    let interface = source.interface.as_ref();
    let built = match (source.role, source.stage, locations) {
        (Role::Main, Some(Stage::Pixel), Some(locations)) => {
            build_linked_pixel(&fssm, interface, &function, locations)
        }
        (Role::Main, ..) => build(source.stage, &fssm, interface, &function),
        (Role::FirstHalf, ..) => build_first_half(&fssm, interface, &function),
    }
    .map_err(fail)?;
    let mut exactness = Exactness::Exact;
    if wave64
        && matches!(
            source.stage,
            Some(Stage::Pixel | Stage::Vertex | Stage::Geometry)
        )
    {
        let hazards = shader_ir::upper_lane_hazards(code);
        if !hazards.is_empty() {
            exactness =
                Exactness::Approximate(hazards.iter().map(|h| h.describe().to_string()).collect());
        }
    }
    Ok(Translated {
        built,
        wave64,
        exactness,
        uses_code_base: reads_own_address(&function),
    })
}

/// The record for a translation result, holding the error text when the program could not be translated.
pub fn record_of(source: &Source, result: &Result<Translated, Failure>) -> Record {
    let context = source.context();
    match result {
        Ok(translated) => {
            let mut record = Record::failed(source.stage, source.role, context, String::new())
                .with_built(&translated.built);
            record.error = None;
            record.wave64 = Some(translated.wave64);
            record.exactness = translated.exactness.clone();
            record
        }
        Err(failure) => {
            let mut record =
                Record::failed(source.stage, source.role, context, failure.error.clone());
            record.wave64 = failure.wave64;
            record
        }
    }
}

/// A source translated, as the record to store and the program the pipeline builder takes.
pub fn translate_program(source: &Source) -> (Record, Result<Built, String>) {
    let (record, built, _) = translate_with_facts(source, None);
    (record, built)
}

/// As [`translate_program`], for a pixel shader linked by `locations`, and with whether the program reads its own
/// address (`None` when it did not translate).
pub fn translate_with_facts(
    source: &Source,
    locations: Option<AttributeLocations>,
) -> (Record, Result<Built, String>, Option<bool>) {
    let result = translate_linked(source, locations);
    let record = record_of(source, &result);
    let uses_code_base = result.as_ref().ok().map(|t| t.uses_code_base);
    (
        record,
        result.map(|t| t.built).map_err(|f| f.error),
        uses_code_base,
    )
}

/// Whether a program reads its own address, by lifting it. For programs stored before the answer was kept.
pub fn uses_code_base(source: &Source) -> Result<bool, String> {
    let fssm = Fssm::parse(&source.fssm).map_err(|e| e.to_string())?;
    let wave64 = fssm.wave_size() == WaveSize::Wave64;
    let CodeRange::Header { start, end } = fssm.code_range() else {
        return Err("no code range in the header".into());
    };
    let code = source
        .fssm
        .get(start..end)
        .ok_or("code past the blob")?;
    let function = shader_ir::lift(code, wave64).map_err(|e| e.to_string())?;
    Ok(reads_own_address(&function))
}
