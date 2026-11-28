//! Authenticode: the byte ranges of a PE file that its signature hashes, the PE checksum, and the digest a
//! signature stores (read out of the PKCS#7 blob far enough to find it).

use sha2::{Digest, Sha256};

use crate::{DIRECTORY_SECURITY, Image};

/// A byte range of the file that the Authenticode hash covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

/// Where the certificate table is: a file offset (not an RVA) and its size. `None` when the file is unsigned or the
/// entry points outside the file.
pub fn certificate_table(image: &Image<'_>) -> Option<(usize, usize)> {
    let (offset, size) = *image.directories.get(DIRECTORY_SECURITY)?;
    let (offset, size) = (offset as usize, size as usize);
    (offset != 0 && size != 0 && offset.checked_add(size)? <= image.bytes().len())
        .then_some((offset, size))
}

/// The ranges the signature hash covers, in hashing order: the headers without the checksum field and the
/// certificate-table entry, every section's raw data by file offset, then whatever follows the last section except
/// the certificate table itself.
pub fn regions(image: &Image<'_>) -> Vec<Region> {
    let len = image.bytes().len();
    let checksum = image.checksum_offset();
    let entry = image.directory_entry_offset(DIRECTORY_SECURITY);
    let headers = (image.headers_size as usize).min(len);
    let mut out = Vec::new();
    let mut push = |label: &str, start: usize, end: usize| {
        let end = end.min(len);
        if start < end {
            out.push(Region {
                label: label.to_string(),
                start,
                end,
            });
        }
    };
    push("headers before checksum", 0, checksum);
    if image.directories.len() > DIRECTORY_SECURITY {
        push("headers up to certificate entry", checksum + 4, entry);
        push("headers after certificate entry", entry + 8, headers);
    } else {
        push("headers after checksum", checksum + 4, headers);
    }
    let mut sections: Vec<_> = image
        .sections
        .iter()
        .filter(|s| s.raw_size != 0)
        .map(|s| {
            (
                s.raw_offset as usize,
                (s.raw_offset as usize).saturating_add(s.raw_size as usize),
                s.name.clone(),
            )
        })
        .collect();
    sections.sort_by_key(|s| s.0);
    let mut covered = headers;
    for (start, end, name) in &sections {
        push(&format!("section {name}"), *start, *end);
        covered = covered.max((*end).min(len));
    }
    match certificate_table(image) {
        Some((offset, size)) => {
            push("data between sections and certificate", covered, offset);
            push("data after certificate", offset + size, len);
        }
        None => push("data after sections", covered, len),
    }
    out
}

/// SHA-256 over the given ranges, as the Authenticode image hash is built.
pub fn sha256(bytes: &[u8], regions: &[Region]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for region in regions {
        if let Some(part) = bytes.get(region.start..region.end) {
            hasher.update(part);
        }
    }
    hasher.finalize().into()
}

