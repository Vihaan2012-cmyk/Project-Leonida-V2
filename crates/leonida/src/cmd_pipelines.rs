use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gpu::{CacheLoad, Gpu};
use shader_container::CarvedShader;
use shader_store::{
    Key, PipelineOutcome, Record, Store, container_programs, key_hex, translate_with_facts,
};

use crate::cli::PipelinesArgs;
use crate::compute_env::ComputeEnv;
use crate::pipeline_work::{Outcomes, Partners, Selection, build_pipeline, for_each_container};

const BATCH: usize = 256;
/// Compute programs take seconds to minutes each, so their results are kept more often.
const COMPUTE_BATCH: usize = 8;
const SAVE_EVERY: Duration = Duration::from_secs(300);
const DATABASE: &str = "leonida.db";
/// The cache the game's device opens, so what is built here is found when the game runs.
const CACHE: &str = "game-pipelines.cache";

/// Which programs a pass builds: compute ones through the game's own device environment, the rest through the
/// checking device.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Compute,
    Graphics,
}

#[derive(Default)]
struct Counts {
    containers: usize,
    programs: usize,
    /// Programs repeated in other containers of this run.
    repeated: usize,
    /// Programs stored by an earlier run with this translator and built on this GPU.
    skipped: usize,
    translated: usize,
    pipelines: usize,
    /// Pipelines identical to one built earlier in this run.
    shared: usize,
    translation_failures: BTreeMap<String, (usize, String)>,
    pipeline_failures: BTreeMap<String, (usize, String)>,
}

fn note(failures: &mut BTreeMap<String, (usize, String)>, error: String, name: &str) {
    println!("failed: {name}: {error}");
    let entry = failures.entry(error).or_default();
    entry.0 += 1;
    if entry.1.is_empty() {
        entry.1 = name.to_string();
    }
}

struct Run<'a> {
    pass: Pass,
    store: &'a Store,
    gpu: Option<&'a Gpu>,
    compute: Option<&'a ComputeEnv>,
    last_save: Mutex<Instant>,
    device: u64,
    cache_path: &'a Path,
    each: bool,
    outcomes: Outcomes,
    seen: Mutex<HashSet<Key>>,
    batch: Mutex<Vec<(Key, Record)>>,
    code_bases: Mutex<Vec<(Key, bool)>>,
    saving: Mutex<()>,
    counts: Mutex<Counts>,
    errors: Mutex<Vec<String>>,
}

