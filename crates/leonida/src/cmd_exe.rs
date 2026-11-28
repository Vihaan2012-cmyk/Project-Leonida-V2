use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use game_package::{Manifest, map_file};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter};
use pe_image::Image;
use source_export::{text_at, thunk_target};

use crate::cli::{ExeArgs, ExeCommand};
use crate::game::resolve_game;

/// The executable `MicrosoftGame.Config` names; it is only ever read as data.
pub(crate) fn executable(game: &Path) -> Result<PathBuf> {
    let config = std::fs::read_to_string(game.join("MicrosoftGame.Config"))
        .context("reading MicrosoftGame.Config")?;
    let entry = config
        .split("<Executable ")
        .nth(1)
        .ok_or_else(|| anyhow!("MicrosoftGame.Config names no executable"))?;
    let name = entry
        .split("Name=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow!("the executable entry has no name"))?;
    if name.contains(['/', '\\']) || name.contains("..") {
        bail!("unexpected executable name in MicrosoftGame.Config");
    }
    Ok(game.join(name))
}

/// The one file of the game folder whose name starts with `prefix`, compared without regard to case.
fn image_by_prefix(game: &Path, prefix: &str) -> Result<PathBuf> {
    let prefix = prefix.to_ascii_lowercase();
    let mut found: Vec<PathBuf> = std::fs::read_dir(game)
        .with_context(|| format!("listing {}", game.display()))?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(&prefix)
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no file in the game folder starts with {prefix:?}"),
        many => bail!(
            "{} files start with {prefix:?}; use a longer prefix",
            many.len()
        ),
    }
}

pub fn exe_command(args: &ExeArgs) -> Result<ExitCode> {
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    if let ExeCommand::Triage { system, search } = &args.command {
        return crate::cmd_triage::triage_command(&game, system.as_deref(), search);
    }
    let path = match &args.image {
        Some(prefix) => image_by_prefix(&game, prefix)?,
        None => executable(&game)?,
    };
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = map_file(&file).context("mapping the executable")?;
    let image = Image::parse(&bytes)?;
    match &args.command {
        ExeCommand::Info => info(&image),
        ExeCommand::Decompile { address, depth } => decompile(&image, address, *depth)?,
        ExeCommand::ExportSource {
            out,
            jobs,
            resume,
            only,
            max_size,
            db,
            no_db,
            per_file,
        } => export_source(
            &image,
            (!*no_db).then(|| {
                db.clone()
                    .unwrap_or_else(|| out.parent().unwrap_or(Path::new("")).join("program.db"))
            }),
            source_export::Options {
                out: out.clone(),
                jobs: *jobs,
                resume: *resume,
                only: only.clone(),
                max_function_bytes: *max_size,
                functions_per_file: *per_file,
                ..source_export::Options::default()
            },
        )?,
        ExeCommand::Triage { .. } => unreachable!("handled before the image is opened"),
        ExeCommand::Imports { dll } => imports(&image, dll.as_deref()),
        ExeCommand::Network => network(&image),
        ExeCommand::Functions { top } => functions(&image, *top),
        ExeCommand::Disasm {
            address,
            limit,
            end,
        } => disasm(&image, address, *limit, end.as_deref())?,
        ExeCommand::Dump {
            address,
            len,
            width,
        } => dump(&image, address, *len, *width)?,
        ExeCommand::Analyze { out, threads } => analyze(&image, out, *threads)?,
        ExeCommand::Strings { find, min, limit } => strings(&image, find.as_deref(), *min, *limit),
        ExeCommand::Xrefs {
            address,
            import,
            string,
            limit,
        } => xrefs(
            &image,
            address.as_deref(),
            import.as_deref(),
            string.as_deref(),
            *limit,
        )?,
    }
    Ok(ExitCode::SUCCESS)
}