/// The PE checksum (the algorithm of `CheckSumMappedFile`) with the file's own checksum field counted as zero.
pub fn checksum(bytes: &[u8], checksum_offset: usize) -> u32 {
    let mut sum: u64 = 0;
    for (index, chunk) in bytes.chunks(2).enumerate() {
        let at = index * 2;
        if at >= checksum_offset && at < checksum_offset + 4 {
            continue;
        }
        let word = u64::from(chunk[0]) | chunk.get(1).map_or(0, |b| u64::from(*b) << 8);
        sum += word;
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum = (sum & 0xFFFF) + (sum >> 16);
    (sum + bytes.len() as u64) as u32
}

pub fn stored_checksum(image: &Image<'_>) -> Option<u32> {
    let at = image.checksum_offset();
    Some(u32::from_le_bytes(
        image.bytes().get(at..at + 4)?.try_into().ok()?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Sha1,
    Sha256,
    Md5,
    Other(String),
}

impl DigestAlgorithm {
    fn from_oid(oid: &str) -> Self {
        match oid {
            "1.3.14.3.2.26" => Self::Sha1,
            "2.16.840.1.101.3.4.2.1" => Self::Sha256,
            "1.2.840.113549.2.5" => Self::Md5,
            other => Self::Other(other.to_string()),
        }
    }
}

/// What a signature says, as far as reading it without checking any certificate chain goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub revision: u16,
    pub certificate_type: u16,
    /// Size of the whole certificate table.
    pub table_size: usize,
    pub algorithm: Option<DigestAlgorithm>,
    /// The file digest stored in the signed SpcIndirectData.
    pub digest: Vec<u8>,
    /// Common names and organisations in the signature's certificates, in order, without repeats.
    pub names: Vec<String>,
}

struct Tlv<'a> {
    tag: u8,
    content: &'a [u8],
    rest: &'a [u8],
}

fn tlv(data: &[u8]) -> Option<Tlv<'_>> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (len, header) = match first {
        0x80 => (data.len().saturating_sub(2), 2),
        0..=0x7F => (usize::from(first), 2),
        count @ 0x81..=0x84 => {
            let count = usize::from(count & 0x7F);
            let bytes = data.get(2..2 + count)?;
            (
                bytes.iter().fold(0usize, |a, b| a << 8 | usize::from(*b)),
                2 + count,
            )
        }
        _ => return None,
    };
    let content = data.get(header..header.checked_add(len)?)?;
    Some(Tlv {
        tag,
        content,
        rest: &data[header + len..],
    })
}

fn oid_text(bytes: &[u8]) -> String {
    let mut parts: Vec<u64> = Vec::new();
    let mut value: u64 = 0;
    for byte in bytes {
        value = value << 7 | u64::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            if parts.is_empty() {
                let first = (value / 40).min(2);
                parts.push(first);
                parts.push(value - first * 40);
            } else {
                parts.push(value);
            }
            value = 0;
        }
    }
    parts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

const SPC_INDIRECT_DATA: &str = "1.3.6.1.4.1.311.2.1.4";

fn find_digest(blob: &[u8]) -> Option<(DigestAlgorithm, Vec<u8>)> {
    let content_info = tlv(blob)?;
    let mut inner = tlv(content_info.content)?;
    if inner.tag == 0x06 {
        inner = tlv(inner.rest)?;
    }
    let signed_data = tlv(inner.content)?;
    let version = tlv(signed_data.content)?;
    let algorithms = tlv(version.rest)?;
    let encapsulated = tlv(algorithms.rest)?;
    let oid = tlv(encapsulated.content)?;
    if oid_text(oid.content) != SPC_INDIRECT_DATA {
        return None;
    }
    let explicit = tlv(oid.rest)?;
    let indirect = tlv(explicit.content)?;
    let data = tlv(indirect.content)?;
    let digest_info = tlv(data.rest)?;
    let algorithm = tlv(digest_info.content)?;
    let algorithm_oid = tlv(algorithm.content)?;
    let digest = tlv(algorithm.rest)?;
    (digest.tag == 0x04).then(|| {
        (
            DigestAlgorithm::from_oid(&oid_text(algorithm_oid.content)),
            digest.content.to_vec(),
        )
    })
}

