use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pe_image::Image;
use rayon::prelude::*;

use crate::image_data::ImportTable;
use crate::index;
use crate::plan::{self, Selection, Selector};
use crate::program::ImageProgram;
use crate::render::{self, MARKER, Method, Outcome, View};
use crate::scan;
use crate::symbols::{ClassInfo, Symbols};

/// Stack for each worker: the decompiler recurses on the structure of the function.
const WORKER_STACK: usize = 256 << 20;
const END_MARK: &str = "// end: ";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("starting the worker threads: {0}")]
    Pool(String),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub out: PathBuf,
    pub jobs: usize,
    pub resume: bool,
    /// Hex RVAs (`0x...`, `sub_...`), function names, classes or part of a file path.
    pub only: Vec<String>,
    /// Functions larger than this are stubbed without being decompiled.
    pub max_function_bytes: u32,
    pub functions_per_file: usize,
    pub progress_every: usize,
    /// Seconds after which a function still being decompiled is reported.
    pub slow_secs: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            out: PathBuf::from("Output/decomp/src"),
            jobs: 4,
            resume: false,
            only: Vec::new(),
            max_function_bytes: 0x2_0000,
            functions_per_file: 200,
            progress_every: 5000,
            slow_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Functions decompiled or stubbed in this run.
    pub functions: usize,
    pub failed: usize,
    pub stubbed: usize,
    /// Code files and class headers written in this run.
    pub files: usize,
    /// Files left as they were because `--resume` found them complete.
    pub skipped_files: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileStats {
    pub functions: usize,
    pub stubbed: usize,
    pub failed: usize,
}

pub(crate) fn parse_end(tail: &str) -> Option<FileStats> {
    let line = tail.lines().rev().find(|l| !l.trim().is_empty())?;
    let numbers = line.strip_prefix(END_MARK)?;
    let mut stats = FileStats::default();
    for part in numbers.split(',') {
        let (key, value) = part.trim().split_once(' ')?;
        let value = value.trim().parse().ok()?;
        match key {
            "functions" => stats.functions = value,
            "stubbed" => stats.stubbed = value,
            "failed" => stats.failed = value,
            _ => return None,
        }
    }
    Some(stats)
}

/// The totals a finished file ends with, or None if it is missing or was cut short.
pub(crate) fn finished_stats(path: &Path) -> Option<FileStats> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(256)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    parse_end(&String::from_utf8_lossy(&tail))
}

pub(crate) fn write_file(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".part");
    let temporary = PathBuf::from(temporary);
    let file = File::create(&temporary).map_err(io_error(&temporary))?;
    let mut writer = BufWriter::new(file);
    write(&mut writer).map_err(io_error(&temporary))?;
    writer.flush().map_err(io_error(&temporary))?;
    drop(writer);
    fs::rename(&temporary, path).map_err(io_error(path))
}

pub(crate) fn write_text(path: &Path, body: &str) -> Result<(), Error> {
    write_file(path, |w| write!(w, "{MARKER}\n\n{body}"))
}

struct Progress {
    started: Instant,
    total: usize,
    every: usize,
    done: AtomicUsize,
    failed: AtomicUsize,
    stubbed: AtomicUsize,
    running: Vec<Mutex<Option<(u32, Instant)>>>,
}

