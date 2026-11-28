//! Authenticode checks for `leonida exe triage`: does the signed digest match the file, and where does the file
//! differ from a signed original. Only hashes, offsets and counts are reported.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use game_package::map_file;
use pe_image::Image;
use pe_image::authenticode::{
    self, DigestAlgorithm, Region, certificate_table, checksum, sha256, signature, stored_checksum,
};

/// Files above this size skip the hash-convention experiments, which copy the file.
const EXPERIMENT_LIMIT: usize = 64 << 20;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy)]
struct Convention {
    skip_checksum: bool,
    skip_certificate_entry: bool,
    table_order: bool,
    trailing: Trailing,
    header_end: Option<usize>,
}

#[derive(Clone, Copy)]
enum Trailing {
    BeforeCertificate,
    None,
    ThroughEndOfFile,
}

fn regions_for(image: &Image<'_>, how: &Convention) -> Vec<Region> {
    let len = image.bytes().len();
    let checksum_at = image.checksum_offset();
    let entry_at = image.directory_entry_offset(pe_image::DIRECTORY_SECURITY);
    let headers = how
        .header_end
        .unwrap_or(image.headers_size as usize)
        .min(len);
    let mut skips: Vec<(usize, usize)> = Vec::new();
    if how.skip_checksum {
        skips.push((checksum_at, checksum_at + 4));
    }
    if how.skip_certificate_entry {
        skips.push((entry_at, entry_at + 8));
    }
    let mut out = Vec::new();
    let mut at = 0;
    for (start, end) in skips {
        if start > at {
            out.push(range("headers", at, start));
        }
        at = end;
    }
    if headers > at {
        out.push(range("headers", at, headers));
    }
    let mut sections: Vec<_> = image
        .sections
        .iter()
        .filter(|s| s.raw_size != 0)
        .map(|s| (s.raw_offset as usize, (s.raw_offset + s.raw_size) as usize))
        .collect();
    if !how.table_order {
        sections.sort();
    }
    let mut covered = headers;
    for (start, end) in sections {
        out.push(range("section", start, end.min(len)));
        covered = covered.max(end.min(len));
    }
    let certificate = certificate_table(image);
    match how.trailing {
        Trailing::None => {}
        Trailing::ThroughEndOfFile => out.push(range("trailer", covered, len)),
        Trailing::BeforeCertificate => {
            let end = certificate.map_or(len, |(offset, _)| offset);
            out.push(range("trailer", covered, end));
            if let Some((offset, size)) = certificate {
                out.push(range("after certificate", offset + size, len));
            }
        }
    }
    out
}

fn range(label: &str, start: usize, end: usize) -> Region {
    Region {
        label: label.to_string(),
        start,
        end: end.max(start),
    }
}

/// Ways the file could have been hashed other than the documented one, each with the digest it gives.
fn experiments(image: &Image<'_>) -> Vec<(String, [u8; 32])> {
    let standard = Convention {
        skip_checksum: true,
        skip_certificate_entry: true,
        table_order: false,
        trailing: Trailing::BeforeCertificate,
        header_end: None,
    };
    let bytes = image.bytes();
    let with = |change: &dyn Fn(&mut Convention)| {
        let mut how = standard;
        change(&mut how);
        sha256(bytes, &regions_for(image, &how))
    };
    let first_raw = image
        .sections
        .iter()
        .filter(|s| s.raw_size != 0)
        .map(|s| s.raw_offset as usize)
        .min();
    let mut out = vec![
        ("documented ranges".to_string(), with(&|_| {})),
        (
            "checksum field included".to_string(),
            with(&|h| h.skip_checksum = false),
        ),
        (
            "certificate directory entry included".to_string(),
            with(&|h| h.skip_certificate_entry = false),
        ),
        (
            "checksum and certificate entry both included".to_string(),
            with(&|h| {
                h.skip_checksum = false;
                h.skip_certificate_entry = false;
            }),
        ),
        (
            "sections in table order".to_string(),
            with(&|h| h.table_order = true),
        ),
        (
            "data after the last section left out".to_string(),
            with(&|h| h.trailing = Trailing::None),
        ),
        (
            "data after the last section runs through the certificate".to_string(),
            with(&|h| h.trailing = Trailing::ThroughEndOfFile),
        ),
    ];
    if let Some(first) = first_raw {
        out.push((
            "headers end at the first section's file offset".to_string(),
            with(&|h| h.header_end = Some(first)),
        ));
    }
    if bytes.len() <= EXPERIMENT_LIMIT {
        let timestamp_at = image.checksum_offset() - 80;
        let mut candidates = vec![0u32];
        for entry in debug_entries(image) {
            if !candidates.contains(&entry.timestamp) && entry.timestamp != image.timestamp {
                candidates.push(entry.timestamp);
            }
        }
        for value in candidates {
            let mut copy = bytes.to_vec();
            if let Some(field) = copy.get_mut(timestamp_at..timestamp_at + 4) {
                field.copy_from_slice(&value.to_le_bytes());
            }
            if let Ok(patched) = Image::parse(&copy) {
                out.push((
                    format!("header time stamp replaced by 0x{value:08x}"),
                    sha256(&copy, &regions_for(&patched, &standard)),
                ));
            }
        }
    }
    out
}

