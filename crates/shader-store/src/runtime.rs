//! The shader source the running game uses: programs come from the store, translated in process on a miss.
//!
//! [`StoreSource`] implements the d3d12 layer's [`ShaderSource`]. A program is found by the hash the store uses
//! (the SHA-256 of its FSSM blob); the stage the pipeline needs is rebuilt from the record, the blob and the
//! container's signatures without lifting the program again. A miss is translated, stored, and used. What the
//! store cannot hold (FSPX compute programs, a container that is not for the stage asked) goes to the fallback
//! source, whose answer is not stored.
//!
//! A pixel shader's SPIR-V depends on the vertex stage in front of it, so a pixel shader linked to a stage that
//! changes its inputs is kept under a key derived from the blob and the wiring.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use sha2::{Digest, Sha256};
use shader_container::{
    CodeRange, Fssm, Interface, Stage, WaveSize, find_part, parts, pixel_attribute_sources,
    register_exports, shader_model,
};
use shader_translate::{ComputeOptions, FloatMode};
use xbox_d3d12::pipeline::{Link, PipelineError, ShaderSource, TranslatedStage};

use crate::program::{
    AttributeLocations, Key, Source, container_programs, key_of, translate_with_facts,
    uses_code_base,
};
use crate::record::Role;
use crate::store::Store;
use crate::translate::Built;

/// How the programs asked for were answered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// Answered from a stage still alive from an earlier ask.
    pub reused: u64,
    /// Read from the store.
    pub from_store: u64,
    /// Translated here and written to the store.
    pub translated: u64,
    /// Handed to the fallback source.
    pub fallback: u64,
    /// Store reads or writes that failed; the program was then translated without the store.
    pub store_errors: u64,
}

#[derive(Default)]
struct Counters {
    reused: AtomicU64,
    from_store: AtomicU64,
    translated: AtomicU64,
    fallback: AtomicU64,
    store_errors: AtomicU64,
}

#[derive(Default)]
struct Alive {
    stages: HashMap<[u8; 32], Weak<TranslatedStage>>,
    /// The size at which dead entries are next swept out.
    sweep_at: usize,
}

pub struct StoreSource {
    store: Arc<Store>,
    fallback: Arc<dyn ShaderSource>,
    alive: Mutex<Alive>,
    counters: Counters,
}

/// A program of a pipeline description, found in its bytecode.
struct Wanted {
    source: Source,
    /// The key of the blob alone.
    base: Key,
    /// The key the translation is stored under.
    variant: Key,
    locations: Option<AttributeLocations>,
}