impl Run<'_> {
    fn fail(&self, error: impl std::fmt::Display) {
        println!("error: {error}");
        self.errors
            .lock()
            .expect("no worker panicked")
            .push(error.to_string());
    }

    /// Writes the finished programs in one transaction, then the pipeline cache they were built into.
    fn flush(&self) {
        let batch = std::mem::take(&mut *self.batch.lock().expect("no worker panicked"));
        if !batch.is_empty()
            && let Err(error) = self.store.put_batch(&batch)
        {
            return self.fail(error);
        }
        let facts = std::mem::take(&mut *self.code_bases.lock().expect("no worker panicked"));
        if !facts.is_empty()
            && let Err(error) = self.store.put_code_bases(&facts)
        {
            return self.fail(error);
        }
        self.save_cache(false);
    }

    /// Writes the pipeline cache: always when `force`, otherwise once `SAVE_EVERY` has passed since the last time.
    fn save_cache(&self, force: bool) {
        let mut last = self.last_save.lock().expect("no worker panicked");
        if !force && last.elapsed() < SAVE_EVERY && self.pass == Pass::Compute {
            return;
        }
        *last = Instant::now();
        let _saving = self.saving.lock().expect("no worker panicked");
        match (self.gpu, self.compute) {
            (Some(gpu), _) => {
                if let Err(error) = gpu.save_pipeline_cache(self.cache_path) {
                    self.fail(error);
                }
            }
            (None, Some(compute)) => {
                if let Err(error) = compute.save() {
                    self.fail(error);
                }
            }
            (None, None) => {}
        }
    }

    fn push(&self, key: Key, record: Record) {
        let full = {
            let mut batch = self.batch.lock().expect("no worker panicked");
            batch.push((key, record));
            batch.len()
                >= match self.pass {
                    Pass::Compute => COMPUTE_BATCH,
                    Pass::Graphics => BATCH,
                }
        };
        if full {
            self.flush();
        }
    }

    fn container(&self, partners: &mut Partners, shader: &CarvedShader) {
        let compute = shader.stage.is_some_and(|stage| stage.short_name() == "cs");
        if compute != (self.pass == Pass::Compute) {
            return;
        }
        self.counts.lock().expect("no worker panicked").containers += 1;
        let sources = match container_programs(shader.stage, &shader.dxbc) {
            Ok(sources) => sources,
            Err(error) => {
                let mut counts = self.counts.lock().expect("no worker panicked");
                return note(&mut counts.translation_failures, error, &shader.name);
            }
        };
        for source in sources {
            let key = source.key();
            if !self.seen.lock().expect("no worker panicked").insert(key) {
                self.counts.lock().expect("no worker panicked").repeated += 1;
                continue;
            }
            self.counts.lock().expect("no worker panicked").programs += 1;
            let stored = match self.store.current(&source) {
                Ok(stored) => stored,
                Err(error) => return self.fail(format!("{}: {error}", shader.name)),
            };
            let overridden = self.store.override_path(&key).is_some();
            if let Some(record) = &stored
                && !overridden
                && (record.error.is_some()
                    || record
                        .pipeline
                        .as_ref()
                        .is_some_and(|p| p.device == self.device))
            {
                self.counts.lock().expect("no worker panicked").skipped += 1;
                continue;
            }
            let (mut record, built) = match stored {
                Some(record) => {
                    let built = record.built();
                    (record, built)
                }
                None => {
                    self.counts.lock().expect("no worker panicked").translated += 1;
                    let (record, built, uses) = translate_with_facts(&source, None);
                    if let Some(uses) = uses {
                        self.code_bases
                            .lock()
                            .expect("no worker panicked")
                            .push((key, uses));
                    }
                    (record, built)
                }
            };
            let program = self.store.resolve_with(key, record.clone(), built);
            if let Some(error) = &program.override_error {
                println!("override not used: {error}");
            }
            let built = match program.built {
                Ok(built) => built,
                Err(error) => {
                    let mut counts = self.counts.lock().expect("no worker panicked");
                    note(
                        &mut counts.translation_failures,
                        format!("translation: {error}"),
                        &shader.name,
                    );
                    drop(counts);
                    self.push(key, record);
                    continue;
                }
            };
            let started = Instant::now();
            let (outcome, shared) = match (self.compute, self.gpu) {
                (Some(env), _) => build_compute(env, shader, &built, &self.outcomes),
                (None, Some(gpu)) => build_pipeline(gpu, partners, &built, &self.outcomes),
                (None, None) => unreachable!("a pass has a device"),
            };
            if self.each {
                println!(
                    "{}: {} {} KiB, {:.2} s{} ({})",
                    shader.name,
                    built.kind(),
                    built.spirv().len() / 256,
                    started.elapsed().as_secs_f64(),
                    if shared { ", shared" } else { "" },
                    &key_hex(&key)[..12]
                );
            }
            {
                let mut counts = self.counts.lock().expect("no worker panicked");
                counts.pipelines += 1;
                counts.shared += usize::from(shared);
                if let Err(error) = &outcome {
                    note(&mut counts.pipeline_failures, error.clone(), &shader.name);
                }
            }
            record.pipeline = Some(PipelineOutcome {
                device: self.device,
                error: outcome.err(),
            });
            self.push(key, record);
        }
    }
}

/// Builds a compute container's pipeline in the game's environment unless an identical program was built before.
fn build_compute(
    env: &ComputeEnv,
    shader: &CarvedShader,
    built: &shader_store::Built,
    outcomes: &Outcomes,
) -> (Result<(), String>, bool) {
    let fingerprint = built.fingerprint();
    let earlier = outcomes
        .lock()
        .expect("no worker panicked")
        .get(&fingerprint)
        .cloned();
    if let Some(outcome) = earlier {
        return (outcome, true);
    }
    let outcome = env
        .build(&shader.dxbc)
        .map_err(|error| format!("compute pipeline: {error}"));
    outcomes
        .lock()
        .expect("no worker panicked")
        .insert(fingerprint, outcome.clone());
    (outcome, false)
}

