//! RDS5 texture pools (`.ttpf`): a chunk table, an embedded ETPF texture index and the chunk
//! data. The pool is parsed over a borrowed byte slice (a memory map or a test buffer).

use super::bcpack::{self, BcPackCodec, BcPackError, DecodeOptions};
use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::io::Read;
use thiserror::Error;

pub const ETPF_VERSION: u32 = 634;
const MAX_CHUNK: u64 = 256 * 1024 * 1024;
const MAX_TEXTURES: u32 = 250_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PoolError {
    #[error("not an RDS5 container or truncated header")]
    NotRds5,
    #[error("unsupported RDS5 index layout")]
    IndexLayout,
    #[error("RDS5 index exceeds its source")]
    IndexTruncated,
    #[error("RDS5 sentinel does not match the file and virtual sizes")]
    Sentinel,
    #[error("RDS5 chunk {0} range is invalid")]
    Chunk(usize),
    #[error("unsupported embedded ETPF index (version {0})")]
    Etpf(u32),
    #[error("ETPF tables are inconsistent with the file")]
    EtpfTables,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("range {start:#x}+{size:#x} is not completely mapped")]
    Unmapped { start: u64, size: u64 },
    #[error("chunk {0} uses unknown codec {1}")]
    UnknownCodec(usize, u8),
    #[error("zlib chunk {0} is truncated, corrupt or exceeds its declared size")]
    Zlib(usize),
    #[error("chunk {0} source range is outside the file")]
    Source(usize),
    #[error("BCPack chunk {chunk}: {source}")]
    BcPack {
        chunk: usize,
        #[source]
        source: BcPackError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Offset in the pool's virtual address space (4 KiB aligned).
    pub virt: u64,
    /// Decoded size.
    pub size: u64,
    /// File offset of the packed bytes.
    pub src: u64,
    pub packed: u64,
    /// 0 raw, 1..=7 BCPack, 8 zlib.
    pub codec: u8,
    pub pad: u64,
}

/// A chunk index with the result of decoding it.
type Decoded = (usize, Result<Vec<u8>, ReadError>);

/// Decoded chunks kept while one texture is read, so mips sharing a chunk inflate it once.
#[derive(Default)]
pub struct ChunkCache(HashMap<usize, Vec<u8>>);

