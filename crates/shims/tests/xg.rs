//! The texture layout computer against the package's measured chains, and the two exports.

use shims::xg::layout::{
    Dimension, LayoutError, SwizzleMode, TextureSpec, compute, suggest_swizzle_mode,
};

fn tex(width: u32, height: u32, mips: u32, format: u32, mode: SwizzleMode) -> TextureSpec {
    TextureSpec::texture_2d(width, height, mips, format, mode)
}

#[test]
fn single_mip_is_padded_to_one_tile() {
    let l = compute(&tex(64, 64, 1, 71, SwizzleMode::SW_4KB_S)).unwrap();
    assert_eq!(l.size, 4096);
    assert_eq!(l.first_tail, 1);
    assert_eq!(l.mips[0].offset, 0);
    assert_eq!(l.mips[0].size, 4096);
    assert!(!l.mips[0].in_tail);
    assert_eq!(l.alignment, 4096);
}

#[test]
fn sub_tile_mips_share_the_tail() {
    let l = compute(&tex(8, 4, 2, 71, SwizzleMode::SW_4KB_S)).unwrap();
    assert_eq!(l.size, 4096);
    assert_eq!(l.first_tail, 0);
    assert!(l.mips.iter().all(|m| m.in_tail));
    let origins: Vec<_> = l.mips.iter().map(|m| (m.tail_x, m.tail_y)).collect();
    assert_eq!(origins, [(16, 0), (8, 8)]);
}

#[test]
fn uncompressed_chain_is_sized_by_pixels() {
    let l = compute(&tex(128, 128, 8, 28, SwizzleMode::SW_4KB_S)).unwrap();
    assert_eq!(l.element_bytes, 4);
    assert_eq!(l.size, 90112);
    assert_eq!(l.first_tail, 3);
    let regions: Vec<_> = l.mips[..3].iter().map(|m| m.size).collect();
    assert_eq!(regions, [65536, 16384, 4096]);
    assert_eq!(l.mips[0].width_elements, 128);
    assert_eq!(l.mips[0].offset, 24576);
    assert_eq!(l.mips[1].offset, 8192);
    assert_eq!(l.mips[2].offset, 4096);
}

#[test]
fn size_is_standalone_regions_plus_one_tail_tile() {
    for (w, h, f, m) in [
        (256, 256, 71, 7),
        (1024, 1024, 98, 9),
        (128, 256, 98, 6),
        (512, 256, 28, 9),
    ] {
        let l = compute(&tex(w, h, m, f, SwizzleMode::SW_4KB_S)).unwrap();
        let standalone: u64 = l.mips.iter().filter(|m| !m.in_tail).map(|m| m.size).sum();
        let tail = if (l.first_tail as usize) < l.mips.len() {
            4096
        } else {
            0
        };
        assert_eq!(l.size, standalone + tail, "{w}x{h} format {f} mips {m}");
    }
    assert_eq!(
        compute(&tex(256, 256, 7, 71, SwizzleMode::SW_4KB_S))
            .unwrap()
            .size,
        45056
    );
}

#[test]
fn the_package_mode_zero_is_a_64k_tile() {
    let l = compute(&tex(512, 512, 1, 28, SwizzleMode::SW_64KB_D)).unwrap();
    assert_eq!(l.alignment, 65536);
    assert_eq!(l.size, 512 * 512 * 4);
    assert_eq!((l.tile_width_elements, l.tile_height_elements), (128, 128));
}

#[test]
fn full_chain_when_mip_count_is_zero() {
    let l = compute(&tex(256, 256, 0, 71, SwizzleMode::SW_4KB_S)).unwrap();
    assert_eq!(l.mips.len(), 9);
}

#[test]
fn arrays_are_mip_major_with_the_tail_first() {
    let mut spec = tex(256, 256, 6, 83, SwizzleMode::SW_64KB_D);
    spec.depth = 3;
    let l = compute(&spec).unwrap();
    assert!(l.mip_major);
    assert_eq!(l.slices, 3);
    assert_eq!(l.first_tail, 1);
    assert_eq!(l.size, 3 * 131072);
    assert_eq!(l.mips[1].offset, 0);
    assert_eq!(l.mips[1].slice_stride, 65536);
    assert_eq!(l.mips[0].offset, 3 * 65536);
    assert_eq!(l.mips[0].slice_stride, l.mips[0].size);
}

#[test]
fn cubemaps_hold_six_slice_major_faces() {
    let mut spec = tex(128, 128, 6, 71, SwizzleMode::SW_4KB_S);
    spec.cube = true;
    let l = compute(&spec).unwrap();
    assert_eq!(l.slices, 6);
    assert!(!l.mip_major);
    assert_eq!(l.size, 73728);
    assert_eq!(l.size / 6, 12288);
}

