//! xg_xs.dll: texture layout and swizzle mode helpers.
//!
//! `layout` is the layout computer itself and has no console dependency: sizes, alignments and mip offsets of
//! a texture in the console's tiled swizzle modes. Its rules are the measured ones: the mip chain that
//! 356,413 single-slice 2D texture descriptors, 127 arrays, 567 cubemaps and 3 volumes of the package follow
//! byte for byte (`textures.md`, `textures_arrays.md`).
//!
//! The two exports follow what the executable's call sites measured (`runtime_imports.md`,
//! `audio_hal_exe_calls.md`):
//!
//! - `XGSuggestSwizzleMode(desc, 0, &mode)`: the out pointer is a 32-bit field of the description the caller
//!   zeroed first; the caller writes `0x100 | mode` there when the call succeeds.
//! - `XGCreateTextureComputer(desc, &computer)`: four call sites, using vtable slots 1 (release), 2 (one out
//!   pointer to a large structure), 8 (six arguments, in a loop over subresources) and 10 (seven arguments).
//!   No site reads a return value from slots 2, 8 or 10.
//!
//! The description is a `D3D12_RESOURCE_DESC`, measured in the builder at rva 0x4ecc940: dimension (3 for 2D,
//! 4 for 3D) at +0, width (64-bit) at +0x10, height at +0x18, depth or array size (16-bit) at +0x1c, mips (16-bit)
//! at +0x1e, format at +0x20, sample count at +0x24, the swizzle mode (`0x100 | mode`, the field
//! `XGSuggestSwizzleMode` is pointed at) at +0x2c and the D3D12 resource flags at +0x30.
//!
//! Unmeasured, and so marked with a trace note when reached: the numbering of swizzle modes (AMD's address library numbering is used), the
//! resource layout structure slot 2 fills (the public one's leading plane is written) and what slots 8 and 10
//! do (they answer `E_NOTIMPL` and write nothing).

pub mod layout {
    use thiserror::Error;

    pub const TILE_256: u32 = 256;
    pub const TILE_4K: u32 = 4096;
    pub const TILE_64K: u32 = 65536;

    #[derive(Debug, Clone, PartialEq, Eq, Error)]
    pub enum LayoutError {
        #[error("invalid texture dimensions {width}x{height}x{depth}")]
        Dimensions { width: u32, height: u32, depth: u32 },
        #[error("mip count {mips} does not fit {width}x{height}")]
        Mips { mips: u32, width: u32, height: u32 },
        #[error("DXGI format {0} has no known element size")]
        Format(u32),
        #[error("unsupported: {0}")]
        Unsupported(&'static str),
    }

    /// A swizzle mode by AMD's address library number.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SwizzleMode(pub u32);

    impl SwizzleMode {
        pub const LINEAR: SwizzleMode = SwizzleMode(0);
        pub const SW_4KB_S: SwizzleMode = SwizzleMode(4);
        pub const SW_64KB_S: SwizzleMode = SwizzleMode(8);
        pub const SW_64KB_D: SwizzleMode = SwizzleMode(9);
        pub const SW_64KB_R_X: SwizzleMode = SwizzleMode(22);

        /// The tile this mode stores a texture in, or `None` for linear.
        pub fn tile_bytes(self) -> Result<Option<u32>, LayoutError> {
            match self.0 {
                0 | 25 => Ok(None),
                1 | 2 => Ok(Some(TILE_256)),
                3..=6 | 15..=18 => Ok(Some(TILE_4K)),
                7..=14 | 19..=22 => Ok(Some(TILE_64K)),
                _ => Err(LayoutError::Unsupported("variable-size swizzle modes")),
            }
        }
    }

