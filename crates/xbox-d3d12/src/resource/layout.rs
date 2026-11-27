//! Where each mip of a console texture lives in its memory.
//!
//! A tiled 2D texture is a chain of standalone mips, each padded to whole micro-tiles and stored smallest first
//! after one tile; the last mips that fit half a tile across and a tile down share that first tile (the "mip
//! tail"). Measured on 356,413 textures of the game package with no mismatch (`docs/specs/textures.md`, "The
//! layout is the AMD mip tail"); an array stores each mip for every slice together.

use super::tiling::{TileMode, Tiled2D};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("{width}x{height} cannot have {mips} mips")]
    Mips { width: u32, height: u32, mips: u32 },
    #[error("no tiled layout for {0}-byte elements")]
    Element(u32),
    #[error("block size {0} is not 1 or 4")]
    Block(u32),
    #[error("a surface needs a size")]
    Empty,
}

/// Where one mip is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MipLayout {
    pub index: u32,
    pub blocks_w: u32,
    pub blocks_h: u32,
    /// The size of the stored surface: a whole number of micro-tiles each way.
    pub padded_w: u32,
    pub padded_h: u32,
    /// From the start of the slice (the start of the texture for one slice).
    pub offset: u64,
    /// The bytes it takes: its padded surface, or the whole tail tile.
    pub region: u64,
    /// Where a tail mip starts inside the shared tile, in blocks.
    pub tile_x: u32,
    pub tile_y: u32,
    pub in_tail: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactLayout {
    pub mode: TileMode,
    pub element_bytes: u32,
    /// Bytes of the whole chain.
    pub size: u64,
    pub first_tail: u32,
    pub mips: Vec<MipLayout>,
}

/// Tail mip origins in a tile, in blocks: the x and y step by element size (1, 2, 4, 8, 16 bytes).
const TILE_ORIGIN: [(u32, u32); 5] = [(16, 16), (16, 8), (8, 8), (8, 4), (4, 4)];

fn blocks_at(count: u32, mip: u32) -> u32 {
    ((count + (1 << mip) - 1) >> mip).max(1)
}

/// The compact mip chain of one 2D texture. `block` is 4 for block-compressed formats and 1 otherwise;
/// `element_bytes` is a texel's size, or a block's.
pub fn compact_layout(
    mode: TileMode,
    width: u32,
    height: u32,
    mips: u32,
    element_bytes: u32,
    block: u32,
) -> Result<CompactLayout, LayoutError> {
    if width == 0 || height == 0 {
        return Err(LayoutError::Empty);
    }
    let log = match element_bytes {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 => 4,
        other => return Err(LayoutError::Element(other)),
    };
    if block != 1 && block != 4 {
        return Err(LayoutError::Block(block));
    }
    let levels = 32 - width.max(height).leading_zeros();
    if mips < 1 || mips > levels {
        return Err(LayoutError::Mips {
            width,
            height,
            mips,
        });
    }
    let tile = mode.tile_bytes();
    let half_bits = if mode == TileMode::Sw4kS { 6 } else { 8 };
    let max_tail = mode.max_tail_mips();
    let (micro_w, micro_h) = (
        1u32 << (half_bits - log / 2),
        1u32 << (half_bits - (log + 1) / 2),
    );
    let (origin_w, origin_h) = TILE_ORIGIN[log as usize];
    let (ew, eh) = (width.div_ceil(block), height.div_ceil(block));

    let mut tail = mips;
    let mut out: Vec<MipLayout> = Vec::new();
    for i in 0..mips {
        let (wc, hc) = (blocks_at(ew, i), blocks_at(eh, i));
        if mips > 1 && wc <= micro_w / 2 && hc <= micro_h && mips - i <= max_tail {
            tail = i;
            break;
        }
        let (pw, ph) = (
            wc.div_ceil(micro_w) * micro_w,
            hc.div_ceil(micro_h) * micro_h,
        );
        out.push(MipLayout {
            index: i,
            blocks_w: wc,
            blocks_h: hc,
            padded_w: pw,
            padded_h: ph,
            offset: 0,
            region: u64::from(pw) * u64::from(ph) * u64::from(element_bytes),
            tile_x: 0,
            tile_y: 0,
            in_tail: false,
        });
    }
    let mut offset = if tail < mips { tile } else { 0 };
    for mip in out.iter_mut().rev() {
        mip.offset = offset;
        offset += mip.region;
    }
    for i in tail..mips {
        let entry = max_tail - 1 - (i - tail);
        let in_tile = if entry > 6 {
            16u32 << entry
        } else {
            entry << 8
        };
        let (mut x, mut y) = (0, 0);
        for bit in 0..6 {
            x |= ((in_tile >> (9 + 2 * bit)) & 1) << bit;
            y |= ((in_tile >> (8 + 2 * bit)) & 1) << bit;
        }
        out.push(MipLayout {
            index: i,
            blocks_w: blocks_at(ew, i),
            blocks_h: blocks_at(eh, i),
            padded_w: micro_w,
            padded_h: micro_h,
            offset: 0,
            region: tile,
            tile_x: x * origin_w,
            tile_y: y * origin_h,
            in_tail: true,
        });
    }
    Ok(CompactLayout {
        mode,
        element_bytes,
        size: offset,
        first_tail: tail,
        mips: out,
    })
}