pub struct DebugEntry {
    pub kind: u32,
    pub timestamp: u32,
    pub size: u32,
    pub pointer: u32,
}

pub fn debug_entries(image: &Image<'_>) -> Vec<DebugEntry> {
    let Some((rva, size)) = image.directory(6) else {
        return Vec::new();
    };
    (0..(size / 28).min(32))
        .filter_map(|index| {
            let raw = image.read(rva + 28 * index, 28)?;
            let field =
                |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap_or([0; 4]));
            Some(DebugEntry {
                timestamp: field(4),
                kind: field(12),
                size: field(16),
                pointer: field(24),
            })
        })
        .collect()
}

fn debug_kind(kind: u32) -> &'static str {
    match kind {
        1 => "COFF",
        2 => "CodeView",
        3 => "FPO",
        4 => "MISC",
        9 => "BORLAND",
        10 => "RESERVED10",
        11 => "CLSID",
        12 => "VC_FEATURE",
        13 => "POGO",
        14 => "ILTCG",
        15 => "MPX",
        16 => "REPRO",
        20 => "EX_DLLCHARACTERISTICS",
        _ => "other",
    }
}

pub fn write_debug_directory(out: &mut dyn Write, image: &Image<'_>) -> io::Result<()> {
    let entries = debug_entries(image);
    if entries.is_empty() {
        return writeln!(out, "debug directory: none");
    }
    writeln!(
        out,
        "debug directory ({} entries, header time stamp 0x{:08x}):",
        entries.len(),
        image.timestamp
    )?;
    for e in &entries {
        let mut extra = String::new();
        if e.kind == 2 {
            let at = e.pointer as usize;
            if let Some(raw) = image.bytes().get(at..at + (e.size as usize).min(512))
                && raw.get(..4) == Some(b"RSDS")
                && raw.len() > 24
            {
                let age = u32::from_le_bytes(raw[20..24].try_into().unwrap_or([0; 4]));
                let path = raw[24..].split(|b| *b == 0).next().unwrap_or(&[]);
                extra = format!(" age {age} pdb {}", String::from_utf8_lossy(path));
            }
        }
        writeln!(
            out,
            "  {:<22} time stamp 0x{:08x}{} size {}{extra}",
            debug_kind(e.kind),
            e.timestamp,
            if e.timestamp == image.timestamp {
                " (= header)"
            } else {
                " (differs from header)"
            },
            e.size
        )?;
    }
    Ok(())
}

