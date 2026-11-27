//! A read-only look at every file in the game folder for signs of tampering or malware: each executable's hash,
//! sections, imports of risky functions, signature block and appended data, and any file that is executable or a
//! script without looking like one. Nothing is run or loaded.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use game_package::{Manifest, map_file};
use pe_image::Image;
use sha2::{Digest, Sha256};

use crate::cli::TriageArgs;
use crate::game::resolve_game;

const RISKY: &[(&str, &str)] = &[
    ("WriteProcessMemory", "writes into other processes"),
    ("ReadProcessMemory", "reads other processes"),
    ("CreateRemoteThread", "starts threads in other processes"),
    ("CreateRemoteThreadEx", "starts threads in other processes"),
    ("NtCreateThreadEx", "starts threads in other processes"),
    ("QueueUserAPC", "queues code into other threads"),
    ("VirtualAllocEx", "allocates in other processes"),
    ("VirtualProtectEx", "changes protection in other processes"),
    ("OpenProcess", "opens other processes"),
    ("SetWindowsHookExA", "hooks input system-wide"),
    ("SetWindowsHookExW", "hooks input system-wide"),
    ("GetAsyncKeyState", "polls keys"),
    ("RegSetValueExA", "writes the registry"),
    ("RegSetValueExW", "writes the registry"),
    ("RegCreateKeyExW", "writes the registry"),
    ("CreateServiceA", "installs services"),
    ("CreateServiceW", "installs services"),
    ("ShellExecuteA", "runs programs"),
    ("ShellExecuteW", "runs programs"),
    ("ShellExecuteExW", "runs programs"),
    ("WinExec", "runs programs"),
    ("CreateProcessA", "runs programs"),
    ("CreateProcessW", "runs programs"),
    ("URLDownloadToFileA", "downloads files"),
    ("URLDownloadToFileW", "downloads files"),
    ("InternetOpenA", "web access"),
    ("InternetOpenW", "web access"),
    ("WinHttpOpen", "web access"),
    ("CryptEncrypt", "encrypts data"),
    ("BCryptEncrypt", "encrypts data"),
    ("AdjustTokenPrivileges", "changes privileges"),
    ("IsDebuggerPresent", "checks for debuggers"),
    ("CheckRemoteDebuggerPresent", "checks for debuggers"),
    ("NtSetInformationThread", "can hide threads from debuggers"),
    ("MiniDumpWriteDump", "writes memory dumps"),
];

const SCRIPT_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "hta", "lnk", "scr", "com",
    "msi", "pif", "reg", "py", "sh",
];

const PE_EXTENSIONS: &[&str] = &["exe", "dll", "sys", "efi", "ocx", "cpl"];

fn entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for b in bytes {
        counts[usize::from(*b)] += 1;
    }
    let n = bytes.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = *c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn walk(folder: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn describe_pe(image: &Image<'_>, bytes: &[u8], out: &mut String) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "  machine {:#06x}, entry {:#x} in {}, image size {:#x}, timestamp {:#x}, dll characteristics {:#06x}",
        image.machine,
        image.entry,
        image
            .section_of(image.entry)
            .map_or("no section".to_string(), |s| s.name.clone()),
        image.image_size,
        image.timestamp,
        image.dll_characteristics
    );
    let mut raw_end = 0usize;
    for section in &image.sections {
        let start = section.raw_offset as usize;
        let end = (start + section.raw_size as usize).min(bytes.len());
        raw_end = raw_end.max(end);
        let flags = format!(
            "{}{}{}",
            if section.characteristics & 0x4000_0000 != 0 {
                "R"
            } else {
                "-"
            },
            if section.writable() { "W" } else { "-" },
            if section.executable() { "X" } else { "-" }
        );
        let warn = if section.writable() && section.executable() {
            "  << writable and executable"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  section {:<8} {flags} virtual {:#10x} raw {:#10x} entropy {:.2}{warn}",
            section.name,
            section.virtual_size,
            section.raw_size,
            entropy(bytes.get(start..end).unwrap_or(&[]))
        );
    }
    let security = image.directory(4);
    let _ = writeln!(
        out,
        "  signature block: {}",
        match security {
            Some((offset, size)) => format!("{size} bytes at file offset {offset:#x}"),
            None => "none".into(),
        }
    );
    let signed_end = security.map_or(0, |(offset, size)| (offset + size) as usize);
    let overlay = bytes.len().saturating_sub(raw_end.max(signed_end));
    if overlay > 0 {
        let _ = writeln!(out, "  appended data after the image: {overlay} bytes");
    }
    if image.directory(pe_image::DIRECTORY_TLS).is_some() {
        let _ = writeln!(
            out,
            "  has thread-local storage (callbacks may run before the entry point)"
        );
    }
    let imports = image.imports();
    let total: usize = imports.iter().map(|d| d.entries.len()).sum();
    let _ = writeln!(
        out,
        "  imports: {total} functions from {} DLLs",
        imports.len()
    );
    let mut risky: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for dll in &imports {
        for entry in &dll.entries {
            if let Some(name) = &entry.name
                && let Some((_, why)) = RISKY.iter().find(|(n, _)| n == name)
            {
                risky
                    .entry(why)
                    .or_default()
                    .push(format!("{}!{name}", dll.name));
            }
        }
    }
    for (why, names) in &risky {
        let _ = writeln!(out, "  risky import ({why}): {}", names.join(", "));
    }
    let _ = writeln!(out, "  exports: {}", image.exports().len());
}

pub fn triage_command(args: &TriageArgs) -> Result<ExitCode> {
    use std::fmt::Write;
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let mut files = Vec::new();
    walk(&game, &mut files).with_context(|| format!("listing {}", game.display()))?;
    files.sort();
    let mut out = String::new();
    let mut pe_count = 0;
    let mut flagged = 0;
    for path in &files {
        let relative = path
            .strip_prefix(&game)
            .unwrap_or(path)
            .display()
            .to_string();
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let file = File::open(path).with_context(|| format!("opening {relative}"))?;
        let size = file.metadata()?.len();
        let pe_named = PE_EXTENSIONS.contains(&extension.as_str());
        let mut head = [0u8; 2];
        let looks_pe = size >= 2 && {
            use std::io::Read;
            (&file).read_exact(&mut head).is_ok() && &head == b"MZ"
        };
        if looks_pe || pe_named {
            pe_count += 1;
            let bytes = map_file(&file).with_context(|| format!("mapping {relative}"))?;
            let hash = Sha256::digest(&bytes[..]);
            let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            let _ = writeln!(out, "[{pe_count}] {relative}  {size} bytes  sha256 {hex}");
            if looks_pe && !pe_named {
                flagged += 1;
                let _ = writeln!(out, "  << executable content under a non-executable name");
            }
            match Image::parse(&bytes) {
                Ok(image) => describe_pe(&image, &bytes, &mut out),
                Err(error) => {
                    let _ = writeln!(out, "  not a readable 64-bit PE: {error}");
                }
            }
        } else if SCRIPT_EXTENSIONS.contains(&extension.as_str()) {
            flagged += 1;
            let _ = writeln!(
                out,
                "[script] {relative}  {size} bytes  << script or shortcut file"
            );
        }
    }
    let _ = writeln!(
        out,
        "\n{} files, {pe_count} executables, {flagged} flagged as executable or script content in an unexpected place",
        files.len()
    );
    match &args.out {
        Some(path) => {
            std::fs::write(path, &out).with_context(|| format!("writing {}", path.display()))?
        }
        None => print!("{out}"),
    }
    Ok(ExitCode::SUCCESS)
}