impl ChunkCache {
    pub fn new() -> ChunkCache {
        ChunkCache::default()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Which codecs a virtual range touches, as a bit set over chunk codec ids 0..=15.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodecSet(pub u16);

impl CodecSet {
    pub fn contains(self, codec: u8) -> bool {
        self.0 & (1 << codec) != 0
    }

    /// First BCPack codec in the set that cannot be decoded.
    pub fn undecodable_bcpack(self) -> Option<BcPackCodec> {
        (1u8..=7)
            .filter(|c| self.contains(*c))
            .filter_map(BcPackCodec::from_chunk_codec)
            .find(|c| !bcpack::is_supported(*c))
    }

    pub fn has_bcpack(self) -> bool {
        (1u8..=7).any(|c| self.contains(c))
    }

    /// True when every chunk in the range can be decoded.
    pub fn readable(self) -> bool {
        self.undecodable_bcpack().is_none() && self.0 & !(0x1FF) == 0
    }
}

/// One 16-byte ETPF record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDescriptor {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub packed_mips: u32,
    /// Streaming snapshot count (cubemaps) or standalone mip count.
    pub non_tail: u32,
    pub mips: u32,
    pub format: u32,
    /// Slices of an array, faces of a cubemap (6) or depth of a volume.
    pub depth: u32,
    /// 1 = 2D / 2D array, 2 = cubemap, 3 = volume.
    pub dimension: u32,
    /// Low flag bit plus a 4-bit Gfx10 tile mode at bit 1.
    pub mode: u32,
    pub meta: u32,
    pub extra: u32,
    /// Virtual offset of the allocation (0 and `0x1FFFFF` pages mean unmapped).
    pub base: u64,
}

impl TextureDescriptor {
    pub fn from_record(index: u32, rec: &[u8; 16]) -> TextureDescriptor {
        let u = |i: usize| u32::from_le_bytes(rec[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
        let (a, b, c, d) = (u(0), u(1), u(2), u(3));
        TextureDescriptor {
            index,
            width: (a & 0x3FFF) + 1,
            height: ((a >> 14) & 0x3FFF) + 1,
            packed_mips: a >> 28,
            non_tail: b & 7,
            mips: (a >> 28) + (b & 7),
            format: (b >> 3) & 0x7F,
            depth: ((b >> 10) & 0x7FF) + 1,
            dimension: (b >> 21) & 3,
            mode: b >> 23,
            meta: c >> 21,
            extra: d,
            base: u64::from(c & 0x1FFFFF) * 4096,
        }
    }

    pub fn is_unmapped(&self) -> bool {
        is_unmapped_base(self.base)
    }
}

pub fn is_unmapped_base(base: u64) -> bool {
    base == 0 || (base >> 12) == 0x1F_FFFF
}

pub struct Pool<'a> {
    data: &'a [u8],
    pub virtual_size: u64,
    chunks: Vec<Chunk>,
    textures: u32,
    records: &'a [u8],
    ids: &'a [u8],
    bcpack: DecodeOptions,
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Pool<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Pool<'a>, PoolError> {
        if data.len() < 256 || &data[0..4] != b"RDS5" {
            return Err(PoolError::NotRds5);
        }
        let count = u32_at(data, 4).ok_or(PoolError::NotRds5)?;
        let size_word = u64_at(data, 8).ok_or(PoolError::NotRds5)?;
        let virtual_size = size_word & 0x00FF_FFFF_FFFF_FFFF;
        if !(3..=1_000_002).contains(&count) || size_word >> 56 != 22 || virtual_size < 4096 {
            return Err(PoolError::IndexLayout);
        }
        let table_end = 24 + (count as usize - 1) * 8;
        let etpf = table_end.div_ceil(256) * 256;
        if table_end + 16 > data.len() {
            return Err(PoolError::IndexTruncated);
        }
        let entry = |i: usize| u64_at(data, 24 + i * 8).expect("table bounds checked");
        let sentinel = entry(count as usize - 2);
        if (sentinel & 0x3FF_FFFF) * 256 != data.len() as u64
            || ((sentinel >> 30) & 0x3F_FFFF) * 4096 != virtual_size
        {
            return Err(PoolError::Sentinel);
        }
        let chunk_count = count as usize - 2;
        let mut chunks = Vec::with_capacity(chunk_count);
        for i in 0..chunk_count {
            let (v, nxt) = (entry(i), entry(i + 1));
            let start = (v & 0x3FF_FFFF) * 256;
            let end = (nxt & 0x3FF_FFFF) * 256;
            let virt = ((v >> 30) & 0x3F_FFFF) * 4096;
            let vend = ((nxt >> 30) & 0x3F_FFFF) * 4096;
            let pad = v >> 52;
            let codec = ((v >> 26) & 15) as u8;
            let bad = Err(PoolError::Chunk(i));
            if start < etpf as u64 + 16 || end > data.len() as u64 || vend <= virt {
                return bad;
            }
            let Some(packed) = (end - start).checked_sub(pad).filter(|p| *p >= 1) else {
                return bad;
            };
            let size = vend - virt;
            if packed > MAX_CHUNK
                || size > MAX_CHUNK
                || vend > virtual_size
                || (codec == 0 && packed != size)
                || chunks
                    .last()
                    .is_some_and(|c: &Chunk| c.virt + c.size > virt)
            {
                return bad;
            }
            chunks.push(Chunk {
                virt,
                size,
                src: start,
                packed,
                codec,
                pad,
            });
        }
        let eh = data.get(etpf..etpf + 16).ok_or(PoolError::IndexTruncated)?;
        let version = u32_at(eh, 4).ok_or(PoolError::IndexTruncated)?;
        if &eh[0..4] != b"ETPF" || version != ETPF_VERSION {
            return Err(PoolError::Etpf(version));
        }
        let textures = u32_at(eh, 8).ok_or(PoolError::IndexTruncated)?;
        let area = u32_at(eh, 12).ok_or(PoolError::IndexTruncated)? as usize;
        if !(1..=MAX_TEXTURES).contains(&textures) || area < textures as usize * 16 {
            return Err(PoolError::EtpfTables);
        }
        let rec_at = etpf + 16;
        let ids_at = rec_at + area;
        let records = data.get(rec_at..ids_at).ok_or(PoolError::EtpfTables)?;
        let ids = data
            .get(ids_at..ids_at + textures as usize * 32)
            .ok_or(PoolError::EtpfTables)?;
        Ok(Pool {
            data,
            virtual_size,
            chunks,
            textures,
            records,
            ids,
            bcpack: DecodeOptions::from_env(),
        })
    }