/// Which header field a file offset in the first `size_of_headers` bytes belongs to.
pub fn header_field(image: &Image<'_>, offset: usize) -> String {
    let pe = u32::from_le_bytes(image.bytes()[0x3C..0x40].try_into().unwrap_or([0; 4])) as usize;
    let optional = pe + 24;
    let table = optional + 240;
    match offset {
        o if o < 0x40 => "DOS header".to_string(),
        o if o < pe => "DOS stub".to_string(),
        o if o < pe + 4 => "PE signature".to_string(),
        o if (pe + 8..pe + 12).contains(&o) => "COFF time stamp".to_string(),
        o if o < optional => "COFF header".to_string(),
        o if (optional + 16..optional + 20).contains(&o) => "entry point".to_string(),
        o if (optional + 56..optional + 60).contains(&o) => "size of image".to_string(),
        o if (optional + 64..optional + 68).contains(&o) => "checksum".to_string(),
        o if (optional + 70..optional + 72).contains(&o) => "DLL characteristics".to_string(),
        o if (optional + 112..optional + 112 + 128).contains(&o) => {
            format!("data directory {}", (o - optional - 112) / 8)
        }
        o if o < table => "optional header".to_string(),
        o if o < table + 40 * image.sections.len() => {
            format!("section table entry {}", (o - table) / 40)
        }
        _ => "padding after the section table".to_string(),
    }
}

struct Difference {
    differing: usize,
    runs: usize,
    first: usize,
    last: usize,
    sample: Vec<usize>,
}

fn difference(a: &[u8], b: &[u8]) -> Difference {
    let mut d = Difference {
        differing: 0,
        runs: 0,
        first: 0,
        last: 0,
        sample: Vec::new(),
    };
    let mut previous = None;
    for (index, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y {
            if d.differing == 0 {
                d.first = index;
            }
            d.differing += 1;
            d.last = index;
            if previous != Some(index.wrapping_sub(1)) {
                d.runs += 1;
            }
            if d.sample.len() < 16 && !d.sample.last().is_some_and(|l| index - l < 4) {
                d.sample.push(index);
            }
            previous = Some(index);
        }
    }
    d
}

pub fn write_signature(
    out: &mut dyn Write,
    image: &Image<'_>,
    refs: &References,
    name: &str,
) -> io::Result<()> {
    let bytes = image.bytes();
    let stored = stored_checksum(image).unwrap_or(0);
    let computed = checksum(bytes, image.checksum_offset());
    writeln!(
        out,
        "pe checksum: stored 0x{stored:08x}, computed 0x{computed:08x} ({})",
        match (stored, stored == computed) {
            (0, _) => "not set",
            (_, true) => "matches",
            _ => "differs",
        }
    )?;
    let Some(signature) = signature(image) else {
        return writeln!(out, "authenticode: no certificate table (unsigned)");
    };
    let table = certificate_table(image).unwrap_or((0, 0));
    writeln!(
        out,
        "authenticode: certificate table at file 0x{:x}, {} bytes ({} bytes after it), revision 0x{:04x}, type {}",
        table.0,
        table.1,
        bytes.len() - table.0 - table.1,
        signature.revision,
        signature.certificate_type
    )?;
    writeln!(
        out,
        "  names in the signature: {}",
        signature.names.join("; ")
    )?;
    writeln!(
        out,
        "  signing times in the signature: {}",
        signature.signing_times.join("; ")
    )?;
    let regions = authenticode::regions(image);
    let computed_digest = sha256(bytes, &regions);
    match &signature.algorithm {
        Some(DigestAlgorithm::Sha256) => {
            let matches = computed_digest.as_slice() == signature.digest;
            writeln!(out, "  digest algorithm: SHA-256")?;
            writeln!(out, "  signed digest:   {}", hex(&signature.digest))?;
            writeln!(out, "  computed digest: {}", hex(&computed_digest))?;
            writeln!(
                out,
                "  result: {}",
                if matches {
                    "MATCH, the file is as signed"
                } else {
                    "MISMATCH, the hashed ranges differ from what was signed"
                }
            )?;
            if !matches {
                write_mismatch_analysis(out, image, &signature.digest, refs, name)?;
            }
        }
        other => writeln!(
            out,
            "  digest algorithm {other:?} (stored digest {}): only SHA-256 is recomputed here",
            hex(&signature.digest)
        )?,
    }
    Ok(())
}