#[test]
fn volumes_use_three_dimensional_tiles() {
    let volume = |w, h, d, f| {
        compute(&TextureSpec {
            dimension: Dimension::D3,
            width: w,
            height: h,
            depth: d,
            mips: 1,
            format: f,
            cube: false,
            swizzle: SwizzleMode::SW_4KB_S,
        })
        .unwrap()
    };
    let v = volume(256, 32, 8, 54);
    assert_eq!((v.size, v.mips[0].padded_depth), (262144, 16));
    let v = volume(32, 32, 32, 10);
    assert_eq!((v.size, v.mips[0].padded_depth), (262144, 32));
    assert_eq!(volume(32, 64, 32, 10).size, 524288);
}

#[test]
fn impossible_textures_are_typed_errors() {
    let mode = SwizzleMode::SW_4KB_S;
    assert!(matches!(
        compute(&tex(64, 64, 20, 71, mode)),
        Err(LayoutError::Mips { .. })
    ));
    assert!(matches!(
        compute(&tex(1, 1, 2, 71, mode)),
        Err(LayoutError::Mips { .. })
    ));
    assert!(matches!(
        compute(&tex(0, 64, 1, 71, mode)),
        Err(LayoutError::Dimensions { .. })
    ));
    assert!(matches!(
        compute(&tex(64, 64, 1, 9999, mode)),
        Err(LayoutError::Format(9999))
    ));
    assert!(matches!(
        compute(&tex(64, 64, 1, 6, mode)),
        Err(LayoutError::Unsupported(_))
    ));
    assert!(matches!(
        compute(&tex(64, 64, 1, 28, SwizzleMode(30))),
        Err(LayoutError::Unsupported(_))
    ));
    let mut volume = tex(32, 32, 2, 10, mode);
    volume.dimension = Dimension::D3;
    volume.depth = 32;
    assert!(matches!(compute(&volume), Err(LayoutError::Unsupported(_))));
}

#[test]
fn linear_rows_are_256_byte_aligned() {
    let l = compute(&tex(100, 10, 1, 28, SwizzleMode::LINEAR)).unwrap();
    assert_eq!(l.mips[0].pitch_bytes, 512);
    assert_eq!(l.size, 5120);
    assert_eq!(l.alignment, 256);
}

#[test]
fn suggestions_follow_use_and_size() {
    let small = tex(64, 64, 1, 71, SwizzleMode::LINEAR);
    assert_eq!(suggest_swizzle_mode(&small, 0), SwizzleMode::SW_4KB_S);
    let large = tex(2048, 2048, 0, 98, SwizzleMode::LINEAR);
    assert_eq!(suggest_swizzle_mode(&large, 0), SwizzleMode::SW_64KB_D);
    assert_eq!(suggest_swizzle_mode(&small, 0x20), SwizzleMode::SW_64KB_R_X);
    assert_eq!(suggest_swizzle_mode(&small, 0x80), SwizzleMode::SW_64KB_R_X);
    assert_ne!(suggest_swizzle_mode(&small, 0x40), SwizzleMode::SW_4KB_S);
}

#[cfg(windows)]
mod exports {
    #![allow(unsafe_code)]

    const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
    const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

    type Create = unsafe extern "win64" fn(*const [u32; 15], *mut *mut usize) -> i32;
    type Suggest = unsafe extern "win64" fn(*const [u32; 15], u32, *mut u32) -> i32;
    type Release = unsafe extern "win64" fn(*mut usize) -> u32;
    type GetLayout = unsafe extern "win64" fn(*mut usize, *mut u8) -> i32;
    type Unknown = unsafe extern "win64" fn(*mut usize, usize, usize, usize, usize, usize) -> i32;