    /// Replace the BCPack options (the default comes from the environment, see [`DecodeOptions::from_env`]).
    pub fn with_bcpack_options(mut self, opts: DecodeOptions) -> Self {
        self.bcpack = opts;
        self
    }

    pub fn texture_count(&self) -> u32 {
        self.textures
    }

    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    pub fn descriptor(&self, index: u32) -> Option<TextureDescriptor> {
        if index >= self.textures {
            return None;
        }
        let at = index as usize * 16;
        let rec: &[u8; 16] = self.records.get(at..at + 16)?.try_into().ok()?;
        Some(TextureDescriptor::from_record(index, rec))
    }

    /// The 64-bit asset key. The key array follows the 16-byte records and continues into the
    /// 32-byte ID region when it outgrows the record area.
    pub fn asset_key(&self, index: u32) -> Option<u64> {
        if index >= self.textures {
            return None;
        }
        let at = self.textures as usize * 16 + index as usize * 8;
        if at + 8 <= self.records.len() {
            u64_at(self.records, at)
        } else if at >= self.records.len() {
            u64_at(self.ids, at - self.records.len())
        } else {
            None
        }
    }

    /// Allocation size of a texture: the distance to the next mapped allocation.
    pub fn span(&self, index: u32, base: u64) -> Option<u64> {
        if is_unmapped_base(base) || base >= self.virtual_size {
            return None;
        }
        for j in index + 1..self.textures {
            let cand = self.descriptor(j)?.base;
            if is_unmapped_base(cand) {
                continue;
            }
            if cand < base {
                return None;
            }
            if cand > base {
                return Some(cand - base);
            }
        }
        Some(self.virtual_size - base)
    }

    /// Chunks covering `[start, start + size)`; fails when the range has a hole.
    pub fn chunks_for(&self, start: u64, size: u64) -> Result<&[Chunk], ReadError> {
        let end = start + size;
        let first = self.chunks.partition_point(|c| c.virt + c.size <= start);
        let mut covered = start;
        let mut last = first;
        while last < self.chunks.len() && self.chunks[last].virt < end {
            if self.chunks[last].virt > covered {
                return Err(ReadError::Unmapped { start, size });
            }
            covered = end.min(self.chunks[last].virt + self.chunks[last].size);
            last += 1;
        }
        if covered != end || size == 0 {
            return Err(ReadError::Unmapped { start, size });
        }
        Ok(&self.chunks[first..last])
    }

    pub fn codecs(&self, start: u64, size: u64) -> Result<CodecSet, ReadError> {
        let mut set = 0u16;
        for c in self.chunks_for(start, size)? {
            set |= 1 << c.codec;
        }
        Ok(CodecSet(set))
    }

    fn chunk_index(&self, chunk: &Chunk) -> usize {
        self.chunks.partition_point(|c| c.virt < chunk.virt)
    }