/// How the signature's per-page hashes compare with the file: pages checked, and the file offsets that differ.
fn page_check(image: &Image<'_>, stored: &authenticode::PageHashes) -> Option<(usize, Vec<u32>)> {
    if stored.algorithm != DigestAlgorithm::Sha256 {
        return None;
    }
    let ours = authenticode::page_hashes(image);
    let mut bad = Vec::new();
    for (offset, hash) in &ours {
        match stored.entries.iter().find(|(o, _)| o == offset) {
            Some((_, wanted)) if wanted.as_slice() == hash.as_slice() => {}
            _ => bad.push(*offset),
        }
    }
    Some((ours.len(), bad))
}

fn write_page_hashes(
    out: &mut dyn Write,
    image: &Image<'_>,
    refs: &References,
    name: &str,
) -> io::Result<()> {
    let Some(signature) = signature(image) else {
        return Ok(());
    };
    let Some(stored) = &signature.page_hashes else {
        return writeln!(out, "  page hashes: the signature carries none");
    };
    writeln!(
        out,
        "  page hashes: {} stored entries ({:?})",
        stored.entries.len(),
        stored.algorithm
    )?;
    match page_check(image, stored) {
        None => writeln!(out, "    not recomputed (not SHA-256)")?,
        Some((total, bad)) => {
            writeln!(
                out,
                "    {} of {total} pages differ from the stored page hashes",
                bad.len()
            )?;
            let mut named = Vec::new();
            for offset in bad.iter().take(60) {
                let place = image
                    .sections
                    .iter()
                    .find(|s| *offset >= s.raw_offset && *offset < s.raw_offset + s.raw_size)
                    .map_or(
                        if *offset == 0 {
                            "headers".to_string()
                        } else {
                            "after sections".to_string()
                        },
                        |s| s.name.clone(),
                    );
                named.push(format!("0x{offset:x} ({place})"));
            }
            writeln!(
                out,
                "    differing pages: {}{}",
                named.join(", "),
                if bad.len() > 60 { ", ..." } else { "" }
            )?;
        }
    }
    let reference = refs.system.join(name);
    if let Ok(map) = File::open(&reference).and_then(|f| map_file(&f))
        && let Ok(other) = Image::parse(&map)
        && let Some(sig) = signature_of(&other)
        && let Some(stored) = &sig.page_hashes
        && let Some((total, bad)) = page_check(&other, stored)
    {
        writeln!(
            out,
            "    method check on {}: {} of {total} pages differ from its own stored page hashes",
            reference.display(),
            bad.len()
        )?;
    }
    Ok(())
}

fn signature_of(image: &Image<'_>) -> Option<authenticode::Signature> {
    signature(image)
}

fn write_mismatch_analysis(
    out: &mut dyn Write,
    image: &Image<'_>,
    signed: &[u8],
    refs: &References,
    name: &str,
) -> io::Result<()> {
    write_page_hashes(out, image, refs, name)?;
    writeln!(out, "  hashed ranges:")?;
    for region in authenticode::regions(image) {
        writeln!(
            out,
            "    {:<42} 0x{:08x}..0x{:08x} ({} bytes)",
            region.label,
            region.start,
            region.end,
            region.end - region.start
        )?;
    }
    writeln!(
        out,
        "  other ways of hashing the same file, and whether any gives the signed digest:"
    )?;
    for (label, digest) in experiments(image) {
        writeln!(
            out,
            "    {:<56} {}",
            label,
            if digest.as_slice() == signed {
                "MATCHES"
            } else {
                "no"
            }
        )?;
    }
    let system = &refs.system;
    let reference = system.join(name);
    match File::open(&reference).and_then(|f| map_file(&f)) {
        Err(_) => writeln!(out, "  no {name} in {} to compare with", system.display())?,
        Ok(map) => match Image::parse(&map) {
            Ok(other) => write_reference_diff(out, image, &other, signed, &reference)?,
            Err(_) => writeln!(
                out,
                "  the copy in {} is not a 64-bit PE image",
                system.display()
            )?,
        },
    }
    write_other_copies(out, image, signed, refs, name, &reference)
}