fn export_source(
    image: &Image<'_>,
    db_path: Option<PathBuf>,
    options: source_export::Options,
) -> Result<()> {
    let out = options.out.clone();
    let summary = match db_path {
        Some(path) => {
            let db = match exe_analysis::ProgramDb::load(&path) {
                Ok(db) if db.image_base() == image.image_base => {
                    println!("using {}", path.display());
                    db
                }
                _ => {
                    println!(
                        "building the program database (saved to {})",
                        path.display()
                    );
                    let build = exe_analysis::BuildOptions {
                        threads: options.jobs,
                    };
                    let db = exe_analysis::ProgramDb::build_with(image, &build);
                    db.save(&path)
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("{}", db.summary());
                    db
                }
            };
            source_export::export(image, &source_export::DbSymbols::new(&db), &options)?
        }
        None => source_export::export(image, &source_export::ImageSymbols::new(image), &options)?,
    };
    println!(
        "{} functions, {} failed, {} stubbed, {} files written ({} kept from an earlier run), {:.1}s",
        summary.functions,
        summary.failed,
        summary.stubbed,
        summary.files,
        summary.skipped_files,
        summary.elapsed.as_secs_f64()
    );
    println!("see {}", out.join("index.md").display());
    Ok(())
}

fn analyze(image: &Image<'_>, out: &Path, threads: usize) -> Result<()> {
    let options = exe_analysis::BuildOptions { threads };
    let db = exe_analysis::ProgramDb::build_with(image, &options);
    db.save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    println!("{}", db.summary());
    println!("saved to {}", out.display());
    Ok(())
}

fn info(image: &Image<'_>) {
    println!(
        "machine 0x{:04x}, image base 0x{:x}, size 0x{:x}, entry RVA 0x{:x}, subsystem {}, timestamp 0x{:08x}",
        image.machine,
        image.image_base,
        image.image_size,
        image.entry,
        image.subsystem,
        image.timestamp
    );
    println!("sections:");
    for section in &image.sections {
        let flags = format!(
            "{}{}",
            if section.executable() { "x" } else { "-" },
            if section.writable() { "w" } else { "-" }
        );
        println!(
            "  {:<10} RVA 0x{:08x} size 0x{:08x} file 0x{:08x}+0x{:08x} {flags}",
            section.name,
            section.virtual_address,
            section.virtual_size,
            section.raw_offset,
            section.raw_size
        );
    }
    let imports = image.imports();
    let imported: usize = imports.iter().map(|d| d.entries.len()).sum();
    let functions = image.functions();
    let code: u64 = functions.iter().map(|f| u64::from(f.end - f.begin)).sum();
    println!(
        "{} imported DLLs ({imported} functions), {} exports, {} functions in the exception table ({code} bytes), {} relocations",
        imports.len(),
        image.exports().len(),
        functions.len(),
        image.relocation_count()
    );
}

fn imports(image: &Image<'_>, filter: Option<&str>) {
    for dll in image.imports() {
        if filter.is_some_and(|f| !dll.name.to_lowercase().contains(&f.to_lowercase())) {
            continue;
        }
        let kind = if dll.delayed { " (delay-loaded)" } else { "" };
        println!("{}{kind}: {} functions", dll.name, dll.entries.len());
        for entry in &dll.entries {
            match (&entry.name, entry.ordinal) {
                (Some(name), _) => println!("  IAT 0x{:08x}  {name}", entry.iat_rva),
                (None, Some(ordinal)) => println!("  IAT 0x{:08x}  #{ordinal}", entry.iat_rva),
                _ => {}
            }
        }
    }
}