    fn shim<T: Copy>(name: &str) -> T {
        let address = shims::builtin()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} registered"))
            .address;
        assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        // SAFETY: `T` is the function pointer type of the shim's shape.
        unsafe { std::mem::transmute_copy(&address) }
    }

    /// The console's `D3D12_RESOURCE_DESC` as the builder at rva 0x4ecc940 fills it: dimension (3 = 2D), padding,
    /// alignment, width (64-bit), height, depth or array and mips (16-bit each), format, sample count and
    /// quality, the swizzle field and the D3D12 flags (`d3d12_flags`: 1 render target).
    fn desc(
        width: u32,
        height: u32,
        mips: u32,
        format: u32,
        d3d12_flags: u32,
        swizzle: u32,
    ) -> [u32; 15] {
        [
            3,
            0,
            0,
            0,
            width,
            0,
            height,
            1 | (mips << 16),
            format,
            1,
            0,
            swizzle,
            d3d12_flags,
            0,
            0,
        ]
    }

    fn slot<T: Copy>(computer: *mut usize, index: usize) -> T {
        // SAFETY: the object's first word is its vtable, which has 32 slots.
        unsafe {
            let table = *computer as *const usize;
            std::mem::transmute_copy(&*table.add(index))
        }
    }

    #[test]
    fn a_computer_reports_the_chain_layout() {
        let create: Create = shim("XGCreateTextureComputer");
        let mut computer = std::ptr::null_mut();
        let d = desc(128, 128, 8, 28, 0, 0x100 | 4);
        // SAFETY: valid description and out pointer.
        assert_eq!(unsafe { create(&d, &mut computer) }, 0);
        assert!(!computer.is_null());
        let mut out = vec![0u8; 4096];
        let layout: GetLayout = slot(computer, 2);
        // SAFETY: the buffer is larger than the structure.
        assert_eq!(unsafe { layout(computer, out.as_mut_ptr()) }, 0);
        let u64_at = |at: usize| u64::from_le_bytes(out[at..at + 8].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        assert_eq!(u64_at(0), 90112, "size");
        assert_eq!(u32_at(8), 4096, "alignment");
        assert_eq!(u32_at(12), 1, "planes");
        assert_eq!(u32_at(16), 8, "mip levels");
        assert_eq!(u64_at(32), 90112, "plane size");
        assert_eq!(u32_at(52), 4, "bytes per element");
        assert_eq!(u64_at(56 + 8), 24576, "mip 0 offset");
        let release: Release = slot(computer, 1);
        // SAFETY: the one reference this call made.
        assert_eq!(unsafe { release(computer) }, 0);
    }

    #[test]
    fn unknown_slots_answer_not_implemented_and_write_nothing() {
        let create: Create = shim("XGCreateTextureComputer");
        let mut computer = std::ptr::null_mut();
        let d = desc(64, 64, 1, 71, 0, 0x104);
        // SAFETY: as above.
        assert_eq!(unsafe { create(&d, &mut computer) }, 0);
        for index in [8, 10] {
            let call: Unknown = slot(computer, index);
            // SAFETY: the shim ignores its arguments.
            assert_eq!(unsafe { call(computer, 0, 0, 0, 0, 0) }, E_NOTIMPL);
        }
        let release: Release = slot(computer, 1);
        // SAFETY: as above.
        unsafe { release(computer) };
    }

    #[test]
    fn a_failed_creation_still_stores_a_usable_object() {
        let create: Create = shim("XGCreateTextureComputer");
        let mut computer = std::ptr::null_mut();
        let d = desc(0, 64, 1, 71, 0, 0x104);
        // SAFETY: as above.
        assert_eq!(unsafe { create(&d, &mut computer) }, E_INVALIDARG);
        assert!(!computer.is_null());
        let layout: GetLayout = slot(computer, 2);
        let mut out = [0u8; 64];
        // SAFETY: a buffer; the empty object writes nothing.
        assert_eq!(unsafe { layout(computer, out.as_mut_ptr()) }, E_NOTIMPL);
        assert_eq!(out, [0u8; 64]);
        let release: Release = slot(computer, 1);
        // SAFETY: the empty object is never freed.
        unsafe { release(computer) };
    }

    #[test]
    fn a_suggested_mode_is_written_to_the_callers_field() {
        let suggest: Suggest = shim("XGSuggestSwizzleMode");
        let mut mode = 0u32;
        let d = desc(64, 64, 1, 71, 0, 0);
        // SAFETY: valid pointers.
        assert_eq!(unsafe { suggest(&d, 0, &mut mode) }, 0);
        assert_eq!(mode, 4);
        let d = desc(1920, 1080, 1, 28, 1, 0);
        // SAFETY: as above.
        assert_eq!(unsafe { suggest(&d, 0, &mut mode) }, 0);
        assert_eq!(mode, 22);
        let d = desc(0, 0, 1, 28, 0, 0);
        mode = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { suggest(&d, 0, &mut mode) }, E_INVALIDARG);
        assert_eq!(mode, 0);
    }
}

/// Small textures in the 256-byte modes, which the title asks for: tiles of 256 bytes, every level its own.
#[test]
fn the_256_byte_modes_pad_each_level_to_256_byte_tiles() {
    // 32x32 RGBA8 (4 bytes per texel) in a 256-byte mode: 8x8 tiles, levels of 4096, 1024, 256, 256, 256, 256.
    let l = compute(&tex(32, 32, 0, 28, SwizzleMode(1))).unwrap();
    assert_eq!(l.alignment, 256);
    assert_eq!((l.tile_width_elements, l.tile_height_elements), (8, 8));
    assert_eq!(l.first_tail, l.mips.len() as u32, "no shared tail");
    let sizes: Vec<u64> = l.mips.iter().map(|m| m.size).collect();
    assert_eq!(sizes, vec![4096, 1024, 256, 256, 256, 256]);
    assert_eq!(l.size, sizes.iter().sum::<u64>());
    // 1-byte texels: 16x16 tiles.
    let r8 = compute(&tex(16, 16, 1, 61, SwizzleMode(2))).unwrap();
    assert_eq!((r8.tile_width_elements, r8.tile_height_elements), (16, 16));
    assert_eq!(r8.size, 256);
}
