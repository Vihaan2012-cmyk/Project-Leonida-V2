//! Join the streams of the data area into resources and say what each one is.
//!
//! Without the table of contents a resource's streams are grouped by what the measured formats say:
//! - a `0DRP` page directory spans its streams until every `q38` record is covered;
//! - `PSIN`, `RBF0`, `RSC`, XML and text resources continue in the next stream while a stream is exactly 64 KiB;
//! - a zero-headed system page is the first stream, plus one smaller spill stream when the first is a multiple of
//!   64 KiB, followed by its graphics pages up to the next resource head;
//! - anything else is a resource of one stream.

use sha2::{Digest, Sha256};

use crate::model::{Codec, Kind, Layout, Resource, Segment, Space};
use crate::names;

const CHUNK: usize = 0x10000;
const NAME_PREFIX: usize = 1 << 20;
const PAGEDIR_VERSION: u32 = 122;
const PAGEDIR_HEADER: usize = 0x60;
const PAGEDIR_MAX_BUFFER: usize = 64 << 20;
const MAX_SPAN: u64 = 1 << 30;
const VIRTUAL_BASE: u64 = 0x5000_0000;
const PHYSICAL_BASE: u64 = 0x6000_0000;
/// Zero padding between the streams of one resource is at most one 16-byte unit.
const MAX_GAP: usize = 16;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Head {
    PageDir,
    Psin,
    Rbf0,
    Rsc,
    TexturePool,
    Xml,
    Text,
    SystemPage,
    Graphics,
    Raw,
}

impl Head {
    /// Whether a stream with this head starts a resource of its own.
    fn starts_resource(self) -> bool {
        !matches!(self, Head::Graphics | Head::Raw | Head::Text)
    }
}

fn is_zero_headed(data: &[u8]) -> bool {
    data.len() >= 16
        && data[..8].iter().all(|&byte| byte == 0)
        && (VIRTUAL_BASE..PHYSICAL_BASE).contains(&u64_at(data, 8))
}

fn is_text(data: &[u8]) -> bool {
    data.len() >= 64
        && data[..data.len().min(256)]
            .iter()
            .all(|&byte| matches!(byte, b'\t' | b'\n' | b'\r' | 0x20..=0x7E))
}