fn network(image: &Image<'_>) {
    let mut found: BTreeMap<offline::Reason, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut add = |reason, group: String, item: String| {
        found
            .entry(reason)
            .or_default()
            .entry(group)
            .or_default()
            .insert(item);
    };
    let mut imported = HashSet::new();
    for dll in image.imports() {
        imported.insert(dll.name.to_ascii_lowercase());
        for entry in &dll.entries {
            let name = match (&entry.name, entry.ordinal) {
                (Some(name), _) => name.clone(),
                (None, ordinal) => format!("#{}", ordinal.unwrap_or_default()),
            };
            imported.insert(name.to_ascii_lowercase());
            if let Some(reason) = offline::import(&dll.name, entry.name.as_deref()) {
                add(reason, format!("{} imports", dll.name), name);
            }
        }
    }
    for section in image.sections.iter().filter(|s| !s.executable()) {
        let Some(bytes) = image.read(section.virtual_address, section.raw_size as usize) else {
            continue;
        };
        for (_, text) in pe_image::strings(bytes, 6) {
            let lower = text.to_ascii_lowercase();
            if imported.contains(&lower) {
                continue;
            }
            if lower.ends_with(".dll") {
                if let Some(reason) = offline::import(&text, None) {
                    add(reason, "libraries it loads by name".into(), text);
                }
            } else if let Some(reason) =
                offline::path(&text).filter(|r| *r != offline::Reason::NetworkShares)
            {
                add(reason, "addresses and paths in its text".into(), text);
            } else if text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && let Some(reason) = offline::function(&text)
            {
                add(reason, "functions named in its text".into(), text);
            }
        }
    }
    for (reason, groups) in &found {
        println!("{}:", reason.describe());
        for (group, items) in groups {
            let items: Vec<&str> = items.iter().map(String::as_str).collect();
            println!("  {group} ({}): {}", items.len(), items.join(", "));
        }
    }
    println!("Leonida blocks every item above when the game runs: nothing reaches the network.");
}

fn functions(image: &Image<'_>, top: usize) {
    let mut functions = image.functions();
    let total = functions.len();
    functions.sort_by_key(|f| std::cmp::Reverse(f.end - f.begin));
    println!("{total} functions; the {top} largest:");
    for function in functions.iter().take(top) {
        println!(
            "  0x{:08x}..0x{:08x}  {:>8} bytes",
            function.begin,
            function.end,
            function.end - function.begin
        );
    }
}

fn import_names(image: &Image<'_>) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    for dll in image.imports() {
        for entry in dll.entries {
            let label = match (entry.name, entry.ordinal) {
                (Some(name), _) => format!("{}!{name}", dll.name),
                (None, Some(ordinal)) => format!("{}!#{ordinal}", dll.name),
                _ => continue,
            };
            names.insert(entry.iat_rva, label);
        }
    }
    names
}

/// The code RVA a pointer stored in the file at `rva` holds before any code runs.
fn code_pointer_at(image: &Image<'_>, rva: u32) -> Option<u32> {
    let value = u64::from_le_bytes(image.read(rva, 8)?.try_into().ok()?);
    let target = u32::try_from(value.checked_sub(image.image_base)?).ok()?;
    image.section_of(target)?.executable().then_some(target)
}

fn parse_address(image: &Image<'_>, text: &str) -> Result<u32> {
    let digits = text.trim_start_matches("0x");
    let value = u64::from_str_radix(digits, 16).context("addresses are hexadecimal")?;
    let rva = if value >= image.image_base {
        value - image.image_base
    } else {
        value
    };
    u32::try_from(rva).map_err(|_| anyhow!("address outside the image"))
}

fn dump(image: &Image<'_>, address: &str, len: usize, width: usize) -> Result<()> {
    let rva = parse_address(image, address)?;
    let bytes = image
        .read(rva, len)
        .ok_or_else(|| anyhow!("0x{rva:x}+{len} is not backed by file data"))?;
    for (n, line) in bytes.chunks(width.max(1)).enumerate() {
        let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        println!("{:08x}  {}", rva as usize + n * width.max(1), hex.join(" "));
    }
    Ok(())
}