impl Progress {
    fn finished(&self, outcome: &Outcome) {
        match outcome {
            Outcome::Decompiled(_) => {}
            Outcome::Stub(_) => {
                self.stubbed.fetch_add(1, Ordering::Relaxed);
            }
            Outcome::Failed(_) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if self.every > 0 && done.is_multiple_of(self.every) {
            eprintln!(
                "  {done}/{} functions, {} failed, {} stubbed, {:.0}s",
                self.total,
                self.failed.load(Ordering::Relaxed),
                self.stubbed.load(Ordering::Relaxed),
                self.started.elapsed().as_secs_f64()
            );
        }
    }

    fn slot(&self) -> &Mutex<Option<(u32, Instant)>> {
        let thread = rayon::current_thread_index().unwrap_or(0);
        &self.running[thread.min(self.running.len() - 1)]
    }

    fn watch(&self, stop: &AtomicBool, slow: Duration) {
        let mut reported: Vec<u32> = Vec::new();
        let mut waited = Duration::ZERO;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
            waited += Duration::from_millis(200);
            if waited < Duration::from_secs(5) {
                continue;
            }
            waited = Duration::ZERO;
            for slot in &self.running {
                let Ok(slot) = slot.lock() else { continue };
                if let Some((rva, since)) = *slot
                    && since.elapsed() >= slow
                    && !reported.contains(&rva)
                {
                    reported.push(rva);
                    eprintln!(
                        "  slow: sub_{rva:08x} has been decompiling for {}s",
                        since.elapsed().as_secs()
                    );
                }
            }
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

fn decompile_one(
    program: &ImageProgram<'_>,
    base: u64,
    rva: u32,
    size: u32,
    limit: u32,
) -> Outcome {
    if size > limit {
        return Outcome::Stub(format!(
            "the function is {size} bytes, over the limit of {limit}"
        ));
    }
    match catch_unwind(AssertUnwindSafe(|| {
        decompile::decompile(program, base + u64::from(rva))
    })) {
        Ok(Ok(text)) => Outcome::Decompiled(text),
        Ok(Err(error)) => {
            let message = error.to_string();
            if message.contains("too large") || message.contains("limit") {
                Outcome::Stub(message)
            } else {
                Outcome::Failed(message)
            }
        }
        Err(payload) => Outcome::Failed(format!(
            "the decompiler panicked: {}",
            panic_text(&*payload)
        )),
    }
}

struct Run<'a> {
    options: &'a Options,
    view: View<'a>,
    program: ImageProgram<'a>,
    progress: Progress,
}

impl Run<'_> {
    fn write_group(&self, group: usize) -> Result<FileStats, Error> {
        let plan = self.view.plan;
        let group = &plan.groups[group];
        let path = self.options.out.join(format!("{}.c", group.path));
        let mut stats = FileStats::default();
        let base = self.view.image.image_base;
        write_file(&path, |w| {
            writeln!(w, "{MARKER}")?;
            writeln!(w, "// file: {}.c", group.path)?;
            writeln!(
                w,
                "// {}: {} functions\n",
                group.kind.label(),
                group.members.len()
            )?;
            for &member in &group.members {
                let entry = self.view.facts.entries[member as usize];
                *self.progress.slot().lock().expect("slot") = Some((entry.rva, Instant::now()));
                let outcome = decompile_one(
                    &self.program,
                    base,
                    entry.rva,
                    entry.size,
                    self.options.max_function_bytes,
                );
                *self.progress.slot().lock().expect("slot") = None;
                self.progress.finished(&outcome);
                stats.functions += 1;
                match outcome {
                    Outcome::Stub(_) => stats.stubbed += 1,
                    Outcome::Failed(_) => stats.failed += 1,
                    Outcome::Decompiled(_) => {}
                }
                w.write_all(
                    self.view
                        .function_text(member as usize, &outcome)
                        .as_bytes(),
                )?;
            }
            writeln!(
                w,
                "{END_MARK}functions {}, stubbed {}, failed {}",
                stats.functions, stats.stubbed, stats.failed
            )
        })?;
        Ok(stats)
    }
}

pub(crate) struct Written {
    pub path: String,
}

fn class_headers(
    view: &View<'_>,
    classes: &[ClassInfo],
    selection: &Selection,
    options: &Options,
) -> Result<(Vec<Written>, usize, usize), Error> {
    let plan = view.plan;
    let mut methods: HashMap<&str, Vec<Method>> = HashMap::new();
    let mut wanted: Vec<&str> = Vec::new();
    for (index, class) in plan.classes.iter().enumerate() {
        let Some(class) = class.as_deref() else {
            continue;
        };
        let group = plan.group_of[index] as usize;
        let entry = view.facts.entries[index];
        methods.entry(class).or_default().push(Method {
            name: plan.names[index].clone(),
            rva: entry.rva,
            path: format!("{}.c", plan.groups[group].path),
        });
        if selection.picked[group] {
            wanted.push(class);
        }
    }
    let everything = selection.picked.iter().all(|p| *p);
    let mut by_path: HashMap<String, Vec<&str>> = HashMap::new();
    let infos: HashMap<&str, &ClassInfo> = classes.iter().map(|c| (c.name.as_str(), c)).collect();
    for name in methods.keys().copied().chain(infos.keys().copied()) {
        if !everything && !wanted.contains(&name) {
            continue;
        }
        if let Some(path) = plan::class_path(name) {
            let list = by_path.entry(path.to_ascii_lowercase()).or_default();
            if !list.contains(&name) {
                list.push(name);
            }
        }
    }
    let resolve = |rva: u32| {
        let index = view.facts.index_of(rva)?;
        Some((
            plan.names[index].clone(),
            format!("{}.c", plan.groups[plan.group_of[index] as usize].path),
        ))
    };
    let mut written = Vec::new();
    let (mut skipped, mut count) = (0, 0);
    let mut paths: Vec<_> = by_path.into_iter().collect();
    paths.sort();
    for (lower, mut names) in paths {
        names.sort_unstable();
        let path = plan::class_path(names[0]).unwrap_or(lower);
        let file = options.out.join(format!("{path}.h"));
        if options.resume && finished_header(&file) {
            skipped += 1;
            continue;
        }
        let mut body = String::new();
        for name in &names {
            let mut list = methods.remove(name).unwrap_or_default();
            list.sort_by_key(|m| m.rva);
            body.push_str(&render::class_header(
                name,
                infos.get(name).copied(),
                &list,
                &resolve,
            ));
            body.push('\n');
        }
        write_file(&file, |w| {
            write!(w, "{MARKER}\n\n{body}{END_MARK}classes {}\n", names.len())
        })?;
        count += 1;
        written.push(Written {
            path: format!("{path}.h"),
        });
    }
    Ok((written, count, skipped))
}

fn finished_header(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(length) = file.metadata().map(|m| m.len()) else {
        return false;
    };
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(length.saturating_sub(128)))
        .is_ok()
        && file.read_to_end(&mut tail).is_ok()
        && String::from_utf8_lossy(&tail)
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.starts_with(&format!("{END_MARK}classes ")))
}

