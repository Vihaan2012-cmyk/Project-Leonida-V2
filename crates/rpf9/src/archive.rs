use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;

use memmap2::Mmap;
use sha2::{Digest, Sha256};

use crate::error::Rpf9Error;
use crate::group::Grouper;
use crate::inflate::{Inflater, decode};
use crate::model::{Codec, Kind, Layout, Resource, Segment, Space};
use crate::names;
use crate::walk::{Piece, WalkStats, walk};

const VIRTUAL_BASE: u64 = 0x5000_0000;
const PHYSICAL_BASE: u64 = 0x6000_0000;
/// kind u8, stored u32, id u32, stored u32, size u32
const SHADER_RECORD_HEADER: usize = 17;

/// The fixed header of an RPF9 archive. The table of contents after it is encrypted and never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Where the data area starts (the `u32` at +4).
    pub data_start: u64,
    /// The `u32` at +8; its role is not established.
    pub field8: u32,
    /// Size of the block that ends the file (the `u32` at +12), not part of any stored file.
    pub trailer: u32,
}

enum Storage {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

/// An archive opened for reading. Files are mapped, not read into memory.
pub struct Archive {
    storage: Storage,
    header: Header,
}

/// What a scan saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub walk: WalkStats,
    pub resources: u64,
    /// Shader records whose header looked right but whose data was not a container (`shaders.rpf` only).
    pub rejected: u64,
}

/// A resource's inflated bytes, split the way the game addresses them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceData {
    /// The system pages: pointers `0x5xxxxxxx` are offsets into this.
    pub virtual_data: Vec<u8>,
    /// The graphics pages: pointers `0x6xxxxxxx` are offsets into this.
    pub physical_data: Vec<u8>,
}

impl ResourceData {
    /// The bytes from a game pointer to the end of its part, or `None` when it points outside both.
    pub fn at_pointer(&self, pointer: u64) -> Option<&[u8]> {
        let (data, base) = if (VIRTUAL_BASE..PHYSICAL_BASE).contains(&pointer) {
            (&self.virtual_data, VIRTUAL_BASE)
        } else if (PHYSICAL_BASE..PHYSICAL_BASE + (1 << 28)).contains(&pointer) {
            (&self.physical_data, PHYSICAL_BASE)
        } else {
            return None;
        };
        data.get((pointer - base) as usize..)
    }

    /// Virtual then physical, for resources that only have one part.
    pub fn into_bytes(mut self) -> Vec<u8> {
        self.virtual_data.append(&mut self.physical_data);
        self.virtual_data
    }
}

