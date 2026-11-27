use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gpu::Gpu;
use shader_container::CarvedShader;
use shader_store::{Source, container_programs, translate_built};

use crate::cli::VerifyArgs;
use crate::pipeline_work::{Outcomes, Partners, Selection, build_pipeline, for_each_container};

#[derive(Default)]
struct Tally {
    translated: usize,
    words: usize,
    passed: usize,
    translate_time: Duration,
    pipeline_time: Duration,
}

#[derive(Default)]
struct Verify {
    each: bool,
    save: Option<PathBuf>,
    saved: HashMap<String, usize>,
    replacement: Option<Vec<u32>>,
    no_build: bool,
    /// Every program built so far, by fingerprint, with how its pipeline went; shared by the workers.
    built: Outcomes,
    kinds: BTreeMap<&'static str, Tally>,
    failures: BTreeMap<String, (usize, String)>,
    containers: usize,
}

impl Verify {
    fn fail(&mut self, error: String, name: &str) {
        println!("failed: {name}: {error}");
        let entry = self.failures.entry(error).or_default();
        entry.0 += 1;
        if entry.1.is_empty() {
            entry.1 = name.to_string();
        }
    }

    fn program(&mut self, gpu: &Gpu, partners: &mut Partners, name: &str, source: &Source) {
        let started = Instant::now();
        let built = translate_built(source);
        let translate_time = started.elapsed();
        let mut built = match built {
            Ok(translated) => translated.built,
            Err(failure) => return self.fail(format!("translation: {}", failure.error), name),
        };
        if let Some(folder) = &self.save {
            let bytes: Vec<u8> = built.spirv().iter().flat_map(|w| w.to_le_bytes()).collect();
            let count = self.saved.entry(name.to_string()).or_default();
            *count += 1;
            let kind = built.kind().replace(' ', "_");
            let path = folder.join(format!("{name}.{count}.{kind}.spv"));
            if let Err(error) = std::fs::write(&path, bytes) {
                println!("could not write {}: {error}", path.display());
            }
        }
        if let Some(words) = &self.replacement {
            *built.spirv_mut() = words.clone();
        }
        if self.no_build {
            if self.each {
                println!("{name}: {} {} KiB", built.kind(), built.spirv().len() / 256);
            }
            return;
        }
        let tally = self.kinds.entry(built.kind()).or_default();
        tally.translated += 1;
        tally.words += built.spirv().len();
        tally.translate_time += translate_time;
        let started = Instant::now();
        let (outcome, _) = build_pipeline(gpu, partners, &built, &self.built);
        let took = started.elapsed();
        if self.each {
            println!(
                "{name}: {} {} KiB, {:.2} s",
                built.kind(),
                built.spirv().len() / 256,
                took.as_secs_f64()
            );
        }
        let tally = self.kinds.entry(built.kind()).or_default();
        tally.pipeline_time += took;
        match outcome {
            Err(error) => self.fail(error, name),
            Ok(()) => self.kinds.entry(built.kind()).or_default().passed += 1,
        }
    }

    fn print(&self) {
        println!("containers: {}", self.containers);
        for (kind, tally) in &self.kinds {
            let per =
                |total: Duration| total.as_secs_f64() * 1000.0 / tally.translated.max(1) as f64;
            println!(
                "{kind}: {} translated, {} built into pipelines; {} KiB of SPIR-V, {:.1} ms to translate and {:.1} ms to build each",
                tally.translated,
                tally.passed,
                tally.words * 4 / 1024 / tally.translated.max(1),
                per(tally.translate_time),
                per(tally.pipeline_time)
            );
        }
        let failed: usize = self.failures.values().map(|(count, _)| count).sum();
        println!("failures: {failed}");
        for (error, (count, example)) in &self.failures {
            println!("  {count:>6}  {error}   e.g. {example}");
        }
    }
}

impl Verify {
    fn container(&mut self, gpu: &Gpu, partners: &mut Partners, shader: &CarvedShader) {
        match container_programs(shader.stage, &shader.dxbc) {
            Ok(sources) => {
                for source in &sources {
                    self.program(gpu, partners, &shader.name, source);
                }
            }
            Err(error) => self.fail(error, &shader.name),
        }
    }

    fn merge(&mut self, other: Verify) {
        self.containers += other.containers;
        for (kind, tally) in other.kinds {
            let mine = self.kinds.entry(kind).or_default();
            mine.translated += tally.translated;
            mine.passed += tally.passed;
            mine.words += tally.words;
            mine.translate_time += tally.translate_time;
            mine.pipeline_time += tally.pipeline_time;
        }
        for (error, (count, example)) in other.failures {
            let entry = self.failures.entry(error).or_default();
            entry.0 += count;
            if entry.1.is_empty() {
                entry.1 = example;
            }
        }
    }
}

pub fn verify_command(args: &VerifyArgs) -> Result<ExitCode> {
    let replacement = match &args.use_spirv {
        Some(path) => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            Some(
                bytes
                    .chunks_exact(4)
                    .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
                    .collect::<Vec<u32>>(),
            )
        }
        None => None,
    };
    if let Some(folder) = &args.save_spirv {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("creating {}", folder.display()))?;
    }
    let gpu = Gpu::new().context("opening the GPU")?;
    let built: Outcomes = Arc::new(Mutex::new(HashMap::new()));
    let started = Instant::now();
    let selection = Selection {
        game: args.game.as_ref(),
        limit: args.limit,
        stage: args.stage.as_deref(),
        name: args.name.as_deref(),
    };
    let reports = for_each_container(
        &selection,
        args.jobs,
        || -> Result<(Partners, Verify)> {
            let verify = Verify {
                each: args.each,
                save: args.save_spirv.clone(),
                replacement: replacement.clone(),
                no_build: args.no_build,
                built: Arc::clone(&built),
                ..Verify::default()
            };
            Ok((Partners::new()?, verify))
        },
        |(partners, verify), shader| {
            verify.containers += 1;
            verify.container(&gpu, partners, shader);
        },
    )?;
    let mut report = Verify::default();
    for (_, verify) in reports {
        report.merge(verify);
    }
    report.print();
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    Ok(if report.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