impl CompactLayout {
    /// The byte offset of the block at (`x`, `y`) of `mip`, from the start of the texture.
    pub fn block_offset(&self, mip: u32, x: u32, y: u32) -> Option<u64> {
        let m = self.mips.get(mip as usize)?;
        if x >= m.blocks_w || y >= m.blocks_h {
            return None;
        }
        let surface = Tiled2D::new(self.mode, self.element_bytes, m.padded_w, m.padded_h)?;
        if m.in_tail {
            Some(surface.intra_offset(x + m.tile_x, y + m.tile_y))
        } else {
            Some(m.offset + surface.offset(x, y)?)
        }
    }

    /// One mip as tightly packed rows of blocks.
    pub fn detile_mip(&self, mip: u32, tiled: &[u8]) -> Option<Vec<u8>> {
        let m = self.mips.get(mip as usize)?;
        let element = self.element_bytes as usize;
        if (tiled.len() as u64) < self.size {
            return None;
        }
        let mut out = vec![0; m.blocks_w as usize * m.blocks_h as usize * element];
        for y in 0..m.blocks_h {
            for x in 0..m.blocks_w {
                let at = self.block_offset(mip, x, y)? as usize;
                let line = (y * m.blocks_w + x) as usize * element;
                out[line..line + element].copy_from_slice(&tiled[at..at + element]);
            }
        }
        Some(out)
    }

    /// Writes tightly packed rows of blocks into one mip of the texture's memory.
    pub fn write_mip(&self, mip: u32, linear: &[u8], tiled: &mut [u8]) -> Option<()> {
        let m = self.mips.get(mip as usize)?;
        let element = self.element_bytes as usize;
        if linear.len() < m.blocks_w as usize * m.blocks_h as usize * element
            || (tiled.len() as u64) < self.size
        {
            return None;
        }
        for y in 0..m.blocks_h {
            for x in 0..m.blocks_w {
                let at = self.block_offset(mip, x, y)? as usize;
                let line = (y * m.blocks_w + x) as usize * element;
                tiled[at..at + element].copy_from_slice(&linear[line..line + element]);
            }
        }
        Some(())
    }
}

/// An array (or cube) of the same chain: the tail tile of every slice first, then each standalone mip from the
/// smallest, all slices of a mip together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayLayout {
    pub slice: CompactLayout,
    pub slices: u32,
    pub size: u64,
    /// For each mip: where slice 0 is, and the distance to the next slice.
    pub mips: Vec<(u64, u64)>,
}

pub fn array_layout(
    mode: TileMode,
    width: u32,
    height: u32,
    slices: u32,
    mips: u32,
    element_bytes: u32,
    block: u32,
) -> Result<ArrayLayout, LayoutError> {
    let slice = compact_layout(mode, width, height, mips, element_bytes, block)?;
    let tile = mode.tile_bytes();
    let n = u64::from(slices.max(1));
    let mut placed = vec![(0, 0); mips as usize];
    let mut offset = if slice.first_tail < mips { n * tile } else { 0 };
    for i in (0..slice.first_tail as usize).rev() {
        let region = slice.mips[i].region;
        placed[i] = (offset, region);
        offset += n * region;
    }
    for p in placed.iter_mut().skip(slice.first_tail as usize) {
        *p = (0, tile);
    }
    Ok(ArrayLayout {
        slice,
        slices: slices.max(1),
        size: offset,
        mips: placed,
    })
}