fn certificate_names(blob: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for attribute in [
        [0x06, 0x03, 0x55, 0x04, 0x03],
        [0x06, 0x03, 0x55, 0x04, 0x0A],
    ] {
        let mut at = 0;
        while let Some(found) = blob[at..].windows(5).position(|w| w == attribute) {
            let start = at + found + 5;
            at = start;
            let Some(value) = tlv(&blob[start..]) else {
                continue;
            };
            let text = match value.tag {
                0x0C | 0x13 | 0x14 | 0x16 => String::from_utf8_lossy(value.content).into_owned(),
                0x1E => String::from_utf16_lossy(
                    &value
                        .content
                        .chunks_exact(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                ),
                _ => continue,
            };
            if !text.is_empty() && text.len() < 120 && !names.contains(&text) {
                names.push(text);
            }
        }
    }
    names
}

/// The signature in the certificate table, if there is one.
pub fn signature(image: &Image<'_>) -> Option<Signature> {
    let (offset, size) = certificate_table(image)?;
    let table = image.bytes().get(offset..offset + size)?;
    let length = u32::from_le_bytes(table.get(..4)?.try_into().ok()?) as usize;
    let revision = u16::from_le_bytes(table.get(4..6)?.try_into().ok()?);
    let certificate_type = u16::from_le_bytes(table.get(6..8)?.try_into().ok()?);
    let blob = table.get(8..length.clamp(8, size))?;
    let found = find_digest(blob);
    Some(Signature {
        revision,
        certificate_type,
        table_size: size,
        algorithm: found.as_ref().map(|f| f.0.clone()),
        digest: found.map(|f| f.1).unwrap_or_default(),
        names: certificate_names(blob),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 0x80 {
            out.push(content.len() as u8);
        } else {
            out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
        }
        out.extend(content);
        out
    }

    fn signature_blob(digest: &[u8; 32]) -> Vec<u8> {
        let sha256 = der(
            0x30,
            &[
                der(0x06, &[0x60, 0x86, 0x48, 0x01, 0x65, 3, 4, 2, 1]),
                der(0x05, &[]),
            ]
            .concat(),
        );
        let digest_info = der(0x30, &[sha256.clone(), der(0x04, digest)].concat());
        let spc_data = der(0x30, &der(0x06, &[0x2B, 6, 1, 4, 1, 0x82, 0x37, 2, 1, 15]));
        let indirect = der(0x30, &[spc_data, digest_info].concat());
        let encapsulated = der(
            0x30,
            &[
                der(0x06, &[0x2B, 6, 1, 4, 1, 0x82, 0x37, 2, 1, 4]),
                der(0xA0, &indirect),
            ]
            .concat(),
        );
        let signed = der(
            0x30,
            &[
                der(0x02, &[1]),
                der(0x31, &sha256),
                encapsulated,
                der(
                    0xA0,
                    &[
                        0x06, 3, 0x55, 4, 3, 0x0C, 9, b'T', b'e', b's', b't', b' ', b'C', b'e',
                        b'r', b't',
                    ],
                ),
            ]
            .concat(),
        );
        der(
            0x30,
            &[
                der(0x06, &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 1, 7, 2]),
                der(0xA0, &signed),
            ]
            .concat(),
        )
    }

    /// One .text section at file 0x400..0x600 and a certificate table of `cert` after it.
    fn signed_image(cert: Option<&[u8]>, tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x600];
        bytes[..2].copy_from_slice(b"MZ");
        put32(&mut bytes, 0x3C, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let file = 0x84;
        bytes[file..file + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[file + 2..file + 4].copy_from_slice(&1u16.to_le_bytes());
        bytes[file + 16..file + 18].copy_from_slice(&240u16.to_le_bytes());
        let optional = file + 20;
        bytes[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        put32(&mut bytes, optional + 56, 0x2000);
        put32(&mut bytes, optional + 60, 0x400);
        put32(&mut bytes, optional + 108, 16);
        let section = optional + 240;
        bytes[section..section + 5].copy_from_slice(b".text");
        put32(&mut bytes, section + 8, 0x200);
        put32(&mut bytes, section + 12, 0x1000);
        put32(&mut bytes, section + 16, 0x200);
        put32(&mut bytes, section + 20, 0x400);
        put32(&mut bytes, section + 36, 0x6000_0020);
        for (i, b) in bytes[0x400..0x600].iter_mut().enumerate() {
            *b = (i * 7) as u8;
        }
        bytes.extend(tail);
        if let Some(cert) = cert {
            let offset = bytes.len();
            let mut table = ((cert.len() + 8) as u32).to_le_bytes().to_vec();
            table.extend([0, 2, 2, 0]);
            table.extend(cert);
            while !table.len().is_multiple_of(8) {
                table.push(0);
            }
            put32(
                &mut bytes,
                optional + 112 + 8 * DIRECTORY_SECURITY,
                offset as u32,
            );
            put32(
                &mut bytes,
                optional + 112 + 8 * DIRECTORY_SECURITY + 4,
                table.len() as u32,
            );
            bytes.extend(table);
        }
        bytes
    }

    fn signed_over_itself() -> Vec<u8> {
        let unsigned = signed_image(None, b"");
        let image = Image::parse(&unsigned).unwrap();
        let digest = sha256(&unsigned, &regions(&image));
        signed_image(Some(&signature_blob(&digest)), b"")
    }

    #[test]
    fn stored_digest_matches_the_hash_of_the_ranges() {
        let bytes = signed_over_itself();
        let image = Image::parse(&bytes).unwrap();
        let signature = signature(&image).unwrap();
        assert_eq!(signature.algorithm, Some(DigestAlgorithm::Sha256));
        assert_eq!(signature.names, vec!["Test Cert".to_string()]);
        assert_eq!(signature.digest, sha256(&bytes, &regions(&image)).to_vec());
    }

    #[test]
    fn checksum_field_and_certificate_bytes_are_not_hashed() {
        let bytes = signed_over_itself();
        let image = Image::parse(&bytes).unwrap();
        let expected = sha256(&bytes, &regions(&image));
        let mut changed = bytes.clone();
        put32(&mut changed, image.checksum_offset(), 0xDEAD_BEEF);
        let (cert, size) = certificate_table(&image).unwrap();
        for byte in &mut changed[cert + size - 4..cert + size] {
            *byte ^= 0xFF;
        }
        let image = Image::parse(&changed).unwrap();
        assert_eq!(sha256(&changed, &regions(&image)), expected);
    }

    #[test]
    fn section_and_trailing_changes_are_hashed() {
        let bytes = signed_over_itself();
        let image = Image::parse(&bytes).unwrap();
        let expected = sha256(&bytes, &regions(&image));
        let mut patched = bytes.clone();
        patched[0x500] ^= 1;
        let image = Image::parse(&patched).unwrap();
        assert_ne!(sha256(&patched, &regions(&image)), expected);
        let mut header = bytes.clone();
        header[0x84 + 4] ^= 1;
        let image = Image::parse(&header).unwrap();
        assert_ne!(sha256(&header, &regions(&image)), expected);
    }

    #[test]
    fn data_after_the_last_section_is_covered_before_the_certificate() {
        let plain = signed_image(None, b"overlay!");
        let image = Image::parse(&plain).unwrap();
        let listed = regions(&image);
        assert_eq!(listed.last().unwrap().label, "data after sections");
        assert_eq!(listed.last().unwrap().start, 0x600);
        let signed = signed_image(Some(&signature_blob(&[0; 32])), b"overlay!");
        let image = Image::parse(&signed).unwrap();
        let labels: Vec<_> = regions(&image).iter().map(|r| r.label.clone()).collect();
        assert!(labels.contains(&"data between sections and certificate".to_string()));
        assert!(!labels.contains(&"data after certificate".to_string()));
    }

    #[test]
    fn checksum_matches_the_reference_algorithm() {
        let mut bytes = vec![0u8; 12];
        bytes[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&0xFFFFu16.to_le_bytes());
        bytes[8..10].copy_from_slice(&0x0001u16.to_le_bytes());
        // the field at 4..8 is ignored whatever it holds
        bytes[4..8].copy_from_slice(&[0xAA; 4]);
        let sum = 0x1234u32 + 0xFFFF + 1;
        let folded = (sum & 0xFFFF) + (sum >> 16);
        assert_eq!(checksum(&bytes, 4), folded + 12);
        let mut odd = bytes.clone();
        odd.push(0x80);
        assert_eq!(checksum(&odd, 4), folded + 0x80 + 13);
    }

    #[test]
    fn unsigned_files_have_no_signature() {
        let bytes = signed_image(None, b"");
        let image = Image::parse(&bytes).unwrap();
        assert_eq!(certificate_table(&image), None);
        assert_eq!(signature(&image), None);
    }

    #[test]
    fn oid_text_decodes_arcs() {
        assert_eq!(
            oid_text(&[0x2B, 6, 1, 4, 1, 0x82, 0x37, 2, 1, 4]),
            SPC_INDIRECT_DATA
        );
        assert_eq!(oid_text(&[0x55, 4, 3]), "2.5.4.3");
    }
}