fn write_other_copies(
    out: &mut dyn Write,
    image: &Image<'_>,
    signed: &[u8],
    refs: &References,
    name: &str,
    system_copy: &Path,
) -> io::Result<()> {
    let Some(paths) = refs.found.get(&name.to_ascii_lowercase()) else {
        return Ok(());
    };
    let mut listed = Vec::new();
    let mut identical = 0;
    for path in paths {
        if path.as_path() == system_copy {
            continue;
        }
        let Ok(map) = File::open(path).and_then(|f| map_file(&f)) else {
            continue;
        };
        if map[..] == *image.bytes() {
            identical += 1;
            continue;
        }
        let Ok(other) = Image::parse(&map) else {
            continue;
        };
        let their = signature(&other);
        let same_digest = their.as_ref().is_some_and(|s| s.digest == signed);
        let verifies = their
            .as_ref()
            .is_some_and(|s| s.digest == sha256(other.bytes(), &authenticode::regions(&other)));
        listed.push((
            same_digest,
            path.clone(),
            other.bytes().len(),
            other.timestamp,
            verifies,
        ));
    }
    listed.sort_by_key(|l| (!l.0, l.1.clone()));
    writeln!(
        out,
        "  other copies of {name} on this PC: {} different, {identical} byte-identical to this one",
        listed.len()
    )?;
    for (same_digest, path, size, timestamp, verifies) in listed.iter().take(30) {
        writeln!(
            out,
            "    {} ({size} bytes, time stamp 0x{timestamp:08x} {}): {}; own signature {}",
            path.display(),
            crate::cmd_triage::utc(*timestamp),
            if *same_digest {
                "SAME SIGNED DIGEST AS OURS"
            } else {
                "other signed digest"
            },
            if *verifies {
                "verifies"
            } else {
                "does not verify"
            },
        )?;
        if (*same_digest || *timestamp == image.timestamp)
            && let Ok(map) = File::open(path).and_then(|f| map_file(&f))
            && let Ok(other) = Image::parse(&map)
        {
            write_reference_diff(out, image, &other, signed, path)?;
        }
    }
    Ok(())
}

/// Other places that hold a library of the same name, found by walking some folders.
pub struct References {
    pub system: PathBuf,
    pub found: HashMap<String, Vec<PathBuf>>,
}

/// Every file under `roots` (at most 14 folders down) whose lowercase name is in `wanted`.
pub fn index_names(roots: &[PathBuf], wanted: &HashSet<String>) -> HashMap<String, Vec<PathBuf>> {
    fn visit(
        dir: &Path,
        depth: usize,
        wanted: &HashSet<String>,
        out: &mut HashMap<String, Vec<PathBuf>>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth > 0 {
                    visit(&entry.path(), depth - 1, wanted, out);
                }
            } else {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if wanted.contains(&name) {
                    out.entry(name).or_default().push(entry.path());
                }
            }
        }
    }
    let mut out = HashMap::new();
    for root in roots {
        visit(root, 14, wanted, &mut out);
    }
    for list in out.values_mut() {
        list.sort();
        list.dedup();
    }
    out
}