fn print_failures(title: &str, failures: &BTreeMap<String, (usize, String)>) {
    let total: usize = failures.values().map(|(count, _)| count).sum();
    println!("{title}: {total}");
    for (error, (count, example)) in failures {
        println!("  {count:>6}  {error}   e.g. {example}");
    }
}

/// One pass over the archive: builds the programs of `pass` and reports them. True when nothing failed.
fn run_pass(
    pass: Pass,
    args: &PipelinesArgs,
    store: &Store,
    gpu: Option<&Gpu>,
    compute: Option<&ComputeEnv>,
    device: u64,
    cache_path: &Path,
) -> Result<bool> {
    let run = Run {
        pass,
        store,
        gpu,
        compute,
        last_save: Mutex::new(Instant::now()),
        device,
        cache_path,
        each: args.each,
        outcomes: Arc::new(Mutex::new(HashMap::new())),
        seen: Mutex::default(),
        batch: Mutex::default(),
        code_bases: Mutex::default(),
        saving: Mutex::default(),
        counts: Mutex::default(),
        errors: Mutex::default(),
    };
    let started = Instant::now();
    let selection = Selection {
        game: args.game.as_ref(),
        limit: args.limit,
        stage: match pass {
            Pass::Compute => Some("cs"),
            Pass::Graphics => args.stage.as_deref(),
        },
        name: args.name.as_deref(),
    };
    let worked = for_each_container(&selection, args.jobs, Partners::new, |partners, shader| {
        run.container(partners, shader)
    });
    run.flush();
    run.save_cache(true);
    worked?;

    let counts = run.counts.lock().expect("no worker panicked");
    println!(
        "{} pass",
        match pass {
            Pass::Compute => "compute",
            Pass::Graphics => "graphics",
        }
    );
    println!("containers: {}", counts.containers);
    println!(
        "programs: {} ({} repeated in other containers)",
        counts.programs, counts.repeated
    );
    println!("skipped, already stored and built: {}", counts.skipped);
    println!("translated: {}", counts.translated);
    println!(
        "pipelines built: {} ({} identical to an earlier one)",
        counts.pipelines, counts.shared
    );
    print_failures("translation failures", &counts.translation_failures);
    print_failures("pipeline failures", &counts.pipeline_failures);
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    let errors = run.errors.lock().expect("no worker panicked");
    Ok(errors.is_empty()
        && counts.translation_failures.is_empty()
        && counts.pipeline_failures.is_empty())
}

fn report_cache(load: CacheLoad) {
    match load {
        CacheLoad::Missing => println!("pipeline cache: none yet"),
        CacheLoad::Loaded(bytes) => println!("pipeline cache: {} KiB loaded", bytes / 1024),
        CacheLoad::Rejected(reason) => println!("pipeline cache: not used, {reason}"),
    }
}

pub fn pipelines_command(args: &PipelinesArgs) -> Result<ExitCode> {
    let cache_path = args.output.join(CACHE);
    let db_path = args.output.join(DATABASE);
    let store = Store::open(&db_path)
        .with_context(|| format!("opening {}", db_path.display()))?
        .with_overrides(args.overrides.clone());
    let started = Instant::now();
    let mut clean = true;
    if args.stage.as_deref().is_none_or(|stage| stage == "cs") {
        let compute = ComputeEnv::open(&cache_path)?;
        println!(
            "pipeline cache: {}",
            if compute.loaded() {
                format!("{} KiB loaded", compute.cache_size()? / 1024)
            } else {
                "none yet".to_string()
            }
        );
        clean &= run_pass(
            Pass::Compute,
            args,
            &store,
            None,
            Some(&compute),
            compute.cache_id(),
            &cache_path,
        )?;
    }
    if args.stage.as_deref() == Some("cs") {
        println!("took {:.1} s", started.elapsed().as_secs_f64());
        return Ok(if clean {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    let gpu = Gpu::new().context("opening the GPU")?;
    report_cache(
        gpu.load_pipeline_cache(&cache_path)
            .with_context(|| format!("reading {}", cache_path.display()))?,
    );
    clean &= run_pass(
        Pass::Graphics,
        args,
        &store,
        Some(&gpu),
        None,
        gpu.pipeline_cache_id(),
        &cache_path,
    )?;
    println!(
        "database: {} programs; pipeline cache: {} KiB in {}",
        store.len()?,
        gpu.pipeline_cache_data()?.len() / 1024,
        cache_path.display()
    );
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    Ok(if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