impl Archive {
    pub fn open(path: &Path) -> Result<Archive, Rpf9Error> {
        let file = File::open(path).map_err(|error| Rpf9Error::io(path, error))?;
        let map = game_package::map_file(&file).map_err(|error| Rpf9Error::io(path, error))?;
        let header = parse_header(&map)?;
        Ok(Archive {
            storage: Storage::Mapped(map),
            header,
        })
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Archive, Rpf9Error> {
        let header = parse_header(&bytes)?;
        Ok(Archive {
            storage: Storage::Owned(bytes),
            header,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.storage {
            Storage::Mapped(map) => map,
            Storage::Owned(bytes) => bytes,
        }
    }

    pub fn header(&self) -> Header {
        self.header
    }

    pub fn len(&self) -> u64 {
        self.bytes().len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.bytes().is_empty()
    }

    /// Find every validated resource of the data area by walking its streams, in archive order. The table of
    /// contents is not read.
    pub fn scan(&self, mut sink: impl FnMut(Resource)) -> ScanReport {
        let data = self.bytes();
        let end = data.len() - self.header.trailer as usize;
        let mut resources = 0;
        let mut counting = |resource: Resource| {
            resources += 1;
            sink(resource);
        };
        let mut grouper = Grouper::new(data, &mut counting);
        let walked = walk(
            data,
            self.header.data_start as usize,
            end,
            &mut |piece| match piece {
                Piece::Zlib {
                    offset,
                    stored,
                    data,
                } => grouper.zlib(offset, stored, data),
                Piece::Bink2 { offset, bytes } => grouper.raw(Kind::Bink2, offset, bytes),
                Piece::Awc { offset, bytes } => grouper.raw(Kind::Awc, offset, bytes),
            },
        );
        grouper.finish();
        ScanReport {
            walk: walked,
            resources,
            rejected: 0,
        }
    }

    /// `shaders.rpf` holds named records of bare deflate data instead of zlib streams: one resource per shader
    /// container found.
    pub fn scan_shaders(&self, mut sink: impl FnMut(Resource)) -> ScanReport {
        let data = self.bytes();
        let mut report = ScanReport::default();
        let stats = shader_container::carve(data, |shader| {
            let nul = shader.offset + shader.name.len();
            let header = &data[nul + 1..nul + 1 + SHADER_RECORD_HEADER];
            let stored = u32::from_le_bytes(header[1..5].try_into().unwrap());
            let data_start = (nul + 1 + SHADER_RECORD_HEADER) as u64;
            let size = shader.dxbc.len() as u32;
            let name = shader.name;
            report.walk.streams += 1;
            report.walk.stored_bytes += u64::from(stored);
            report.walk.inflated_bytes += u64::from(size);
            sink(Resource {
                archive: 0,
                offset: shader.offset as u64,
                stored: data_start + u64::from(stored) - shader.offset as u64,
                size: u64::from(size),
                kind: Kind::Shader,
                magic: *b"DXBC",
                version: 0,
                layout: Layout {
                    virtual_size: u64::from(size),
                    physical_size: 0,
                    segments: vec![Segment {
                        offset: data_start,
                        stored,
                        size,
                        codec: Codec::Deflate,
                        space: Space::Virtual,
                    }],
                    complete: true,
                },
                name_hash: Some(names::name_hash(&name)),
                names: vec![name.clone()],
                name: Some(name),
                content_hash: Sha256::digest(&shader.dxbc).into(),
            });
            ControlFlow::Continue(())
        });
        report.resources = stats.carved as u64;
        report.rejected = stats.rejected as u64;
        report
    }

    /// Read a resource's bytes: every segment decompressed and placed in its part.
    pub fn read(&self, resource: &Resource) -> Result<ResourceData, Rpf9Error> {
        let bytes = self.bytes();
        let mut inflater = Inflater::default();
        let mut out = ResourceData::default();
        for segment in &resource.layout.segments {
            let start = segment.offset as usize;
            let stored = bytes
                .get(start..start.saturating_add(segment.stored as usize))
                .ok_or(Rpf9Error::OutOfRange {
                    offset: segment.offset,
                    len: u64::from(segment.stored),
                })?;
            let plain = decode(&mut inflater, segment.codec, stored, segment.size as usize)
                .map_err(|detail| Rpf9Error::Corrupt {
                    codec: segment.codec.name(),
                    offset: segment.offset,
                    detail,
                })?;
            if plain.len() != segment.size as usize {
                return Err(Rpf9Error::SizeMismatch {
                    offset: segment.offset,
                    declared: u64::from(segment.size),
                    actual: plain.len() as u64,
                });
            }
            match segment.space {
                Space::Virtual => out.virtual_data.extend_from_slice(&plain),
                Space::Physical => out.physical_data.extend_from_slice(&plain),
            }
        }
        Ok(out)
    }
}

fn parse_header(bytes: &[u8]) -> Result<Header, Rpf9Error> {
    if bytes.len() < 16 {
        return Err(Rpf9Error::NotAnArchive("shorter than the 16-byte header"));
    }
    if &bytes[..4] != b"9FPR" && &bytes[..4] != b"RPF9" {
        return Err(Rpf9Error::NotAnArchive("no RPF9 magic"));
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let data_start = u64::from(word(4));
    if !(16..=bytes.len() as u64).contains(&data_start) {
        return Err(Rpf9Error::NotAnArchive("data start is outside the file"));
    }
    let mut trailer = word(12);
    if trailer % 16 != 0 || u64::from(trailer) > bytes.len() as u64 - data_start {
        trailer = 0;
    }
    Ok(Header {
        data_start,
        field8: word(8),
        trailer,
    })
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::walk::testutil::zlib;

    /// Builds an archive: header, an opaque table of contents, then the pieces added with `push`.
    pub struct ArchiveBuilder {
        pub bytes: Vec<u8>,
    }

    impl ArchiveBuilder {
        pub fn new() -> Self {
            let mut bytes = vec![0u8; 0x40];
            bytes[..4].copy_from_slice(b"9FPR");
            bytes[4..8].copy_from_slice(&0x40u32.to_le_bytes());
            for (index, byte) in bytes[0x10..].iter_mut().enumerate() {
                *byte = 0xA7u8.wrapping_mul(index as u8 | 1);
            }
            ArchiveBuilder { bytes }
        }

        /// Pad with zeros to a multiple of `unit`, then add 16 zero bytes, so the next stream starts a resource.
        pub fn start_resource(&mut self) {
            while self.bytes.len() % 4096 != 0 {
                self.bytes.push(0);
            }
            self.bytes.extend_from_slice(&[0u8; 16]);
        }

        /// Add a zlib stream; returns its archive offset.
        pub fn stream(&mut self, data: &[u8]) -> u64 {
            let at = self.bytes.len() as u64;
            self.bytes.extend_from_slice(&zlib(data));
            at
        }

        pub fn raw(&mut self, data: &[u8]) -> u64 {
            let at = self.bytes.len() as u64;
            self.bytes.extend_from_slice(data);
            at
        }

        pub fn finish(mut self, trailer: usize) -> Archive {
            while self.bytes.len() % 4096 != 0 {
                self.bytes.push(0);
            }
            let trailer = trailer / 16 * 16;
            self.bytes
                .extend((0..trailer).map(|index| 0x5Bu8.wrapping_mul(index as u8 | 1)));
            self.bytes[12..16].copy_from_slice(&(trailer as u32).to_le_bytes());
            Archive::from_bytes(self.bytes).unwrap()
        }
    }
}