fn disasm(image: &Image<'_>, address: &str, limit: usize, end: Option<&str>) -> Result<()> {
    let range_end = end;
    let rva = parse_address(image, address)?;
    let function = image.function_containing(rva);
    let (start, end) = match (end, function) {
        (Some(end), _) => (rva, parse_address(image, end)?),
        (None, Some(f)) => (f.begin, f.end),
        (None, None) => (rva, rva.saturating_add(0x100)),
    };
    let bytes = image
        .bytes_at(start)
        .ok_or_else(|| anyhow!("0x{start:x} is not backed by file data"))?;
    let length = ((end - start) as usize).min(bytes.len());
    let names = import_names(image);
    let starts: HashSet<u32> = image.functions().iter().map(|f| f.begin).collect();
    match function.filter(|_| range_end.is_none()) {
        Some(f) => println!(
            "; function 0x{:08x}..0x{:08x} ({} bytes)",
            f.begin,
            f.end,
            f.end - f.begin
        ),
        None if range_end.is_some() => println!("; range 0x{start:08x}..0x{end:08x}"),
        None => println!("; no exception-table entry covers 0x{rva:x}; showing 256 bytes"),
    }
    let base = image.image_base;
    let mut decoder = Decoder::with_ip(
        64,
        &bytes[..length],
        base + u64::from(start),
        DecoderOptions::NONE,
    );
    let mut formatter = IntelFormatter::new();
    let mut instruction = Instruction::default();
    let mut text = String::new();
    let mut printed = 0;
    while decoder.can_decode() && printed < limit {
        decoder.decode_out(&mut instruction);
        text.clear();
        formatter.format(&instruction, &mut text);
        let mut note = String::new();
        if instruction.is_ip_rel_memory_operand() {
            let target = (instruction.ip_rel_memory_address() - base) as u32;
            if let Some(name) = names.get(&target) {
                note = format!("  ; {name}");
            } else if let Some(text) = text_at(image, target) {
                note = format!("  ; {text:?}");
            } else if let Some(code) =
                code_pointer_at(image, target).filter(|_| instruction.memory_size().size() == 8)
            {
                note = format!("  ; initially sub_{code:08x}");
            }
        }
        let call = instruction.flow_control() == FlowControl::Call;
        if (call || instruction.flow_control() == FlowControl::UnconditionalBranch)
            && instruction.near_branch_target() != 0
        {
            let target = (instruction.near_branch_target() - base) as u32;
            if let Some(name) = thunk_target(image, target).and_then(|slot| names.get(&slot)) {
                note = format!("  ; {name}");
            } else if call || (starts.contains(&target) && target != start) {
                note = format!("  ; sub_{target:08x}");
            }
        }
        println!("  {:08x}  {text}{note}", instruction.ip() - base);
        printed += 1;
    }
    Ok(())
}

fn strings(image: &Image<'_>, find: Option<&str>, min: usize, limit: usize) {
    let mut printed = 0;
    for section in image.sections.iter().filter(|s| !s.executable()) {
        let Some(bytes) = image.read(section.virtual_address, section.raw_size as usize) else {
            continue;
        };
        for (offset, text) in pe_image::strings(bytes, min) {
            if find.is_some_and(|f| !text.contains(f)) {
                continue;
            }
            println!(
                "{} 0x{:08x}  {text}",
                section.name,
                section.virtual_address + offset as u32
            );
            printed += 1;
            if printed >= limit {
                return;
            }
        }
    }
}

/// Stubs of the form `jmp qword ptr [rip+x]` through one of the IAT slots in `slots`. Most lie
/// outside the exception table, so the whole code section is searched for them.
fn import_thunks(image: &Image<'_>, slots: &HashSet<u32>) -> Vec<u32> {
    let mut thunks = Vec::new();
    for section in image.sections.iter().filter(|s| s.executable()) {
        let Some(bytes) = image.read(section.virtual_address, section.raw_size as usize) else {
            continue;
        };
        for (offset, window) in bytes.windows(6).enumerate() {
            if window[0] != 0xFF || window[1] != 0x25 {
                continue;
            }
            let rva = section.virtual_address + offset as u32;
            let displacement = i32::from_le_bytes([window[2], window[3], window[4], window[5]]);
            let slot = i64::from(rva) + 6 + i64::from(displacement);
            if u32::try_from(slot).is_ok_and(|slot| slots.contains(&slot)) {
                thunks.push(rva);
            }
        }
    }
    thunks
}