    fn decode_chunk(&self, i: usize) -> Result<Vec<u8>, ReadError> {
        let c = &self.chunks[i];
        let raw = self
            .data
            .get(c.src as usize..(c.src + c.packed) as usize)
            .ok_or(ReadError::Source(i))?;
        match c.codec {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(c.size as usize);
                let mut dec = ZlibDecoder::new(raw).take(c.size + 1);
                dec.read_to_end(&mut out).map_err(|_| ReadError::Zlib(i))?;
                if out.len() as u64 != c.size {
                    return Err(ReadError::Zlib(i));
                }
                Ok(out)
            }
            1..=7 => {
                let codec = BcPackCodec::from_chunk_codec(c.codec).expect("1..=7");
                bcpack::decode_with(codec, raw, c.size as usize, &self.bcpack)
                    .map(|d| d.data)
                    .map_err(|source| ReadError::BcPack { chunk: i, source })
            }
            other => Err(ReadError::UnknownCodec(i, other)),
        }
    }

    /// Decode the chunks of a range into `cache` without copying the range out. Several missing chunks are
    /// decoded on separate threads (at most four): they are independent.
    pub fn warm(&self, start: u64, size: u64, cache: &mut ChunkCache) -> Result<(), ReadError> {
        let missing: Vec<usize> = self
            .chunks_for(start, size)?
            .iter()
            .map(|c| self.chunk_index(c))
            .filter(|i| !cache.0.contains_key(i))
            .collect();
        let threads = missing.len().min(
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(4),
        );
        if threads <= 1 {
            for i in missing {
                cache.0.insert(i, self.decode_chunk(i)?);
            }
            return Ok(());
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let done: Vec<Vec<Decoded>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&i) = missing.get(at) else { break };
                            mine.push((i, self.decode_chunk(i)));
                        }
                        mine
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("chunk decoding does not panic"))
                .collect()
        });
        let mut first_error: Option<(usize, ReadError)> = None;
        for (i, result) in done.into_iter().flatten() {
            match result {
                Ok(bytes) => {
                    cache.0.insert(i, bytes);
                }
                Err(e) if first_error.as_ref().is_none_or(|(j, _)| i < *j) => {
                    first_error = Some((i, e));
                }
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), |(_, e)| Err(e))
    }

    /// Decoded bytes of `[start, start + size)` of the virtual address space.
    pub fn read_range(
        &self,
        start: u64,
        size: u64,
        cache: &mut ChunkCache,
    ) -> Result<Vec<u8>, ReadError> {
        self.warm(start, size, cache)?;
        let mut out = vec![0u8; size as usize];
        for chunk in self.chunks_for(start, size)? {
            let i = self.chunk_index(chunk);
            let bytes = &cache.0[&i];
            let lo = start.max(chunk.virt);
            let hi = (start + size).min(chunk.virt + chunk.size);
            out[(lo - start) as usize..(hi - start) as usize]
                .copy_from_slice(&bytes[(lo - chunk.virt) as usize..(hi - chunk.virt) as usize]);
        }
        Ok(out)
    }
}

/// Test support: assemble a minimal valid pool from texture records and chunk payloads.
#[cfg(test)]
pub(crate) mod builder {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    pub enum Packed {
        Raw(Vec<u8>),
        Zlib(Vec<u8>),
        /// Bytes that claim to be BCPack, decoded size.
        BcPack(u8, Vec<u8>, u64),
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        width: u32,
        height: u32,
        mips: u32,
        non_tail: u32,
        format: u32,
        depth: u32,
        dimension: u32,
        mode: u32,
        base_pages: u32,
        meta: u32,
        extra: u32,
    ) -> [u8; 16] {
        let a = (width - 1) | ((height - 1) << 14) | ((mips - non_tail) << 28);
        let b = non_tail | (format << 3) | ((depth - 1) << 10) | (dimension << 21) | (mode << 23);
        let c = base_pages | (meta << 21);
        let mut r = [0u8; 16];
        r[0..4].copy_from_slice(&a.to_le_bytes());
        r[4..8].copy_from_slice(&b.to_le_bytes());
        r[8..12].copy_from_slice(&c.to_le_bytes());
        r[12..16].copy_from_slice(&extra.to_le_bytes());
        r
    }