fn write_reference_diff(
    out: &mut dyn Write,
    image: &Image<'_>,
    other: &Image<'_>,
    signed: &[u8],
    reference: &Path,
) -> io::Result<()> {
    writeln!(out, "  against {}:", reference.display())?;
    let their = signature(other);
    let their_computed = sha256(other.bytes(), &authenticode::regions(other));
    let same_signed_original = their.as_ref().is_some_and(|s| s.digest == signed);
    writeln!(
        out,
        "    that file: {} bytes, time stamp 0x{:08x} (ours 0x{:08x}), {}",
        other.bytes().len(),
        other.timestamp,
        image.timestamp,
        match &their {
            None => "unsigned (catalog-signed or unsigned)".to_string(),
            Some(s) if s.digest == their_computed => "its own signature verifies".to_string(),
            Some(_) => "its own signature does not verify by this check".to_string(),
        }
    )?;
    writeln!(
        out,
        "    signed digest equals ours: {}",
        if same_signed_original {
            "yes, so that file is the signed original of ours"
        } else {
            "no, it is another build of the library or another signing"
        }
    )?;
    let ours = authenticode::regions(image);
    let theirs = authenticode::regions(other);
    let comparable = ours.len() == theirs.len()
        && ours
            .iter()
            .zip(&theirs)
            .all(|(a, b)| a.end - a.start == b.end - b.start);
    if !comparable {
        return writeln!(
            out,
            "    the layouts differ (section count or sizes), so the files are compared by structure only"
        );
    }
    writeln!(out, "    byte differences per hashed range (same layout):")?;
    for (a, b) in ours.iter().zip(&theirs) {
        let d = difference(
            &image.bytes()[a.start..a.end],
            &other.bytes()[b.start..b.end],
        );
        if d.differing == 0 {
            writeln!(out, "      {:<42} identical", a.label)?;
            continue;
        }
        writeln!(
            out,
            "      {:<42} {} of {} bytes differ in {} run(s), from +0x{:x} to +0x{:x}",
            a.label,
            d.differing,
            a.end - a.start,
            d.runs,
            d.first,
            d.last
        )?;
        if a.label.starts_with("headers") {
            let fields: Vec<String> = d
                .sample
                .iter()
                .map(|o| header_field(image, a.start + o))
                .collect();
            writeln!(
                out,
                "        header fields touched: {}",
                dedupe(fields).join(", ")
            )?;
        }
    }
    Ok(())
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

/// How two images relate as libraries: exports and section layout.
pub fn write_library_comparison(
    out: &mut dyn Write,
    ours: &Image<'_>,
    theirs: &Image<'_>,
    reference: &Path,
) -> io::Result<()> {
    let names = |image: &Image<'_>| -> std::collections::BTreeSet<String> {
        image.exports().into_iter().filter_map(|e| e.name).collect()
    };
    let (a, b) = (names(ours), names(theirs));
    let both = a.intersection(&b).count();
    let only_ours: Vec<&String> = a.difference(&b).collect();
    let only_theirs: Vec<&String> = b.difference(&a).collect();
    writeln!(
        out,
        "compared with {}: exports {} here, {} there, {} in common",
        reference.display(),
        a.len(),
        b.len(),
        both
    )?;
    let show = |list: &[&String]| {
        let shown: Vec<&str> = list.iter().take(12).map(|s| s.as_str()).collect();
        format!(
            "{}{}",
            shown.join(", "),
            if list.len() > 12 { ", ..." } else { "" }
        )
    };
    if !only_ours.is_empty() {
        writeln!(
            out,
            "  only here ({}): {}",
            only_ours.len(),
            show(&only_ours)
        )?;
    }
    if !only_theirs.is_empty() {
        writeln!(
            out,
            "  only there ({}): {}",
            only_theirs.len(),
            show(&only_theirs)
        )?;
    }
    let layout = |image: &Image<'_>| -> Vec<String> {
        image
            .sections
            .iter()
            .map(|s| format!("{}:{:#x}", s.name, s.virtual_size))
            .collect()
    };
    writeln!(out, "  sections here:  {}", layout(ours).join(" "))?;
    writeln!(out, "  sections there: {}", layout(theirs).join(" "))?;
    let same_names = ours
        .sections
        .iter()
        .map(|s| s.name.as_str())
        .eq(theirs.sections.iter().map(|s| s.name.as_str()));
    writeln!(
        out,
        "  same section names in the same order: {}; machine {:#06x} vs {:#06x}",
        if same_names { "yes" } else { "no" },
        ours.machine,
        theirs.machine
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_are_counted_in_runs() {
        let a = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let mut b = a;
        b[2] = 0xFF;
        b[3] = 0xFF;
        b[8] = 0xFF;
        let d = difference(&a, &b);
        assert_eq!((d.differing, d.runs, d.first, d.last), (3, 2, 2, 8));
        assert_eq!(difference(&a, &a).differing, 0);
    }

    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(&[0, 0xAB, 0x0F]), "00ab0f");
    }
}
