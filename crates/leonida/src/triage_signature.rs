//! Authenticode checks for `leonida exe triage`: does the signed digest match the file, and where does the file
//! differ from a signed original. Only hashes, offsets and counts are reported.

use std::io::{self, Write};
use std::path::Path;

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
fn experiments(image: &Image<'_>) -> Vec<(&'static str, [u8; 32])> {
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
        ("documented ranges", with(&|_| {})),
        (
            "checksum field included",
            with(&|h| h.skip_checksum = false),
        ),
        (
            "certificate directory entry included",
            with(&|h| h.skip_certificate_entry = false),
        ),
        (
            "checksum and certificate entry both included",
            with(&|h| {
                h.skip_checksum = false;
                h.skip_certificate_entry = false;
            }),
        ),
        ("sections in table order", with(&|h| h.table_order = true)),
        (
            "data after the last section left out",
            with(&|h| h.trailing = Trailing::None),
        ),
        (
            "data after the last section runs through the certificate",
            with(&|h| h.trailing = Trailing::ThroughEndOfFile),
        ),
    ];
    if let Some(first) = first_raw {
        out.push((
            "headers end at the first section's file offset",
            with(&|h| h.header_end = Some(first)),
        ));
    }
    if bytes.len() <= EXPERIMENT_LIMIT {
        let mut copy = bytes.to_vec();
        let timestamp = image.checksum_offset() - 80;
        if let Some(field) = copy.get_mut(timestamp..timestamp + 4) {
            field.fill(0);
        }
        if let Ok(patched) = Image::parse(&copy) {
            out.push((
                "header timestamp zeroed",
                sha256(&copy, &regions_for(&patched, &standard)),
            ));
        }
    }
    out
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
    system: Option<&Path>,
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
                write_mismatch_analysis(out, image, &signature.digest, system, name)?;
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

fn write_mismatch_analysis(
    out: &mut dyn Write,
    image: &Image<'_>,
    signed: &[u8],
    system: Option<&Path>,
    name: &str,
) -> io::Result<()> {
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
    let Some(system) = system else { return Ok(()) };
    let reference = system.join(name);
    let Ok(file) = std::fs::File::open(&reference) else {
        return writeln!(out, "  no {name} in {} to compare with", system.display());
    };
    let Ok(map) = map_file(&file) else {
        return Ok(());
    };
    let Ok(other) = Image::parse(&map) else {
        return writeln!(
            out,
            "  the copy in {} is not a 64-bit PE image",
            system.display()
        );
    };
    write_reference_diff(out, image, &other, signed, &reference)
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