    /// `chunks` are laid out back to back in the virtual space; returns the file bytes.
    pub fn pool(records: &[[u8; 16]], keys: &[u64], chunks: &[Packed]) -> Vec<u8> {
        let count = chunks.len() + 2;
        let table_end = 24 + (count - 1) * 8;
        let etpf = table_end.div_ceil(256) * 256;
        let area = records.len() * 16 + keys.len() * 8;
        let metadata = 16 + area + records.len() * 32;
        let data_start = etpf + metadata.div_ceil(4096) * 4096;

        let mut payload: Vec<(u8, Vec<u8>, u64)> = Vec::new();
        for c in chunks {
            payload.push(match c {
                Packed::Raw(b) => (0, b.clone(), b.len() as u64),
                Packed::Zlib(b) => {
                    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
                    e.write_all(b).unwrap();
                    (8, e.finish().unwrap(), b.len() as u64)
                }
                Packed::BcPack(codec, b, size) => (*codec, b.clone(), *size),
            });
        }
        let first_virt = (metadata.div_ceil(4096) * 4096) as u64;
        let mut table = Vec::new();
        let mut file_off = data_start as u64;
        let mut virt = first_virt;
        let mut body = Vec::new();
        for (codec, bytes, size) in &payload {
            let padded = (bytes.len() as u64).div_ceil(256) * 256;
            let pad = padded - bytes.len() as u64;
            let v =
                (file_off / 256) | ((virt / 4096) << 30) | (u64::from(*codec) << 26) | (pad << 52);
            table.push(v);
            body.extend_from_slice(bytes);
            body.resize(body.len() + pad as usize, 0);
            file_off += padded;
            virt += size.div_ceil(4096) * 4096;
        }
        table.push((file_off / 256) | ((virt / 4096) << 30));
        let mut f = vec![0u8; data_start];
        f[0..4].copy_from_slice(b"RDS5");
        f[4..8].copy_from_slice(&(count as u32).to_le_bytes());
        f[8..16].copy_from_slice(&(virt | (22u64 << 56)).to_le_bytes());
        for (i, v) in table.iter().enumerate() {
            f[24 + i * 8..32 + i * 8].copy_from_slice(&v.to_le_bytes());
        }
        f[etpf..etpf + 4].copy_from_slice(b"ETPF");
        f[etpf + 4..etpf + 8].copy_from_slice(&ETPF_VERSION.to_le_bytes());
        f[etpf + 8..etpf + 12].copy_from_slice(&(records.len() as u32).to_le_bytes());
        f[etpf + 12..etpf + 16].copy_from_slice(&(area as u32).to_le_bytes());
        let mut at = etpf + 16;
        for r in records {
            f[at..at + 16].copy_from_slice(r);
            at += 16;
        }
        for k in keys {
            f[at..at + 8].copy_from_slice(&k.to_le_bytes());
            at += 8;
        }
        f.extend_from_slice(&body);
        f
    }

