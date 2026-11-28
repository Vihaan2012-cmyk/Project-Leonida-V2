//! `leonida exe triage`: static malware triage of every executable image in the game folder. Nothing is loaded or
//! run; each file is mapped as data and only names, counts and short matched strings are printed.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use exe_analysis::triage::{
    self, ApiCategory, HIGH_ENTROPY_DATA, HIGH_ENTROPY_EXECUTABLE, StringKind, Triage,
};
use game_package::map_file;
use pe_image::Image;

use crate::triage_signature::{write_library_comparison, write_signature};

const NAMES_PER_LINE: usize = 104;
const EXPORTS_SHOWN: usize = 400;

/// Every file under `game` that starts like a PE image (MZ header, a PE signature where it points), sorted with the
/// executable named by the game config first.
pub(crate) fn find_images(game: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if looks_like_pe(&path) {
                out.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(game, &mut found);
    found.sort_by_key(|p| {
        (
            p.extension().is_none_or(|e| !e.eq_ignore_ascii_case("exe")),
            p.to_string_lossy().to_ascii_lowercase(),
        )
    });
    found
}

fn looks_like_pe(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut head = [0u8; 0x40];
    if file.read_exact(&mut head).is_err() || &head[..2] != b"MZ" {
        return false;
    }
    let pe = u64::from(u32::from_le_bytes([
        head[0x3C], head[0x3D], head[0x3E], head[0x3F],
    ]));
    let mut signature = [0u8; 4];
    file.seek(SeekFrom::Start(pe)).is_ok()
        && file.read_exact(&mut signature).is_ok()
        && &signature == b"PE\0\0"
}

pub fn system_folder() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32")
}

pub fn triage_command(game: &Path, system: Option<&Path>) -> Result<ExitCode> {
    let system = system.map_or_else(system_folder, Path::to_path_buf);
    let images = find_images(game);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "Static triage of {} executable images in {}\nThe files are read as data only: nothing is run or loaded.\nReference folder for runtime-library comparison: {}\n",
        images.len(),
        game.display(),
        system.display()
    )?;
    let mut summary = Vec::new();
    for path in &images {
        let relative = path.strip_prefix(game).unwrap_or(path);
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let map = map_file(&file).with_context(|| format!("mapping {}", path.display()))?;
        let image = match Image::parse(&map) {
            Ok(image) => image,
            Err(error) => {
                writeln!(out, "== {}: not analysable: {error}\n", relative.display())?;
                continue;
            }
        };
        let triage = triage::analyse(&image);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        write_image(
            &mut out,
            &relative.display().to_string(),
            &name,
            &image,
            &triage,
            &system,
        )?;
        summary.push((relative.display().to_string(), triage.indicators.len()));
    }
    writeln!(out, "== Summary")?;
    for (name, indicators) in &summary {
        writeln!(out, "  {name:<44} {indicators} indicator line(s)")?;
    }
    Ok(ExitCode::SUCCESS)
}

fn flags(value: u32, table: &[(u32, &str)]) -> String {
    let names: Vec<&str> = table
        .iter()
        .filter(|(bit, _)| value & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(" ")
    }
}

