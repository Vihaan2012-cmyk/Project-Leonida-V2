use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use gpu::{CacheLoad, Gpu};
use shader_container::CarvedShader;
use shader_store::{
    Key, PipelineOutcome, Record, Store, container_programs, key_hex, translate_program,
};

use crate::cli::PipelinesArgs;
use crate::pipeline_work::{Outcomes, Partners, Selection, build_pipeline, for_each_container};

const BATCH: usize = 256;
const DATABASE: &str = "leonida.db";
const CACHE: &str = "pipelines.cache";

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
    store: &'a Store,
    gpu: &'a Gpu,
    device: u64,
    cache_path: &'a Path,
    each: bool,
    outcomes: Outcomes,
    seen: Mutex<HashSet<Key>>,
    batch: Mutex<Vec<(Key, Record)>>,
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
        let _saving = self.saving.lock().expect("no worker panicked");
        if let Err(error) = self.gpu.save_pipeline_cache(self.cache_path) {
            self.fail(error);
        }
    }

    fn push(&self, key: Key, record: Record) {
        let full = {
            let mut batch = self.batch.lock().expect("no worker panicked");
            batch.push((key, record));
            batch.len() >= BATCH
        };
        if full {
            self.flush();
        }
    }

    fn container(&self, partners: &mut Partners, shader: &CarvedShader) {
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
                    translate_program(&source)
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
            let (outcome, shared) = build_pipeline(self.gpu, partners, &built, &self.outcomes);
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

fn print_failures(title: &str, failures: &BTreeMap<String, (usize, String)>) {
    let total: usize = failures.values().map(|(count, _)| count).sum();
    println!("{title}: {total}");
    for (error, (count, example)) in failures {
        println!("  {count:>6}  {error}   e.g. {example}");
    }
}

pub fn pipelines_command(args: &PipelinesArgs) -> Result<ExitCode> {
    let gpu = Gpu::new().context("opening the GPU")?;
    let cache_path = args.output.join(CACHE);
    match gpu
        .load_pipeline_cache(&cache_path)
        .with_context(|| format!("reading {}", cache_path.display()))?
    {
        CacheLoad::Missing => println!("pipeline cache: none yet"),
        CacheLoad::Loaded(bytes) => println!("pipeline cache: {} KiB loaded", bytes / 1024),
        CacheLoad::Rejected(reason) => println!("pipeline cache: not used, {reason}"),
    }
    let db_path = args.output.join(DATABASE);
    let store = Store::open(&db_path)
        .with_context(|| format!("opening {}", db_path.display()))?
        .with_overrides(args.overrides.clone());
    let run = Run {
        store: &store,
        gpu: &gpu,
        device: gpu.pipeline_cache_id(),
        cache_path: &cache_path,
        each: args.each,
        outcomes: Arc::new(Mutex::new(HashMap::new())),
        seen: Mutex::default(),
        batch: Mutex::default(),
        saving: Mutex::default(),
        counts: Mutex::default(),
        errors: Mutex::default(),
    };
    let started = Instant::now();
    let selection = Selection {
        game: args.game.as_ref(),
        limit: args.limit,
        ..Selection::default()
    };
    let worked = for_each_container(&selection, args.jobs, Partners::new, |partners, shader| {
        run.container(partners, shader)
    });
    run.flush();
    worked?;

    let counts = run.counts.lock().expect("no worker panicked");
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
    println!(
        "database: {} programs; pipeline cache: {} KiB",
        store.len()?,
        gpu.pipeline_cache_data()?.len() / 1024
    );
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    let errors = run.errors.lock().expect("no worker panicked");
    Ok(
        if errors.is_empty()
            && counts.translation_failures.is_empty()
            && counts.pipeline_failures.is_empty()
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}