    /// Virtual page of the first chunk for a pool built with `pool`.
    pub fn first_page(records: usize, keys: usize) -> u32 {
        let area = records * 16 + keys * 8;
        let metadata = 16 + area + records * 32;
        (metadata.div_ceil(4096) * 4096 / 4096) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::builder::*;
    use super::*;

    fn simple_pool() -> Vec<u8> {
        let first = first_page(2, 2);
        let recs = [
            record(64, 64, 1, 0, 28, 1, 1, 2, first, 0, 0),
            record(32, 32, 1, 0, 28, 1, 1, 2, first + 8, 0, 0),
        ];
        let a: Vec<u8> = (0..32768u32).map(|i| (i * 31) as u8).collect();
        let b: Vec<u8> = (0..4096u32).map(|i| (i * 7) as u8).collect();
        pool(
            &recs,
            &[0x1111, 0x2222],
            &[Packed::Zlib(a.clone()), Packed::Raw(b.clone())],
        )
    }

    #[test]
    fn parses_descriptors_keys_and_chunks() {
        let file = simple_pool();
        let p = Pool::parse(&file).unwrap();
        assert_eq!(p.texture_count(), 2);
        let d = p.descriptor(0).unwrap();
        assert_eq!(
            (d.width, d.height, d.mips, d.format, d.depth),
            (64, 64, 1, 28, 1)
        );
        assert_eq!((d.dimension, d.mode), (1, 2));
        assert_eq!(p.asset_key(0), Some(0x1111));
        assert_eq!(p.asset_key(1), Some(0x2222));
        assert_eq!(p.chunks().len(), 2);
        assert_eq!(p.chunks()[0].codec, 8);
        assert_eq!(p.chunks()[1].codec, 0);
        assert!(p.descriptor(2).is_none());
        let base0 = p.descriptor(0).unwrap().base;
        assert_eq!(p.span(0, base0), Some(32768));
        assert_eq!(p.span(1, p.descriptor(1).unwrap().base), Some(4096));
    }

    #[test]
    fn reads_across_chunks_and_codecs() {
        let file = simple_pool();
        let p = Pool::parse(&file).unwrap();
        let base = p.descriptor(0).unwrap().base;
        let mut cache = ChunkCache::new();
        let bytes = p.read_range(base + 32760, 16, &mut cache).unwrap();
        let a_tail: Vec<u8> = (32760..32768u32).map(|i| (i * 31) as u8).collect();
        let b_head: Vec<u8> = (0..8u32).map(|i| (i * 7) as u8).collect();
        assert_eq!(&bytes[..8], &a_tail[..]);
        assert_eq!(&bytes[8..], &b_head[..]);
        assert!(p.codecs(base, 32768 + 4096).unwrap().readable());
    }

    #[test]
    fn holes_and_out_of_range_are_errors() {
        let file = simple_pool();
        let p = Pool::parse(&file).unwrap();
        let base = p.descriptor(0).unwrap().base;
        let mut cache = ChunkCache::new();
        assert!(matches!(
            p.read_range(base + 36000, 8000, &mut cache),
            Err(ReadError::Unmapped { .. })
        ));
        assert!(matches!(
            p.read_range(0, 16, &mut cache),
            Err(ReadError::Unmapped { .. })
        ));
    }

    #[test]
    fn bcpack_chunk_is_a_typed_unsupported_read() {
        let first = first_page(1, 1);
        let recs = [record(64, 64, 1, 0, 71, 1, 1, 2, first, 0, 0)];
        let file = pool(&recs, &[1], &[Packed::BcPack(2, vec![0x1f; 100], 2048)]);
        let p = Pool::parse(&file).unwrap();
        let base = p.descriptor(0).unwrap().base;
        let set = p.codecs(base, 2048).unwrap();
        assert_eq!(set.undecodable_bcpack(), Some(BcPackCodec::Bc2));
        assert!(!set.readable());
        let err = p
            .read_range(base, 2048, &mut ChunkCache::new())
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::BcPack {
                source: BcPackError::Unsupported {
                    codec: BcPackCodec::Bc2
                },
                ..
            }
        ));
    }

    #[test]
    fn rejects_malformed_pools() {
        let file = simple_pool();
        assert!(matches!(Pool::parse(&file[..100]), Err(PoolError::NotRds5)));
        let mut bad = file.clone();
        bad[0] = b'X';
        assert!(matches!(Pool::parse(&bad), Err(PoolError::NotRds5)));
        let mut bad = file.clone();
        bad.push(0);
        assert!(matches!(Pool::parse(&bad), Err(PoolError::Sentinel)));
    }

    #[test]
    fn corrupt_zlib_chunk_is_an_error() {
        let mut file = simple_pool();
        let p = Pool::parse(&file).unwrap();
        let c = p.chunks()[0];
        file[c.src as usize + 4] ^= 0xFF;
        file[c.src as usize + 5] ^= 0xFF;
        let p = Pool::parse(&file).unwrap();
        let base = p.descriptor(0).unwrap().base;
        assert!(matches!(
            p.read_range(base, 16, &mut ChunkCache::new()),
            Err(ReadError::Zlib(0))
        ));
    }
}
