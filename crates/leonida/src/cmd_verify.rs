use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gpu::Gpu;
use shader_container::CarvedShader;
use shader_store::{Source, container_programs, translate_built};

use crate::cli::{TranslateArgs, VerifyArgs};
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

/// The failure class of a translation error and its detail with numbers removed, so equal causes group.
fn classify(error: &str) -> (&'static str, String) {
    let numberless = |text: &str| {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '0' && chars.peek() == Some(&'x') {
                chars.next();
                while chars.next_if(char::is_ascii_hexdigit).is_some() {}
                out.push('#');
            } else if c.is_ascii_digit() {
                while chars.next_if(char::is_ascii_digit).is_some() {}
                out.push('#');
            } else {
                out.push(c);
            }
        }
        out
    };
    if let Some(rest) = error.strip_prefix("unsupported instruction at ") {
        let text = rest.split_once(": ").map_or(rest, |(_, text)| text);
        let mnemonic = text.split_whitespace().next().unwrap_or("?");
        return ("unsupported instruction", mnemonic.to_string());
    }
    if error.starts_with("invalid instruction words") {
        return ("undecodable words", String::new());
    }
    if error.starts_with("branch at ") {
        return ("control flow", numberless(error));
    }
    if error.starts_with("the IR does not verify") {
        return ("IR does not verify", numberless(error));
    }
    if error.starts_with("building SPIR-V failed") {
        return ("SPIR-V construction", numberless(error));
    }
    if error.starts_with("no ") || error.contains("geometry") || error.contains("mesh shader") {
        return ("missing stage setup", numberless(error));
    }
    if error.starts_with("LDS")
        || error.contains("cube maps")
        || error.contains("without a sampler")
    {
        return ("unsupported operand or resource", numberless(error));
    }
    ("other", numberless(error))
}

type Details = BTreeMap<String, (usize, String)>;

#[derive(Default)]
struct Translation {
    containers: usize,
    programs: BTreeMap<&'static str, usize>,
    translated: usize,
    failed: BTreeMap<&'static str, Details>,
    saved: usize,
}

fn note(details: &mut Details, detail: String, name: &str) {
    let entry = details.entry(detail).or_default();
    entry.0 += 1;
    if entry.1.is_empty() {
        entry.1 = name.to_string();
    }
}

impl Translation {
    fn merge(&mut self, other: Translation) {
        self.containers += other.containers;
        self.translated += other.translated;
        self.saved += other.saved;
        for (kind, count) in other.programs {
            *self.programs.entry(kind).or_default() += count;
        }
        for (class, details) in other.failed {
            let mine = self.failed.entry(class).or_default();
            for (detail, (count, example)) in details {
                let entry = mine.entry(detail).or_default();
                entry.0 += count;
                if entry.1.is_empty() {
                    entry.1 = example;
                }
            }
        }
    }

    fn print(&self, detail_rows: usize) {
        let total_of = |details: &Details| details.values().map(|(count, _)| count).sum::<usize>();
        let failed: usize = self.failed.values().map(total_of).sum();
        println!("containers: {}", self.containers);
        println!("programs: {} translated, {failed} failed", self.translated);
        for (kind, count) in &self.programs {
            println!("  {kind}: {count} translated");
        }
        let mut classes: Vec<_> = self.failed.iter().collect();
        classes.sort_by_key(|(_, details)| std::cmp::Reverse(total_of(details)));
        for (class, details) in classes {
            println!("{class}: {}", total_of(details));
            let mut rows: Vec<_> = details.iter().collect();
            rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
            for (detail, (count, example)) in rows.into_iter().take(detail_rows) {
                println!("  {count:>6}  {detail}   e.g. {example}");
            }
        }
    }
}

fn translate_container(
    state: &mut Translation,
    args: &TranslateArgs,
    saved: &AtomicUsize,
    shader: &CarvedShader,
) {
    state.containers += 1;
    let sources = match container_programs(shader.stage, &shader.dxbc) {
        Ok(sources) => sources,
        Err(error) => {
            note(
                state.failed.entry("unreadable container").or_default(),
                error,
                &shader.name,
            );
            return;
        }
    };
    for source in &sources {
        match translate_built(source) {
            Ok(translated) => {
                state.translated += 1;
                *state.programs.entry(translated.built.kind()).or_default() += 1;
                let Some(folder) = &args.save_spirv else {
                    continue;
                };
                let index = saved.fetch_add(1, Ordering::Relaxed);
                if index >= args.save_max {
                    continue;
                }
                let bytes: Vec<u8> = translated
                    .built
                    .spirv()
                    .iter()
                    .flat_map(|w| w.to_le_bytes())
                    .collect();
                let kind = translated.built.kind().replace(' ', "_");
                let path = folder.join(format!("{}.{index}.{kind}.spv", shader.name));
                if std::fs::write(&path, bytes).is_ok() {
                    state.saved += 1;
                }
            }
            Err(failure) => {
                let (class, detail) = classify(&failure.error);
                if args.each {
                    println!("failed: {}: {class}: {detail}", shader.name);
                }
                note(state.failed.entry(class).or_default(), detail, &shader.name);
            }
        }
    }
}

pub fn translate_command(args: &TranslateArgs) -> Result<ExitCode> {
    if let Some(folder) = &args.save_spirv {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("creating {}", folder.display()))?;
    }
    let every = args.every.max(1);
    let seen = AtomicUsize::new(0);
    let saved = AtomicUsize::new(0);
    let started = Instant::now();
    let selection = Selection {
        game: args.game.as_ref(),
        limit: args.limit,
        stage: args.stage.as_deref(),
        name: args.name.as_deref(),
    };
    let states = for_each_container(
        &selection,
        args.jobs,
        || -> Result<Translation> { Ok(Translation::default()) },
        |state, shader| {
            if seen.fetch_add(1, Ordering::Relaxed) % every == 0 {
                translate_container(state, args, &saved, shader);
            }
        },
    )?;
    let mut report = Translation::default();
    for state in states {
        report.merge(state);
    }
    report.print(12);
    println!("saved {} programs", report.saved);
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    Ok(if report.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn errors_group_by_kind_and_ignore_numbers() {
        assert_eq!(
            classify("unsupported instruction at 0x1f4: v_foo_f32 v1, v2"),
            ("unsupported instruction", "v_foo_f32".to_string())
        );
        assert_eq!(classify("invalid instruction words").0, "undecodable words");
        assert_eq!(
            classify("branch at 0x40 leaves the program"),
            ("control flow", "branch at # leaves the program".to_string())
        );
        assert_eq!(
            classify("LDS in a pixel shader").0,
            "unsupported operand or resource"
        );
        assert_eq!(classify("something odd 12").1, "something odd #");
    }
}