    /// Element size and block edge of a DXGI format.
    pub fn format_info(dxgi: u32) -> Option<(u32, u32)> {
        let info = match dxgi {
            1..=4 => (16, 1),
            5..=8 => (12, 1),
            9..=18 => (8, 1),
            23..=47 | 67 | 87..=93 => (4, 1),
            48..=59 | 85 | 86 | 115 => (2, 1),
            60..=65 => (1, 1),
            70..=72 | 79..=81 => (8, 4),
            73..=78 | 82..=84 | 94..=99 => (16, 4),
            _ => return None,
        };
        Some(info)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Dimension {
        D2,
        D3,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TextureSpec {
        pub dimension: Dimension,
        pub width: u32,
        pub height: u32,
        /// Volume depth; for 2D, the array size (a cube counts its faces here too).
        pub depth: u32,
        /// Zero asks for the full chain.
        pub mips: u32,
        pub format: u32,
        pub cube: bool,
        pub swizzle: SwizzleMode,
    }

    impl TextureSpec {
        pub fn texture_2d(
            width: u32,
            height: u32,
            mips: u32,
            format: u32,
            swizzle: SwizzleMode,
        ) -> Self {
            TextureSpec {
                dimension: Dimension::D2,
                width,
                height,
                depth: 1,
                mips,
                format,
                cube: false,
                swizzle,
            }
        }
    }

    /// One mip level. `size` is one slice's region; the level's whole extent is `size * slices` for arrays.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MipLayout {
        pub offset: u64,
        pub size: u64,
        /// Distance between slices of this level.
        pub slice_stride: u64,
        pub width_pixels: u32,
        pub height_pixels: u32,
        pub width_elements: u32,
        pub height_elements: u32,
        pub padded_width_elements: u32,
        pub padded_height_elements: u32,
        pub padded_depth: u32,
        pub pitch_bytes: u32,
        /// In the shared tail tile, with the element origin of the level inside it.
        pub in_tail: bool,
        pub tail_x: u32,
        pub tail_y: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ResourceLayout {
        pub size: u64,
        pub alignment: u32,
        pub element_bytes: u32,
        pub block: u32,
        pub slices: u32,
        /// Slices of an array lie inside each level (levels stored one after another) rather than one
        /// after another with each slice holding its whole chain.
        pub mip_major: bool,
        pub first_tail: u32,
        pub tile_width_elements: u32,
        pub tile_height_elements: u32,
        pub mips: Vec<MipLayout>,
    }

    const TILE_ORIGIN: [(u32, u32); 5] = [(16, 16), (16, 8), (8, 8), (8, 4), (4, 4)];
    const VOLUME_TILE: [(u32, u32, u32); 5] =
        [(16, 16, 16), (8, 16, 16), (8, 16, 8), (8, 8, 8), (4, 8, 8)];

    fn align(value: u64, to: u64) -> u64 {
        value.div_ceil(to) * to
    }

    fn levels(width: u32, height: u32, depth: u32) -> u32 {
        32 - width.max(height).max(depth).leading_zeros()
    }

    /// The Gfx10 chain of one 2D slice: standalone mips smallest first, the last few sharing a tile.
    fn compact(
        width: u32,
        height: u32,
        mips: u32,
        element: u32,
        block: u32,
        tile: u32,
    ) -> Result<(u64, u32, u32, u32, Vec<MipLayout>), LayoutError> {
        if !matches!(element, 1 | 2 | 4 | 8 | 16) {
            return Err(LayoutError::Unsupported(
                "element sizes of 3 or 12 bytes in tiled modes",
            ));
        }
        let log = element.trailing_zeros();
        // A 256-byte tile has no mip tail: every level is padded to whole tiles of its own.
        let (half_bits, max_tail) = match tile {
            TILE_256 => (4, 0),
            TILE_4K => (6, 8),
            _ => (8, 12),
        };
        let bw = 1u32 << (half_bits - log / 2);
        let bh = 1u32 << (half_bits - log.div_ceil(2));
        let (mw, mh) = TILE_ORIGIN[log as usize];
        let ew = width.div_ceil(block);
        let eh = height.div_ceil(block);

        let mut tail = mips;
        let mut out: Vec<MipLayout> = Vec::new();
        for i in 0..mips {
            let wc = ((ew + (1 << i) - 1) >> i).max(1);
            let hc = ((eh + (1 << i) - 1) >> i).max(1);
            if mips > 1 && wc <= bw / 2 && hc <= bh && mips - i <= max_tail {
                tail = i;
                break;
            }
            let pw = wc.div_ceil(bw) * bw;
            let ph = hc.div_ceil(bh) * bh;
            out.push(MipLayout {
                offset: 0,
                size: u64::from(pw) * u64::from(ph) * u64::from(element),
                slice_stride: 0,
                width_pixels: (width >> i).max(1),
                height_pixels: (height >> i).max(1),
                width_elements: wc,
                height_elements: hc,
                padded_width_elements: pw,
                padded_height_elements: ph,
                padded_depth: 1,
                pitch_bytes: pw * element,
                in_tail: false,
                tail_x: 0,
                tail_y: 0,
            });
        }
        let mut off = if tail < mips { u64::from(tile) } else { 0 };
        for mip in out.iter_mut().rev() {
            mip.offset = off;
            mip.slice_stride = mip.size;
            off += mip.size;
        }
        for i in tail..mips {
            let entry = max_tail - 1 - (i - tail);
            let moff: u32 = if entry > 6 { 16 << entry } else { entry << 8 };
            let (mut x, mut y) = (0, 0);
            for bit in 0..6 {
                x |= ((moff >> (9 + 2 * bit)) & 1) << bit;
                y |= ((moff >> (8 + 2 * bit)) & 1) << bit;
            }
            out.push(MipLayout {
                offset: 0,
                size: u64::from(tile),
                slice_stride: u64::from(tile),
                width_pixels: (width >> i).max(1),
                height_pixels: (height >> i).max(1),
                width_elements: ((ew + (1 << i) - 1) >> i).max(1),
                height_elements: ((eh + (1 << i) - 1) >> i).max(1),
                padded_width_elements: bw,
                padded_height_elements: bh,
                padded_depth: 1,
                pitch_bytes: bw * element,
                in_tail: true,
                tail_x: x * mw,
                tail_y: y * mh,
            });
        }
        Ok((off, tail, bw, bh, out))
    }

    fn linear(spec: &TextureSpec, element: u32, block: u32, mips: u32) -> ResourceLayout {
        let mut out = Vec::new();
        let mut off = 0u64;
        let depth = if spec.dimension == Dimension::D3 {
            spec.depth
        } else {
            1
        };
        for i in 0..mips {
            let wc = (spec.width.div_ceil(block) >> i).max(1);
            let hc = (spec.height.div_ceil(block) >> i).max(1);
            let d = (depth >> i).max(1);
            let pitch = align(u64::from(wc) * u64::from(element), 256);
            let slice = pitch * u64::from(hc);
            out.push(MipLayout {
                offset: off,
                size: slice * u64::from(d),
                slice_stride: slice,
                width_pixels: (spec.width >> i).max(1),
                height_pixels: (spec.height >> i).max(1),
                width_elements: wc,
                height_elements: hc,
                padded_width_elements: (pitch / u64::from(element)) as u32,
                padded_height_elements: hc,
                padded_depth: d,
                pitch_bytes: pitch as u32,
                in_tail: false,
                tail_x: 0,
                tail_y: 0,
            });
            off += slice * u64::from(d);
        }
        let slices = if spec.dimension == Dimension::D2 {
            spec.depth * if spec.cube { 6 } else { 1 }
        } else {
            1
        };
        // Linear arrays hold one whole chain per slice.
        let chain = off;
        ResourceLayout {
            size: align(chain * u64::from(slices), 256),
            alignment: 256,
            element_bytes: element,
            block,
            slices,
            mip_major: false,
            first_tail: mips,
            tile_width_elements: 0,
            tile_height_elements: 0,
            mips: out,
        }
    }

    /// Lays a texture out in its swizzle mode.
    pub fn compute(spec: &TextureSpec) -> Result<ResourceLayout, LayoutError> {
        let (width, height, depth) = (spec.width, spec.height, spec.depth.max(1));
        if width == 0
            || height == 0
            || spec.depth == 0
            || width > 16384
            || height > 16384
            || depth > 16384
        {
            return Err(LayoutError::Dimensions {
                width,
                height,
                depth: spec.depth,
            });
        }
        let (element, block) = format_info(spec.format).ok_or(LayoutError::Format(spec.format))?;
        let full = if spec.dimension == Dimension::D3 {
            levels(width, height, depth)
        } else {
            levels(width, height, 1)
        };
        let mips = if spec.mips == 0 { full } else { spec.mips };
        if mips > full {
            return Err(LayoutError::Mips {
                mips,
                width,
                height,
            });
        }
        if spec.cube && (spec.dimension != Dimension::D2 || width != height) {
            return Err(LayoutError::Unsupported(
                "cube textures that are not square 2D",
            ));
        }
        let Some(tile) = spec.swizzle.tile_bytes()? else {
            return Ok(linear(spec, element, block, mips));
        };
        match spec.dimension {
            Dimension::D3 => {
                if mips != 1 {
                    return Err(LayoutError::Unsupported(
                        "tiled volumes with more than one mip",
                    ));
                }
                if block != 1 || tile != TILE_4K {
                    return Err(LayoutError::Unsupported(
                        "block-compressed or 64 KiB tiled volumes",
                    ));
                }
                let (bw, bh, bd) = VOLUME_TILE[element.trailing_zeros() as usize];
                let (pw, ph, pd) = (
                    width.div_ceil(bw) * bw,
                    height.div_ceil(bh) * bh,
                    depth.div_ceil(bd) * bd,
                );
                let z_stride = u64::from(pw / bw) * u64::from(ph / bh) * u64::from(TILE_4K);
                let size = u64::from(pd / bd) * z_stride;
                Ok(ResourceLayout {
                    size,
                    alignment: TILE_4K,
                    element_bytes: element,
                    block,
                    slices: 1,
                    mip_major: false,
                    first_tail: 1,
                    tile_width_elements: bw,
                    tile_height_elements: bh,
                    mips: vec![MipLayout {
                        offset: 0,
                        size,
                        slice_stride: z_stride,
                        width_pixels: width,
                        height_pixels: height,
                        width_elements: width,
                        height_elements: height,
                        padded_width_elements: pw,
                        padded_height_elements: ph,
                        padded_depth: pd,
                        pitch_bytes: pw * element,
                        in_tail: false,
                        tail_x: 0,
                        tail_y: 0,
                    }],
                })
            }
            Dimension::D2 => {
                let (slice_size, tail, bw, bh, mut chain) =
                    compact(width, height, mips, element, block, tile)?;
                let slices = spec.depth * if spec.cube { 6 } else { 1 };
                if slices == 1 {
                    return Ok(ResourceLayout {
                        size: slice_size,
                        alignment: tile,
                        element_bytes: element,
                        block,
                        slices,
                        mip_major: false,
                        first_tail: tail,
                        tile_width_elements: bw,
                        tile_height_elements: bh,
                        mips: chain,
                    });
                }
                if spec.cube {
                    return Ok(ResourceLayout {
                        size: slice_size * u64::from(slices),
                        alignment: tile,
                        element_bytes: element,
                        block,
                        slices,
                        mip_major: false,
                        first_tail: tail,
                        tile_width_elements: bw,
                        tile_height_elements: bh,
                        mips: chain,
                    });
                }
                // Arrays: every level holds all slices, the shared tail first.
                let count = u64::from(slices);
                let mut off = if (tail as usize) < chain.len() {
                    count * u64::from(tile)
                } else {
                    0
                };
                for mip in chain[..tail as usize].iter_mut().rev() {
                    mip.offset = off;
                    mip.slice_stride = mip.size;
                    off += count * mip.size;
                }
                for mip in &mut chain[tail as usize..] {
                    mip.offset = 0;
                    mip.slice_stride = u64::from(tile);
                }
                Ok(ResourceLayout {
                    size: off,
                    alignment: tile,
                    element_bytes: element,
                    block,
                    slices,
                    mip_major: true,
                    first_tail: tail,
                    tile_width_elements: bw,
                    tile_height_elements: bh,
                    mips: chain,
                })
            }
        }
    }

    /// Render targets and unordered access views, which the console wants in the 64 KiB `R_X` mode.
    pub const BIND_RENDER_TARGET: u32 = 0x20;
    pub const BIND_DEPTH_STENCIL: u32 = 0x40;
    pub const BIND_UNORDERED_ACCESS: u32 = 0x80;

    /// A mode for a texture whose own mode the title leaves open.
    ///
    /// Measured: the package's textures are `4KB_S` (the small and middling ones) or `64KB_D`; what picks
    /// between them is not known, so a chain of one 4 KiB tile or a footprint under 64 KiB takes the first
    /// and larger ones the second. Surfaces the GPU writes take `64KB_R_X`, depth the `Z` mode: both are
    /// choices, not measurements.
    pub fn suggest_swizzle_mode(spec: &TextureSpec, bind_flags: u32) -> SwizzleMode {
        if bind_flags & BIND_DEPTH_STENCIL != 0 {
            return SwizzleMode(19);
        }
        if bind_flags & (BIND_RENDER_TARGET | BIND_UNORDERED_ACCESS) != 0 {
            return SwizzleMode::SW_64KB_R_X;
        }
        let small = TextureSpec {
            swizzle: SwizzleMode::SW_4KB_S,
            ..*spec
        };
        match compute(&small) {
            Ok(l) if l.size >= u64::from(TILE_64K) => SwizzleMode::SW_64KB_D,
            _ => SwizzleMode::SW_4KB_S,
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use super::layout::{self, Dimension, LayoutError, ResourceLayout, SwizzleMode, TextureSpec};
    use crate::trace::note;

    pub const S_OK: i32 = 0;
    pub const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
    pub const E_POINTER: i32 = 0x8000_4003_u32 as i32;
    pub const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

    static LAYOUT_NOTE: AtomicBool = AtomicBool::new(false);
    static SLOT_NOTE: AtomicBool = AtomicBool::new(false);
    static SUGGEST_NOTE: AtomicBool = AtomicBool::new(false);

    fn note_once(flag: &AtomicBool, text: &str) {
        if !flag.swap(true, Ordering::Relaxed) {
            note(text.to_string());
        }
    }

    const SLOTS: usize = 32;
    const MAX_MIPS: usize = 15;

    /// The console's `D3D12_RESOURCE_DESC`, with the swizzle mode where the PC's has its layout.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct TextureDesc {
        pub dimension: u32,
        pub _pad: u32,
        pub alignment: u64,
        pub width: u64,
        pub height: u32,
        pub depth_or_array: u16,
        pub mip_levels: u16,
        pub format: u32,
        pub sample_count: u32,
        pub sample_quality: u32,
        pub swizzle: u32,
        pub flags: u32,
    }

    /// `D3D12_RESOURCE_DIMENSION_TEXTURE3D`.
    const DIMENSION_3D: u32 = 4;
    const FLAG_RENDER_TARGET: u32 = 0x1;
    const FLAG_DEPTH_STENCIL: u32 = 0x2;
    const FLAG_UNORDERED_ACCESS: u32 = 0x4;

    impl TextureDesc {
        /// A swizzle field the caller wrote itself carries bit 8 (`0x100 | mode`); anything else is open.
        fn explicit_mode(&self) -> Option<SwizzleMode> {
            (self.swizzle & 0x100 != 0).then_some(SwizzleMode(self.swizzle & 0xFF))
        }

        /// The D3D12 flags as the bind flags the swizzle rule takes.
        fn bind_flags(&self) -> u32 {
            let mut bind = 0;
            if self.flags & FLAG_RENDER_TARGET != 0 {
                bind |= layout::BIND_RENDER_TARGET;
            }
            if self.flags & FLAG_DEPTH_STENCIL != 0 {
                bind |= layout::BIND_DEPTH_STENCIL;
            }
            if self.flags & FLAG_UNORDERED_ACCESS != 0 {
                bind |= layout::BIND_UNORDERED_ACCESS;
            }
            bind
        }

        fn spec(&self, swizzle: SwizzleMode) -> TextureSpec {
            TextureSpec {
                dimension: if self.dimension == DIMENSION_3D {
                    Dimension::D3
                } else {
                    Dimension::D2
                },
                width: u32::try_from(self.width).unwrap_or(u32::MAX),
                height: self.height,
                depth: u32::from(self.depth_or_array.max(1)),
                mips: u32::from(self.mip_levels),
                format: self.format,
                cube: false,
                swizzle,
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct MipOut {
        size_bytes: u64,
        offset_bytes: u64,
        slice_2d_size_bytes: u64,
        pitch_pixels: u32,
        pitch_bytes: u32,
        alignment_bytes: u32,
        padded_width_elements: u32,
        padded_height_elements: u32,
        padded_depth_or_array: u32,
    }

    #[repr(C)]
    struct PlaneOut {
        usage: u32,
        size_bytes: u64,
        base_offset_bytes: u64,
        base_alignment_bytes: u32,
        bytes_per_element: u32,
        mips: [MipOut; MAX_MIPS],
    }

    #[repr(C)]
    struct ResourceOut {
        size_bytes: u64,
        base_alignment_bytes: u32,
        planes: u32,
        mip_levels: u32,
        plane: [PlaneOut; 1],
    }

    #[repr(C)]
    pub struct Computer {
        vtable: *const [usize; SLOTS],
        references: AtomicU32,
        layout: Option<ResourceLayout>,
    }

    fn mip_out(layout: &ResourceLayout, i: usize) -> MipOut {
        let m = &layout.mips[i];
        MipOut {
            size_bytes: m.size
                * if layout.mip_major {
                    u64::from(layout.slices)
                } else {
                    1
                },
            offset_bytes: m.offset,
            slice_2d_size_bytes: m.slice_stride,
            pitch_pixels: m.padded_width_elements,
            pitch_bytes: m.pitch_bytes,
            alignment_bytes: layout.alignment,
            padded_width_elements: m.padded_width_elements,
            padded_height_elements: m.padded_height_elements,
            padded_depth_or_array: if layout.slices > 1 {
                layout.slices
            } else {
                m.padded_depth
            },
        }
    }

    extern "win64" fn add_ref(this: *mut Computer) -> u32 {
        // SAFETY: `this` is a computer this module made; the table keeps it alive until its last release.
        unsafe { (*this).references.fetch_add(1, Ordering::AcqRel) + 1 }
    }

    extern "win64" fn release(this: *mut Computer) -> u32 {
        // SAFETY: as above.
        let left = unsafe { (*this).references.fetch_sub(1, Ordering::AcqRel) } - 1;
        if left == 0 {
            // SAFETY: the last reference of a boxed computer.
            drop(unsafe { Box::from_raw(this) });
        }
        left
    }

    /// Slot 2: the whole resource's layout.
    extern "win64" fn get_resource_layout(this: *mut Computer, out: *mut ResourceOut) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        // SAFETY: `this` is a live computer.
        let Some(layout) = (unsafe { &(*this).layout }) else {
            return E_NOTIMPL;
        };
        if layout.mips.len() > MAX_MIPS {
            return E_INVALIDARG;
        }
        let mut plane = PlaneOut {
            usage: 0,
            size_bytes: layout.size,
            base_offset_bytes: 0,
            base_alignment_bytes: layout.alignment,
            bytes_per_element: layout.element_bytes,
            mips: [MipOut::default(); MAX_MIPS],
        };
        for i in 0..layout.mips.len() {
            plane.mips[i] = mip_out(layout, i);
        }
        let head = ResourceOut {
            size_bytes: layout.size,
            base_alignment_bytes: layout.alignment,
            planes: 1,
            mip_levels: layout.mips.len() as u32,
            plane: [plane],
        };
        let used = std::mem::offset_of!(ResourceOut, plane)
            + std::mem::offset_of!(PlaneOut, mips)
            + layout.mips.len() * std::mem::size_of::<MipOut>();
        // SAFETY: only the leading plane and the levels in use are written, so a caller's structure that is
        // smaller than ours in its unused tail is not overrun.
        unsafe {
            std::ptr::copy_nonoverlapping((&raw const head).cast::<u8>(), out.cast::<u8>(), used)
        };
        note_once(
            &LAYOUT_NOTE,
            "xg_xs: texture computer slot 2 wrote the public resource layout (structure unmeasured)",
        );
        S_OK
    }

    /// Slot 3, the public header's `GetMipLayout(plane, mip, out)`.
    extern "win64" fn get_mip_layout(
        this: *mut Computer,
        plane: u32,
        mip: u32,
        out: *mut MipOut,
    ) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        // SAFETY: `this` is a live computer.
        let Some(layout) = (unsafe { &(*this).layout }) else {
            return E_NOTIMPL;
        };
        if plane != 0 || mip as usize >= layout.mips.len() {
            return E_INVALIDARG;
        }
        // SAFETY: the caller's out structure.
        unsafe { out.write_unaligned(mip_out(layout, mip as usize)) };
        S_OK
    }

    extern "win64" fn not_implemented(_this: *mut Computer) -> i32 {
        note_once(
            &SLOT_NOTE,
            "xg_xs: a texture computer method with no known meaning was called; E_NOTIMPL",
        );
        E_NOTIMPL
    }

    fn vtable() -> &'static [usize; SLOTS] {
        static TABLE: std::sync::OnceLock<[usize; SLOTS]> = std::sync::OnceLock::new();
        TABLE.get_or_init(|| {
            let mut table = [not_implemented as *const () as usize; SLOTS];
            table[0] = add_ref as *const () as usize;
            table[1] = release as *const () as usize;
            table[2] = get_resource_layout as *const () as usize;
            table[3] = get_mip_layout as *const () as usize;
            table
        })
    }

    /// What a failed creation stores for callers that call through the object without checking.
    static EMPTY: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

    fn empty() -> *mut Computer {
        let at = *EMPTY.get_or_init(|| {
            Box::into_raw(Box::new(Computer {
                vtable: vtable(),
                references: AtomicU32::new(1 << 30),
                layout: None,
            })) as usize
        });
        at as *mut Computer
    }

    fn describe(error: &LayoutError) -> i32 {
        match error {
            LayoutError::Unsupported(_) => E_NOTIMPL,
            _ => E_INVALIDARG,
        }
    }

    /// # Safety
    /// `desc` points at a texture description and `out` at a pointer-sized slot.
    pub unsafe extern "win64" fn create_texture_computer(
        desc: *const TextureDesc,
        out: *mut *mut Computer,
    ) -> i32 {
        if out.is_null() {
            return E_POINTER;
        }
        // Two measured callers use `*out` whatever the result, so a failure leaves an object that answers
        // `E_NOTIMPL` instead of null.
        // SAFETY: `out` is writable.
        unsafe { *out = empty() };
        if desc.is_null() {
            return E_POINTER;
        }
        // SAFETY: the description is a stack structure of the caller's, at least this large.
        let desc = unsafe { desc.read_unaligned() };
        if desc.sample_count > 1 {
            note("xg_xs: multisampled texture layouts are not computed".into());
            return E_NOTIMPL;
        }
        let swizzle = desc.explicit_mode().unwrap_or_else(|| {
            layout::suggest_swizzle_mode(&desc.spec(SwizzleMode::SW_4KB_S), desc.bind_flags())
        });
        match layout::compute(&desc.spec(swizzle)) {
            Ok(layout) => {
                let computer = Box::into_raw(Box::new(Computer {
                    vtable: vtable(),
                    references: AtomicU32::new(1),
                    layout: Some(layout),
                }));
                // SAFETY: `out` is writable.
                unsafe { *out = computer };
                S_OK
            }
            Err(error) => {
                note(format!("xg_xs: XGCreateTextureComputer: {error}"));
                describe(&error)
            }
        }
    }

    /// # Safety
    /// `desc` points at a texture description and `mode` at a 32-bit slot.
    pub unsafe extern "win64" fn suggest_swizzle_mode(
        desc: *const TextureDesc,
        _flags: u32,
        mode: *mut u32,
    ) -> i32 {
        if desc.is_null() || mode.is_null() {
            return E_POINTER;
        }
        // SAFETY: as above.
        let desc = unsafe { desc.read_unaligned() };
        let spec = desc.spec(SwizzleMode::SW_4KB_S);
        if layout::format_info(spec.format).is_none() || spec.width == 0 || spec.height == 0 {
            return E_INVALIDARG;
        }
        // SAFETY: `mode` is writable.
        unsafe { *mode = layout::suggest_swizzle_mode(&spec, desc.bind_flags()).0 };
        note_once(
            &SUGGEST_NOTE,
            "xg_xs: XGSuggestSwizzleMode chose by its own rule (the console's is unmeasured)",
        );
        S_OK
    }
}

use crate::Shim;

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        vec![
            Shim {
                dll: "xg_xs.dll",
                name: "XGCreateTextureComputer",
                address: imp::create_texture_computer as *const () as usize,
            },
            Shim {
                dll: "xg_xs.dll",
                name: "XGSuggestSwizzleMode",
                address: imp::suggest_swizzle_mode as *const () as usize,
            },
        ]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
