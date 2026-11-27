use super::desc::*;
use crate::descriptor::format::dxgi;

#[test]
fn a_buffer_is_rounded_to_64_kib() {
    let info = ResourceDesc::buffer(100_000).allocation_info().unwrap();
    assert_eq!(
        info,
        AllocationInfo {
            size: 131072,
            alignment: 65536
        }
    );
    assert_eq!(
        ResourceDesc::buffer(65536).allocation_info().unwrap().size,
        65536
    );
}

#[test]
fn a_buffer_with_texture_fields_is_refused() {
    let mut desc = ResourceDesc::buffer(256);
    desc.format = dxgi::R8G8B8A8_UNORM;
    assert_eq!(desc.validate(), Err(DescError::Buffer));
}

#[test]
fn zero_mips_means_the_whole_chain() {
    assert_eq!(
        ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 256, 64, 0).mips(),
        9
    );
    assert_eq!(
        ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 256, 64, 3).mips(),
        3
    );
    assert_eq!(
        ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 4, 4, 9).mips(),
        3
    );
    let mut volume = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 8, 8, 0);
    volume.dimension = Dimension::Texture3D;
    volume.depth_or_array_size = 64;
    assert_eq!(volume.mips(), 7);
    assert_eq!(volume.array_size(), 1);
    assert_eq!(volume.depth(), 64);
}

#[test]
fn a_tiled_texture_takes_its_mip_chain_in_tiles() {
    let desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 256, 256, 0);
    let info = desc.allocation_info().unwrap();
    assert_eq!(info.size, 65536 + 65536 + 262144);
    assert_eq!(info.alignment, 65536);
    let mut array = desc;
    array.depth_or_array_size = 6;
    assert_eq!(
        array.allocation_info().unwrap().size,
        6 * (65536 + 65536 + 262144)
    );
}

#[test]
fn compressed_textures_count_blocks() {
    let desc = ResourceDesc::texture_2d(dxgi::BC7_UNORM, 512, 512, 1);
    let layout = desc.compact_layout().unwrap();
    assert_eq!(layout.mips[0].blocks_w, 128);
    assert_eq!(desc.allocation_info().unwrap().size, 4 * 65536);
}

#[test]
fn a_row_major_texture_uses_copyable_footprints() {
    let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 100, 50, 1);
    desc.layout = Layout::RowMajor;
    assert_eq!(desc.allocation_info().unwrap().size, 65536);
    desc.height = 1000;
    assert_eq!(
        desc.allocation_info().unwrap().size,
        (512u64 * 1000).next_multiple_of(65536)
    );
}

#[test]
fn a_multisampled_texture_is_aligned_to_4_mib() {
    let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 64, 64, 1);
    desc.sample_count = 4;
    desc.layout = Layout::Undefined64K;
    let info = desc.allocation_info().unwrap();
    assert_eq!(info.alignment, 4 * 1024 * 1024);
    assert_eq!(info.size, 4 * 1024 * 1024);
}

#[test]
fn unknown_formats_and_empty_textures_are_refused() {
    let desc = ResourceDesc::texture_2d(200, 16, 16, 1);
    assert_eq!(desc.validate(), Err(DescError::Format(200)));
    let desc = ResourceDesc::texture_2d(dxgi::R8_UNORM, 0, 16, 1);
    assert!(matches!(desc.validate(), Err(DescError::Empty(0, 16))));
}

#[test]
fn raw_layout_values_map_to_the_enum() {
    assert_eq!(Layout::from_raw(0x101), Some(Layout::ConsoleTiled));
    assert_eq!(Layout::from_raw(1), Some(Layout::RowMajor));
    assert_eq!(Layout::from_raw(7), None);
    assert_eq!(Dimension::from_raw(3), Some(Dimension::Texture2D));
    assert_eq!(Dimension::from_raw(0), None);
}