fn xrefs(
    image: &Image<'_>,
    address: Option<&str>,
    import: Option<&str>,
    string: Option<&str>,
    limit: usize,
) -> Result<()> {
    let mut targets = HashSet::new();
    if let Some(address) = address {
        targets.insert(parse_address(image, address)?);
    }
    if let Some(wanted) = import {
        for dll in image.imports() {
            for entry in dll.entries {
                if entry.name.as_deref() == Some(wanted) {
                    targets.insert(entry.iat_rva);
                }
            }
        }
    }
    if let Some(wanted) = string {
        for section in image.sections.iter().filter(|s| !s.executable()) {
            let Some(bytes) = image.read(section.virtual_address, section.raw_size as usize) else {
                continue;
            };
            for (offset, text) in pe_image::strings(bytes, wanted.len().min(4)) {
                if text.contains(wanted) {
                    targets.insert(section.virtual_address + offset as u32);
                }
            }
        }
    }
    if targets.is_empty() {
        bail!("nothing to look for: give an address, --import or --string");
    }
    let functions = image.functions();
    let thunks = import_thunks(image, &targets);
    targets.extend(thunks);
    let base = image.image_base;
    let rva = |va: u64| va.wrapping_sub(base) as u32;
    let mut formatter = IntelFormatter::new();
    let mut instruction = Instruction::default();
    let mut text = String::new();
    let mut found = 0;
    for function in &functions {
        let Some(bytes) = image.read(function.begin, (function.end - function.begin) as usize)
        else {
            continue;
        };
        let mut decoder = Decoder::with_ip(
            64,
            bytes,
            base + u64::from(function.begin),
            DecoderOptions::NONE,
        );
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            let memory = instruction.is_ip_rel_memory_operand()
                && targets.contains(&rva(instruction.ip_rel_memory_address()));
            let branch = instruction.near_branch_target();
            if memory || (branch != 0 && targets.contains(&rva(branch))) {
                text.clear();
                formatter.format(&instruction, &mut text);
                println!(
                    "  {:08x}  in sub_{:08x}  {text}",
                    rva(instruction.ip()),
                    function.begin
                );
                found += 1;
                if found >= limit {
                    println!("  (stopped at {limit})");
                    return Ok(());
                }
            }
        }
    }
    println!("{found} references");
    Ok(())
}

fn decompile(image: &Image<'_>, address: &str, depth: usize) -> Result<()> {
    let rva = parse_address(image, address)?;
    let symbols = source_export::ImageSymbols::new(image);
    let program = source_export::ImageProgram::new(
        image,
        &image.functions(),
        &source_export::ImportTable::new(image),
        &symbols,
    );
    let start = program
        .containing(rva)
        .map_or(rva, |f| image.primary(f).begin);
    let mut queue = std::collections::VecDeque::from([(start, 0)]);
    let mut seen = HashSet::from([start]);
    while let Some((rva, level)) = queue.pop_front() {
        let entry = image.image_base + u64::from(rva);
        let text = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(256 << 20)
                .spawn_scoped(scope, || decompile::decompile(&program, entry))
                .map(|handle| handle.join())
        })
        .context("starting the decompiler")?;
        match text {
            Ok(Ok(text)) => println!("{text}"),
            Ok(Err(error)) => println!("// sub_{rva:08x}: {error}\n"),
            Err(_) => println!("// sub_{rva:08x}: the decompiler hit an internal error\n"),
        }
        if level < depth {
            let Some(function) = program.containing(rva) else {
                continue;
            };
            let Some(bytes) = image.read(function.begin, (function.end - function.begin) as usize)
            else {
                continue;
            };
            let mut decoder = Decoder::with_ip(
                64,
                bytes,
                image.image_base + u64::from(function.begin),
                DecoderOptions::NONE,
            );
            for instruction in decoder.iter() {
                if instruction.flow_control() != FlowControl::Call
                    || instruction.near_branch_target() == 0
                {
                    continue;
                }
                let Some(callee) = program.rva(instruction.near_branch_target()) else {
                    continue;
                };
                if thunk_target(image, callee).is_some() || !seen.insert(callee) {
                    continue;
                }
                queue.push_back((callee, level + 1));
            }
        }
    }
    Ok(())
}
