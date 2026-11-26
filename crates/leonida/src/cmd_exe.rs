use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use game_package::{Manifest, map_file};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter};
use pe_image::Image;

use crate::cli::{ExeArgs, ExeCommand};
use crate::game::resolve_game;

/// The executable `MicrosoftGame.Config` names; it is only ever read as data.
fn executable(game: &Path) -> Result<PathBuf> {
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

pub fn exe_command(args: &ExeArgs) -> Result<ExitCode> {
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let path = executable(&game)?;
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = map_file(&file).context("mapping the executable")?;
    let image = Image::parse(&bytes)?;
    match &args.command {
        ExeCommand::Info => info(&image),
        ExeCommand::Imports { dll } => imports(&image, dll.as_deref()),
        ExeCommand::Functions { top } => functions(&image, *top),
        ExeCommand::Disasm { address, limit } => disasm(&image, address, *limit)?,
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

/// The IAT slot an import thunk (`jmp qword ptr [rip+x]`) at `rva` jumps through.
fn thunk_target(image: &Image<'_>, rva: u32) -> Option<u32> {
    let bytes = image.read(rva, 16)?;
    let mut decoder = Decoder::with_ip(
        64,
        bytes,
        image.image_base + u64::from(rva),
        DecoderOptions::NONE,
    );
    let instruction = decoder.decode();
    (instruction.flow_control() == FlowControl::IndirectBranch
        && instruction.is_ip_rel_memory_operand())
    .then(|| (instruction.ip_rel_memory_address() - image.image_base) as u32)
}

/// A NUL-terminated run of at least four printable bytes at `rva` in a data section.
fn text_at(image: &Image<'_>, rva: u32) -> Option<String> {
    if image.section_of(rva)?.executable() {
        return None;
    }
    let bytes = image.bytes_at(rva)?;
    let end = bytes.iter().take(512).position(|b| *b == 0)?;
    let printable = bytes[..end]
        .iter()
        .all(|b| (0x20..0x7F).contains(b) || matches!(b, b'\t' | b'\n' | b'\r'));
    (end >= 4 && printable).then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
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

fn disasm(image: &Image<'_>, address: &str, limit: usize) -> Result<()> {
    let rva = parse_address(image, address)?;
    let function = image.function_containing(rva);
    let (start, end) = match function {
        Some(f) => (f.begin, f.end),
        None => (rva, rva.saturating_add(0x100)),
    };
    let bytes = image
        .bytes_at(start)
        .ok_or_else(|| anyhow!("0x{start:x} is not backed by file data"))?;
    let length = ((end - start) as usize).min(bytes.len());
    let names = import_names(image);
    let starts: HashSet<u32> = image.functions().iter().map(|f| f.begin).collect();
    match function {
        Some(f) => println!(
            "; function 0x{:08x}..0x{:08x} ({} bytes)",
            f.begin,
            f.end,
            f.end - f.begin
        ),
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
    let thunks: Vec<u32> = functions
        .iter()
        .filter(|f| f.end - f.begin <= 16)
        .filter(|f| thunk_target(image, f.begin).is_some_and(|slot| targets.contains(&slot)))
        .map(|f| f.begin)
        .collect();
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