/// `YYYY-MM-DD HH:MM:SS` UTC for seconds since 1970.
pub(crate) fn utc(seconds: u32) -> String {
    let days = i64::from(seconds / 86_400);
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn wrapped(out: &mut dyn Write, indent: &str, items: &[String], separator: &str) -> io::Result<()> {
    let mut line = String::new();
    for item in items {
        if !line.is_empty() && indent.len() + line.len() + item.len() + 2 > NAMES_PER_LINE {
            writeln!(out, "{indent}{line}")?;
            line.clear();
        }
        if !line.is_empty() {
            line.push_str(separator);
        }
        line.push_str(item);
    }
    if !line.is_empty() {
        writeln!(out, "{indent}{line}")?;
    }
    Ok(())
}

pub(crate) fn write_image(
    out: &mut dyn Write,
    relative: &str,
    name: &str,
    image: &Image<'_>,
    t: &Triage,
    system: &Path,
) -> io::Result<()> {
    writeln!(
        out,
        "================================================================"
    )?;
    writeln!(out, "== {relative} ({} bytes)", t.file_len)?;
    writeln!(
        out,
        "machine 0x{:04x}{}, time stamp 0x{:08x} ({} UTC), image base 0x{:x}, subsystem {}",
        t.machine,
        if t.machine == 0x8664 { " (x64)" } else { "" },
        t.timestamp,
        utc(t.timestamp),
        t.image_base,
        t.subsystem
    )?;
    writeln!(
        out,
        "file flags: {}; DLL flags: {}",
        flags(
            u32::from(t.characteristics),
            &[
                (0x2, "executable"),
                (0x2000, "DLL"),
                (0x20, "large-address-aware")
            ]
        ),
        flags(
            u32::from(t.dll_characteristics),
            &[
                (0x20, "high-entropy-ASLR"),
                (0x40, "ASLR"),
                (0x80, "force-integrity"),
                (0x100, "DEP"),
                (0x4000, "CFG"),
            ]
        )
    )?;
    if !t.version_info.is_empty() {
        writeln!(out, "version resource:")?;
        for (key, value) in t.version_info.iter().filter(|(_, v)| !v.is_empty()) {
            writeln!(out, "  {key}: {value}")?;
        }
    }
    writeln!(out, "sections:")?;
    for s in &t.sections {
        let mut notes = Vec::new();
        if s.executable && s.writable {
            notes.push("WRITABLE+EXECUTABLE");
        }
        if s.executable && s.entropy >= HIGH_ENTROPY_EXECUTABLE {
            notes.push("HIGH-ENTROPY CODE");
        } else if !s.executable && s.entropy >= HIGH_ENTROPY_DATA && s.raw_size >= 4096 {
            notes.push("high-entropy data");
        }
        writeln!(
            out,
            "  {:<9} rva 0x{:08x} vsize 0x{:08x} file 0x{:08x}+0x{:08x} {}{} entropy {:.3} {}",
            s.name,
            s.virtual_address,
            s.virtual_size,
            s.raw_offset,
            s.raw_size,
            if s.executable { "x" } else { "-" },
            if s.writable { "w" } else { "-" },
            s.entropy,
            notes.join(" ")
        )?;
    }
    match &t.entry_section {
        Some(section) => writeln!(out, "entry point: rva 0x{:x} in {section}", t.entry)?,
        None if t.entry == 0 => writeln!(out, "entry point: none")?,
        None => writeln!(
            out,
            "entry point: rva 0x{:x} OUTSIDE every section",
            t.entry
        )?,
    }
    write_imports(out, t)?;
    write_exports(out, t)?;
    if t.tls_callbacks.is_empty() {
        writeln!(out, "TLS callbacks: none")?;
    } else {
        let list: Vec<String> = t
            .tls_callbacks
            .iter()
            .map(|rva| {
                format!(
                    "0x{rva:x} ({})",
                    image
                        .section_of(*rva)
                        .map_or("outside sections", |s| s.name.as_str())
                )
            })
            .collect();
        writeln!(out, "TLS callbacks: {}", list.join(", "))?;
    }
    match &t.overlay {
        Some(o) => writeln!(
            out,
            "overlay: {} bytes at file 0x{:x}, entropy {:.3}",
            o.len, o.offset, o.entropy
        )?,
        None => writeln!(
            out,
            "overlay: none (nothing between the last section and the certificate or end of file)"
        )?,
    }
    writeln!(
        out,
        "embedded 'This program cannot be run in DOS mode' markers beyond the header: {}",
        t.embedded_pe_markers
    )?;
    if t.resources.is_empty() {
        writeln!(out, "resources: none")?;
    } else {
        writeln!(out, "resources:")?;
        for r in &t.resources {
            writeln!(
                out,
                "  {:<14} {} entr{}, {} bytes{}",
                r.kind,
                r.entries,
                if r.entries == 1 { "y" } else { "ies" },
                r.bytes,
                if r.starting_with_mz > 0 {
                    format!(", {} start with MZ", r.starting_with_mz)
                } else {
                    String::new()
                }
            )?;
        }
    }
    write_strings(out, t)?;
    write_signature(out, image, Some(system), name)?;
    let reference = system.join(name);
    if !system.as_os_str().is_empty()
        && name.to_ascii_lowercase().ends_with(".dll")
        && let Ok(file) = File::open(&reference)
        && let Ok(map) = map_file(&file)
        && let Ok(theirs) = Image::parse(&map)
    {
        write_library_comparison(out, image, &theirs, &reference)?;
    }
    writeln!(out, "indicators ({}):", t.indicators.len())?;
    for line in &t.indicators {
        writeln!(out, "  - {line}")?;
    }
    writeln!(out)
}

fn write_imports(out: &mut dyn Write, t: &Triage) -> io::Result<()> {
    writeln!(
        out,
        "imports: {} DLLs, {} functions",
        t.imports.len(),
        t.api_total()
    )?;
    for dll in &t.imports {
        writeln!(
            out,
            "  {}{}: {} named{}",
            dll.name,
            if dll.delayed { " (delay-loaded)" } else { "" },
            dll.names.len(),
            if dll.ordinals > 0 {
                format!(", {} by ordinal", dll.ordinals)
            } else {
                String::new()
            }
        )?;
        wrapped(out, "      ", &dll.names, ", ")?;
    }
    writeln!(out, "risky imports by category:")?;
    let mut any = false;
    for category in [
        ApiCategory::ProcessInjection,
        ApiCategory::Hook,
        ApiCategory::Persistence,
        ApiCategory::Credential,
        ApiCategory::Network,
        ApiCategory::ProcessCreation,
        ApiCategory::InputCapture,
        ApiCategory::MemoryProtection,
        ApiCategory::DynamicResolution,
        ApiCategory::AntiAnalysis,
    ] {
        let hits = t.hits_of(category);
        if hits.is_empty() {
            continue;
        }
        any = true;
        let names: Vec<String> = hits.iter().map(|h| h.name.clone()).collect();
        writeln!(out, "  {category}: {}", names.len())?;
        wrapped(out, "      ", &names, ", ")?;
    }
    if !any {
        writeln!(out, "  none")?;
    }
    Ok(())
}

fn write_exports(out: &mut dyn Write, t: &Triage) -> io::Result<()> {
    writeln!(
        out,
        "exports: {} named, {} by ordinal only, {} forwarders",
        t.exports.len(),
        t.export_ordinal_only,
        t.export_forwarders
    )?;
    wrapped(
        out,
        "    ",
        &t.exports[..t.exports.len().min(EXPORTS_SHOWN)],
        ", ",
    )?;
    if t.exports.len() > EXPORTS_SHOWN {
        writeln!(out, "    ... and {} more", t.exports.len() - EXPORTS_SHOWN)?;
    }
    Ok(())
}

fn write_strings(out: &mut dyn Write, t: &Triage) -> io::Result<()> {
    writeln!(
        out,
        "strings: {} runs of 6 or more characters scanned (ASCII and UTF-16)",
        t.strings.scanned
    )?;
    for kind in [
        StringKind::Url,
        StringKind::Ip,
        StringKind::Domain,
        StringKind::Registry,
        StringKind::Path,
        StringKind::PdbPath,
        StringKind::SourcePath,
        StringKind::CommandLine,
        StringKind::Keyword,
    ] {
        let total = t.strings.count(kind);
        let Some(found) = t.strings.found.get(&kind) else {
            writeln!(out, "  {}: none", kind.title())?;
            continue;
        };
        writeln!(
            out,
            "  {}: {total} match(es), {} distinct{}",
            kind.title(),
            found.len(),
            match t.strings.dropped.get(&kind) {
                Some(n) => format!(" (+{n} more not listed)"),
                None => String::new(),
            }
        )?;
        let limit = if matches!(kind, StringKind::SourcePath | StringKind::PdbPath) {
            5
        } else {
            60
        };
        for (text, count) in found.iter().take(limit) {
            writeln!(
                out,
                "      {text}{}",
                if *count > 1 {
                    format!("  (x{count})")
                } else {
                    String::new()
                }
            )?;
        }
        if found.len() > limit {
            writeln!(out, "      ... {} more distinct", found.len() - limit)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_pe() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x200];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C] = 0x80;
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes
    }

    #[test]
    fn walks_the_folder_for_pe_files_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.dll"), tiny_pe()).unwrap();
        std::fs::write(dir.path().join("a.exe"), tiny_pe()).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("c.bin"), tiny_pe()).unwrap();
        std::fs::write(dir.path().join("data.rpf"), b"RPF9 not a program").unwrap();
        let mut fake = tiny_pe();
        fake[0x80] = b'X';
        std::fs::write(dir.path().join("fake.dll"), fake).unwrap();
        std::fs::write(dir.path().join("short.dll"), b"MZ").unwrap();
        let names: Vec<String> = find_images(dir.path())
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["a.exe", "b.dll", "sub/c.bin"]);
    }

    #[test]
    fn formats_unix_times() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_000_000_000), "2001-09-09 01:46:40");
        assert_eq!(utc(1_709_164_800 + 86_399), "2024-02-29 23:59:59");
    }
}