impl StoreSource {
    pub fn new(store: Arc<Store>, fallback: Arc<dyn ShaderSource>) -> StoreSource {
        StoreSource {
            store,
            fallback,
            alive: Mutex::new(Alive::default()),
            counters: Counters::default(),
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn stats(&self) -> SourceStats {
        let c = &self.counters;
        SourceStats {
            reused: c.reused.load(Ordering::Relaxed),
            from_store: c.from_store.load(Ordering::Relaxed),
            translated: c.translated.load(Ordering::Relaxed),
            fallback: c.fallback.load(Ordering::Relaxed),
            store_errors: c.store_errors.load(Ordering::Relaxed),
        }
    }

    fn count(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// The program in `bytecode` as the store sees it, or `None` when the store has no place for it.
    fn wanted(&self, bytecode: &[u8], stage: Stage, link: &Link) -> Option<Wanted> {
        let role = if link.first_half && stage == Stage::Vertex {
            Role::FirstHalf
        } else {
            Role::Main
        };
        let source = if bytecode.starts_with(b"FSSM") {
            if role == Role::FirstHalf {
                return None;
            }
            Source {
                stage: Some(stage),
                role,
                fssm: bytecode.to_vec(),
                interface: None,
            }
        } else {
            if bytecode.starts_with(b"FSPX") {
                return None;
            }
            let list = parts(bytecode).ok()?;
            if find_part(&list, b"ACCC").is_some_and(|p| p.starts_with(b"FSPX")) {
                return None;
            }
            if let Ok(Some((declared, _))) = shader_model(&list)
                && declared != stage
            {
                return None;
            }
            container_programs(Some(stage), bytecode)
                .ok()?
                .into_iter()
                .find(|s| s.role == role)?
        };
        let base = key_of(&source.fssm);
        let locations = if stage == Stage::Pixel {
            let locations = pixel_locations(&source, link)?;
            (!locations.is_empty()).then_some(locations)
        } else {
            None
        };
        let variant = match &locations {
            None => base,
            Some(locations) => {
                let mut hash = Sha256::new();
                hash.update(b"pixel linked to its vertex stage");
                hash.update(base);
                for location in locations {
                    hash.update(match location {
                        None => [0, 0],
                        Some(n) => [1, *n],
                    });
                }
                hash.finalize().into()
            }
        };
        Some(Wanted {
            source,
            base,
            variant,
            locations,
        })
    }

    /// The translation of `wanted`, from the store or made now, and whether the program reads its own address.
    fn program(&self, wanted: &Wanted) -> Result<(Built, bool), PipelineError> {
        let stored = match self
            .store
            .current_for(&wanted.variant, wanted.source.context())
        {
            Ok(stored) => stored,
            Err(_) => {
                self.count(&self.counters.store_errors);
                None
            }
        };
        let (program, translated_here) = match stored {
            Some(record) => (self.store.resolve(wanted.variant, record), None),
            None => {
                let (record, built, uses) =
                    translate_with_facts(&wanted.source, wanted.locations.clone());
                let facts: Vec<_> = uses.map(|u| (wanted.base, u)).into_iter().collect();
                if self
                    .store
                    .put_translations(&[(wanted.variant, record.clone())], &facts)
                    .is_err()
                {
                    self.count(&self.counters.store_errors);
                }
                (self.store.resolve_with(wanted.variant, record, built), uses)
            }
        };
        let built = program.built.map_err(PipelineError::Translate)?;
        if translated_here.is_some() {
            self.count(&self.counters.translated);
        } else {
            self.count(&self.counters.from_store);
        }
        let uses = match translated_here {
            Some(uses) => uses,
            None => self.known_code_base(wanted),
        };
        Ok((built, uses))
    }

    /// Whether the program reads its own address: the stored fact, or worked out and stored now.
    fn known_code_base(&self, wanted: &Wanted) -> bool {
        if let Ok(Some(known)) = self.store.uses_code_base(&wanted.base) {
            return known;
        }
        match uses_code_base(&wanted.source) {
            Ok(uses) => {
                if self.store.put_code_bases(&[(wanted.base, uses)]).is_err() {
                    self.count(&self.counters.store_errors);
                }
                uses
            }
            Err(_) => false,
        }
    }
}

/// How the pixel shader's attributes are wired to the stage in front of it.
fn pixel_locations(source: &Source, link: &Link) -> Option<AttributeLocations> {
    let fssm = Fssm::parse(&source.fssm).ok()?;
    let setup = fssm.pixel_setup()?;
    Some(match (&link.previous, &source.interface) {
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
    })
}

/// The stage a pipeline needs from a translation, filled in as translating the program directly would.
fn stage_of(
    wanted: &Wanted,
    built: Built,
    uses_code_base: bool,
    stage: Stage,
    link: &Link,
) -> Result<TranslatedStage, PipelineError> {
    let source = &wanted.source;
    let bad = |text: &str| PipelineError::Bytecode(text.to_string());
    let fssm = Fssm::parse(&source.fssm).map_err(|e| PipelineError::Bytecode(e.to_string()))?;
    let CodeRange::Header { start, .. } = fssm.code_range() else {
        return Err(bad("no code range in the header"));
    };
    let interface: Option<&Interface> = source.interface.as_ref();
    let mut out = TranslatedStage {
        stage,
        first_half: link.first_half,
        spirv: Vec::new(),
        wave64: fssm.wave_size() == WaveSize::Wave64,
        user_sgprs: fssm.user_sgprs(),
        threads: None,
        interface: source.interface.clone(),
        exports: Vec::new(),
        interpolants: Vec::new(),
        vertex_inputs: Vec::new(),
        outputs: Vec::new(),
        writes_depth: false,
        mesh: None,
        uses_code_base,
        code: source.fssm.clone(),
        code_start: start as u32,
        hash: wanted.base,
    };
    match built {
        Built::Compute(spirv) => {
            let (rsrc2, group) = fssm
                .compute_setup()
                .ok_or_else(|| bad("no compute setup in the header"))?;
            let options = ComputeOptions::from_rsrc2(rsrc2, group, FloatMode::default());
            out.user_sgprs = options.user_sgprs;
            out.threads = Some(group);
            out.spirv = spirv;
        }
        Built::Vertex(spirv, options) => {
            out.vertex_inputs = options.inputs;
            out.exports = fssm.parameter_exports();
            out.spirv = spirv;
        }
        Built::FirstHalf(spirv, options) => {
            out.vertex_inputs = options.inputs;
            out.spirv = spirv;
        }
        Built::Pixel(spirv, options) => {
            let setup = fssm
                .pixel_setup()
                .ok_or_else(|| bad("no pixel setup in the header"))?;
            out.user_sgprs = setup.user_sgprs;
            out.outputs = options.outputs;
            out.writes_depth = options.depth.is_some();
            out.interpolants = setup.interpolants;
            out.spirv = spirv;
        }
        Built::Geometry(spirv, _) => {
            let found = interface.ok_or_else(|| bad("no geometry signatures"))?;
            out.exports = register_exports(&found.outputs);
            out.spirv = spirv;
        }
        Built::Mesh(spirv) => {
            let pipeline = interface
                .and_then(|f| f.pipeline.as_ref())
                .ok_or_else(|| bad("no mesh pipeline state"))?;
            let (Some(threads), Some(mesh)) = (pipeline.threads, pipeline.mesh) else {
                return Err(bad("no mesh pipeline state"));
            };
            out.exports = fssm.parameter_exports();
            out.threads = Some(threads);
            out.mesh = Some(mesh);
            out.spirv = spirv;
        }
    }
    Ok(out)
}

impl ShaderSource for StoreSource {
    fn translate(
        &self,
        bytecode: &[u8],
        stage: Stage,
        link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        let Some(wanted) = self.wanted(bytecode, stage, link) else {
            self.count(&self.counters.fallback);
            return self.fallback.translate(bytecode, stage, link);
        };
        let mut memo = Sha256::new();
        memo.update(wanted.variant);
        memo.update(wanted.source.context());
        memo.update(stage.short_name().as_bytes());
        memo.update([u8::from(link.first_half)]);
        let memo: [u8; 32] = memo.finalize().into();
        if let Some(live) = self
            .alive
            .lock()
            .expect("live stages")
            .stages
            .get(&memo)
            .and_then(Weak::upgrade)
        {
            self.count(&self.counters.reused);
            return Ok(live);
        }
        let (built, uses_code_base) = self.program(&wanted)?;
        let made = Arc::new(stage_of(&wanted, built, uses_code_base, stage, link)?);
        let mut alive = self.alive.lock().expect("live stages");
        if alive.stages.len() >= alive.sweep_at.max(4096) {
            alive.stages.retain(|_, stage| stage.strong_count() > 0);
            alive.sweep_at = 2 * alive.stages.len();
        }
        alive.stages.insert(memo, Arc::downgrade(&made));
        Ok(made)
    }
}