pub fn export(
    image: &Image<'_>,
    symbols: &dyn Symbols,
    options: &Options,
) -> Result<Summary, Error> {
    let started = Instant::now();
    let jobs = options.jobs.max(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(WORKER_STACK)
        .build()
        .map_err(|e| Error::Pool(e.to_string()))?;
    pool.install(|| run(image, symbols, options, jobs, started))
}

fn run(
    image: &Image<'_>,
    symbols: &dyn Symbols,
    options: &Options,
    jobs: usize,
    started: Instant,
) -> Result<Summary, Error> {
    let functions = image.functions();
    let imports = ImportTable::new(image);
    eprintln!(
        "scanning {} functions in the exception table",
        functions.len()
    );
    let facts = scan::analyze(image, &imports, symbols, &functions);
    eprintln!(
        "{} functions to write ({} found only through calls), scanned in {:.0}s",
        facts.entries.len(),
        facts.entries.iter().filter(|e| e.size == 0).count(),
        started.elapsed().as_secs_f64()
    );
    let plan = plan::build(&facts.entries, symbols, options.functions_per_file);
    let partial = !options.only.is_empty();
    let selection = if partial {
        let selectors: Vec<Selector> = options.only.iter().map(|s| Selector::parse(s)).collect();
        plan::select(&plan, &facts.entries, symbols, &selectors)
    } else {
        Selection::everything(&plan)
    };
    let total: usize = plan
        .groups
        .iter()
        .zip(&selection.picked)
        .filter(|(_, picked)| **picked)
        .map(|(g, _)| g.members.len())
        .sum();
    let run = Run {
        options,
        view: View {
            image,
            facts: &facts,
            plan: &plan,
            imports: &imports,
            symbols,
        },
        program: ImageProgram::new(image, &functions, &imports),
        progress: Progress {
            started,
            total,
            every: options.progress_every,
            done: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            stubbed: AtomicUsize::new(0),
            running: (0..jobs).map(|_| Mutex::new(None)).collect(),
        },
    };

    let stop = AtomicBool::new(false);
    let slow = Duration::from_secs(options.slow_secs);
    let results: Result<Vec<(usize, FileStats, bool)>, Error> = std::thread::scope(|scope| {
        scope.spawn(|| run.progress.watch(&stop, slow));
        let results = plan
            .groups
            .par_iter()
            .enumerate()
            .filter(|(g, _)| selection.picked[*g])
            .map(|(g, group)| {
                let path = options.out.join(format!("{}.c", group.path));
                if options.resume
                    && let Some(stats) = finished_stats(&path)
                {
                    return Ok((g, stats, true));
                }
                run.write_group(g).map(|stats| (g, stats, false))
            })
            .collect();
        stop.store(true, Ordering::Relaxed);
        results
    });
    let results = results?;

    let classes = symbols.classes();
    let (headers, header_count, header_skips) =
        class_headers(&run.view, &classes, &selection, options)?;

    let mut summary = Summary {
        functions: run.progress.done.load(Ordering::Relaxed),
        failed: run.progress.failed.load(Ordering::Relaxed),
        stubbed: run.progress.stubbed.load(Ordering::Relaxed),
        files: header_count,
        skipped_files: header_skips,
        elapsed: Duration::ZERO,
    };
    let mut stats_of: HashMap<usize, FileStats> = HashMap::new();
    for (g, stats, skipped) in &results {
        stats_of.insert(*g, *stats);
        if *skipped {
            summary.skipped_files += 1;
        } else {
            summary.files += 1;
        }
    }
    index::write(&index::Inputs {
        view: &run.view,
        options,
        stats: &stats_of,
        headers: &headers,
        partial,
    })?;
    summary.elapsed = started.elapsed();
    Ok(summary)
}