pub(crate) fn classify(data: &[u8]) -> Head {
    if is_zero_headed(data) {
        if data.len() >= PAGEDIR_HEADER
            && &data[0x10..0x14] == b"0DRP"
            && u32_at(data, 0x14) == PAGEDIR_VERSION
        {
            return Head::PageDir;
        }
        return Head::SystemPage;
    }
    match data.get(..4) {
        Some(b"PSIN") => Head::Psin,
        Some(b"RBF0") => Head::Rbf0,
        Some(b"RSC7" | b"RSC8") => Head::Rsc,
        Some(b"RDS5" | b"TTPF") => Head::TexturePool,
        Some([0x42, 0x40, 0, 0]) => Head::Graphics,
        _ => {
            let body = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
            if body.starts_with(b"<?xml") {
                Head::Xml
            } else if is_text(data) {
                Head::Text
            } else {
                Head::Raw
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PageDirParse {
    /// Every record is covered by the first `span` bytes of the logical resource.
    Span(u64),
    /// The `q38` table lies beyond the bytes read so far.
    NeedMore,
    Invalid,
}

/// Read the `q38` table of a page directory (`buf` is the logical resource so far) and return how far the
/// records reach. Records must point into the resource and tile it without overlapping.
fn parse_pagedir(buf: &[u8]) -> PageDirParse {
    if buf.len() < PAGEDIR_HEADER {
        return PageDirParse::Invalid;
    }
    let count = usize::from(u16::from_le_bytes([buf[0x4C], buf[0x4D]]));
    let table = u64_at(buf, 0x38);
    if count == 0 || !(VIRTUAL_BASE..PHYSICAL_BASE).contains(&table) {
        return PageDirParse::Invalid;
    }
    let table = (table - VIRTUAL_BASE) as usize;
    let Some(table_end) = table.checked_add(count * 16) else {
        return PageDirParse::Invalid;
    };
    if table_end > buf.len() {
        return if table_end <= PAGEDIR_MAX_BUFFER {
            PageDirParse::NeedMore
        } else {
            PageDirParse::Invalid
        };
    }
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let record = table + index * 16;
        let size = u64::from(u32_at(buf, record + 4));
        let pointer = u64_at(buf, record + 8);
        if !(VIRTUAL_BASE..PHYSICAL_BASE).contains(&pointer) {
            return PageDirParse::Invalid;
        }
        records.push((pointer - VIRTUAL_BASE, size));
    }
    records.sort_unstable();
    let mut reach = 0;
    for &(target, size) in &records {
        if target < reach {
            return PageDirParse::Invalid;
        }
        reach = target + size;
    }
    if reach > MAX_SPAN {
        return PageDirParse::Invalid;
    }
    PageDirParse::Span(reach)
}

#[derive(Debug)]
enum Rule {
    Single,
    /// Continues while the last stream is exactly 64 KiB.
    Chunked,
    /// A system page: `spill_open` while a spill stream may still follow the first stream.
    Paged {
        spill_open: bool,
        graphics: bool,
    },
    PageDir {
        buffer: Vec<u8>,
        span: Option<u64>,
    },
}

struct Open {
    kind: Kind,
    magic: [u8; 4],
    version: u32,
    segments: Vec<Segment>,
    hasher: Sha256,
    names: Vec<String>,
    end: u64,
    rule: Rule,
}

impl Open {
    fn inflated(&self) -> u64 {
        self.segments.iter().map(|s| u64::from(s.size)).sum()
    }

    fn push(&mut self, offset: u64, stored: u32, data: &[u8], space: Space) {
        self.hasher.update(data);
        self.segments.push(Segment {
            offset,
            stored,
            size: data.len() as u32,
            codec: Codec::Zlib,
            space,
        });
        self.end = offset + u64::from(stored);
    }

    /// Whether `data`, the stream at `offset`, continues this resource. Appends it when it does.
    fn continues(&mut self, offset: u64, stored: u32, data: &[u8], head: Head) -> bool {
        if head.starts_resource() {
            return false;
        }
        let last_size = self.segments.last().map_or(0, |s| s.size as usize);
        let covered = self.inflated();
        let streams = self.segments.len();
        let space = match &mut self.rule {
            Rule::Single => return false,
            Rule::Chunked => {
                if last_size != CHUNK {
                    return false;
                }
                Space::Virtual
            }
            Rule::Paged {
                spill_open,
                graphics,
            } => {
                let spill = *spill_open
                    && !*graphics
                    && head != Head::Graphics
                    && !data.len().is_multiple_of(CHUNK);
                *spill_open = false;
                *graphics = !spill;
                if spill {
                    Space::Virtual
                } else {
                    Space::Physical
                }
            }
            Rule::PageDir { buffer, span } => {
                let done = span.is_some_and(|span| covered >= span);
                if done || buffer.len() + data.len() > PAGEDIR_MAX_BUFFER || streams >= 8 {
                    return false;
                }
                if span.is_none() {
                    buffer.extend_from_slice(data);
                    match parse_pagedir(buffer) {
                        PageDirParse::Span(found) => *span = Some(found),
                        PageDirParse::NeedMore => {}
                        PageDirParse::Invalid => {
                            buffer.truncate(buffer.len() - data.len());
                            return false;
                        }
                    }
                }
                Space::Virtual
            }
        };
        self.push(offset, stored, data, space);
        true
    }

    fn finish(self) -> Resource {
        let size = self.inflated();
        let complete = match &self.rule {
            Rule::PageDir { span, .. } => span.is_some_and(|span| size >= span),
            _ => true,
        };
        let content_hash: [u8; 32] = self.hasher.finalize().into();
        let virtual_size = self
            .segments
            .iter()
            .filter(|s| s.space == Space::Virtual)
            .map(|s| u64::from(s.size))
            .sum();
        let first = self.segments[0].offset;
        let name = self.names.first().cloned();
        Resource {
            archive: 0,
            offset: first,
            stored: self.end - first,
            size,
            kind: self.kind,
            magic: self.magic,
            version: self.version,
            layout: Layout {
                virtual_size,
                physical_size: size - virtual_size,
                segments: self.segments,
                complete,
            },
            name_hash: name.as_deref().map(names::name_hash),
            name,
            names: self.names,
            content_hash,
        }
    }
}

/// Groups pieces handed over in archive order and passes each finished resource to the sink.
pub(crate) struct Grouper<'a> {
    archive: &'a [u8],
    sink: &'a mut dyn FnMut(Resource),
    open: Option<Open>,
}

impl<'a> Grouper<'a> {
    pub(crate) fn new(archive: &'a [u8], sink: &'a mut dyn FnMut(Resource)) -> Self {
        Grouper {
            archive,
            sink,
            open: None,
        }
    }

    pub(crate) fn finish(mut self) {
        self.close();
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            (self.sink)(open.finish());
        }
    }

    fn contiguous(&self, open: &Open, offset: u64) -> bool {
        let (from, to) = (open.end as usize, offset as usize);
        to >= from && to - from <= MAX_GAP && self.archive[from..to].iter().all(|&byte| byte == 0)
    }

    pub(crate) fn zlib(&mut self, offset: u64, stored: u32, data: &[u8]) {
        let head = classify(data);
        if let Some(mut open) = self.open.take() {
            if self.contiguous(&open, offset) && open.continues(offset, stored, data, head) {
                self.open = Some(open);
                return;
            }
            self.open = Some(open);
            self.close();
        }
        self.open = Some(start(offset, stored, data, head));
    }

    /// A resource that is stored as is: its own piece, never joined to a neighbour.
    pub(crate) fn raw(&mut self, kind: Kind, offset: u64, bytes: &[u8]) {
        self.close();
        let mut open = Open {
            kind,
            magic: bytes[..4].try_into().unwrap(),
            version: if kind == Kind::Bink2 {
                u32::from(bytes[3])
            } else {
                0
            },
            segments: Vec::new(),
            hasher: Sha256::new(),
            names: Vec::new(),
            end: offset,
            rule: Rule::Single,
        };
        open.hasher.update(bytes);
        open.segments.push(Segment {
            offset,
            stored: bytes.len() as u32,
            size: bytes.len() as u32,
            codec: Codec::Raw,
            space: Space::Virtual,
        });
        open.end = offset + bytes.len() as u64;
        self.open = Some(open);
        self.close();
    }
}

fn start(offset: u64, stored: u32, data: &[u8], head: Head) -> Open {
    let mut magic = [0u8; 4];
    let head_len = data.len().min(4);
    magic[..head_len].copy_from_slice(&data[..head_len]);
    let mut version = 0;
    let mut space = Space::Virtual;
    let mut found = Vec::new();
    let kind;
    let rule;
    match head {
        Head::PageDir => {
            let mut buffer = Vec::new();
            buffer.extend_from_slice(data);
            match parse_pagedir(&buffer) {
                PageDirParse::Invalid => return start(offset, stored, data, Head::SystemPage),
                parsed => {
                    kind = Kind::PageDir;
                    magic = *b"0DRP";
                    version = u32_at(data, 0x14);
                    found = names::find_names(&data[..data.len().min(NAME_PREFIX)]);
                    rule = Rule::PageDir {
                        buffer,
                        span: match parsed {
                            PageDirParse::Span(span) => Some(span),
                            _ => None,
                        },
                    };
                }
            }
        }
        Head::SystemPage => {
            found = names::find_names(&data[..data.len().min(NAME_PREFIX)]);
            kind = match found.first().and_then(|name| name.rsplit(".#").next()) {
                Some("dr") => Kind::Drawable,
                Some("ft") => Kind::Fragment,
                Some("dd") => Kind::DrawableDictionary,
                _ => Kind::SystemPage,
            };
            rule = Rule::Paged {
                spill_open: data.len().is_multiple_of(CHUNK),
                graphics: false,
            };
        }
        Head::Psin => {
            kind = Kind::Psin;
            if data.len() >= 12 {
                version = u32_at(data, 8);
            }
            rule = Rule::Chunked;
        }
        Head::Rbf0 => {
            kind = Kind::Rbf0;
            rule = Rule::Chunked;
        }
        Head::Rsc => {
            kind = Kind::Rsc;
            version = u32_at(data, 4);
            rule = Rule::Chunked;
        }
        Head::TexturePool => {
            kind = Kind::TexturePool;
            rule = Rule::Chunked;
        }
        Head::Xml => {
            kind = Kind::Xml;
            rule = Rule::Chunked;
        }
        Head::Text => {
            kind = Kind::Text;
            rule = Rule::Chunked;
        }
        Head::Graphics => {
            kind = Kind::GraphicsPage;
            space = Space::Physical;
            rule = Rule::Single;
        }
        Head::Raw => {
            kind = Kind::Raw;
            rule = Rule::Single;
        }
    }
    let mut open = Open {
        kind,
        magic,
        version,
        segments: Vec::new(),
        hasher: Sha256::new(),
        names: found,
        end: offset,
        rule,
    };
    open.push(offset, stored, data, space);
    open
}