impl ArrayLayout {
    /// The byte offset of the block at (`x`, `y`) of `mip` in `slice`, from the start of the texture.
    pub fn block_offset(&self, slice: u32, mip: u32, x: u32, y: u32) -> Option<u64> {
        if slice >= self.slices {
            return None;
        }
        let m = self.slice.mips.get(mip as usize)?;
        let single = self.slice.block_offset(mip, x, y)?;
        let (start, stride) = self.mips[mip as usize];
        let step = u64::from(slice) * stride;
        Some(if m.in_tail {
            single + step
        } else {
            single - m.offset + start + step
        })
    }

    /// One subresource as tightly packed rows of blocks.
    pub fn detile(&self, slice: u32, mip: u32, memory: &[u8]) -> Option<Vec<u8>> {
        let m = self.slice.mips.get(mip as usize)?;
        let element = self.slice.element_bytes as usize;
        if (memory.len() as u64) < self.size {
            return None;
        }
        let mut out = vec![0; m.blocks_w as usize * m.blocks_h as usize * element];
        for y in 0..m.blocks_h {
            for x in 0..m.blocks_w {
                let at = self.block_offset(slice, mip, x, y)? as usize;
                let line = (y * m.blocks_w + x) as usize * element;
                out[line..line + element].copy_from_slice(&memory[at..at + element]);
            }
        }
        Some(out)
    }

    /// Writes tightly packed rows of blocks into one subresource of the texture's memory.
    pub fn tile(&self, slice: u32, mip: u32, rows: &[u8], memory: &mut [u8]) -> Option<()> {
        let m = self.slice.mips.get(mip as usize)?;
        let element = self.slice.element_bytes as usize;
        if (memory.len() as u64) < self.size
            || rows.len() < m.blocks_w as usize * m.blocks_h as usize * element
        {
            return None;
        }
        for y in 0..m.blocks_h {
            for x in 0..m.blocks_w {
                let at = self.block_offset(slice, mip, x, y)? as usize;
                let line = (y * m.blocks_w + x) as usize * element;
                memory[at..at + element].copy_from_slice(&rows[line..line + element]);
            }
        }
        Some(())
    }
}

/// A subresource in the row-major arrangement `GetCopyableFootprints` describes: rows `row_pitch` bytes apart
/// (a multiple of 256), each subresource starting at a multiple of 512.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footprint {
    pub offset: u64,
    pub width_blocks: u32,
    pub rows: u32,
    pub depth: u32,
    pub row_bytes: u64,
    pub row_pitch: u64,
}

impl Footprint {
    pub fn size(&self) -> u64 {
        self.row_pitch * u64::from(self.rows) * u64::from(self.depth)
    }
}

pub const PITCH_ALIGNMENT: u64 = 256;
pub const PLACEMENT_ALIGNMENT: u64 = 512;

/// Every subresource (mip fastest within a slice, slices after) of a texture in row-major order; the total.
pub fn row_major_footprints(
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    slices: u32,
    block: [u32; 2],
    element_bytes: u32,
) -> (Vec<Footprint>, u64) {
    let mut out = Vec::new();
    let mut offset = 0u64;
    for _slice in 0..slices.max(1) {
        for mip in 0..mips.max(1) {
            let w = (width >> mip).max(1);
            let h = (height >> mip).max(1);
            let d = (depth >> mip).max(1);
            let (bw, bh) = (w.div_ceil(block[0]), h.div_ceil(block[1]));
            let row_bytes = u64::from(bw) * u64::from(element_bytes);
            let footprint = Footprint {
                offset: offset.next_multiple_of(PLACEMENT_ALIGNMENT),
                width_blocks: bw,
                rows: bh,
                depth: d,
                row_bytes,
                row_pitch: row_bytes.next_multiple_of(PITCH_ALIGNMENT),
            };
            offset = footprint.offset + footprint.size();
            out.push(footprint);
        }
    }
    (out, offset)
}
