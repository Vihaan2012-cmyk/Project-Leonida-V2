use super::gfx10::*;
use super::sampler::*;

fn words(desc: SamplerDesc) -> SamplerWords {
    encode_sampler(&sampler(&desc, &mut BorderTable::default()))
}

#[test]
fn trilinear_clamped() {
    let w = words(SamplerDesc {
        filter: 0x15,
        address_u: 3,
        address_v: 3,
        address_w: 3,
        max_lod: 15.0,
        ..SamplerDesc::default()
    });
    assert_eq!(w, [0x92, 0xF00 << 12, (1 << 20) | (1 << 22) | (2 << 26), 0]);
}

#[test]
fn anisotropic_sixteen() {
    let w = words(SamplerDesc {
        filter: 0x55,
        max_anisotropy: 16,
        max_lod: 15.0,
        ..SamplerDesc::default()
    });
    assert_eq!(
        w,
        [4 << 9, 0xF00 << 12, (3 << 20) | (3 << 22) | (2 << 26), 0]
    );
    let odd = sampler(
        &SamplerDesc {
            filter: 0x55,
            max_anisotropy: 5,
            ..SamplerDesc::default()
        },
        &mut BorderTable::default(),
    );
    assert_eq!(odd.max_anisotropy, 8);
}

#[test]
fn a_shadow_sampler_compares_against_a_white_border() {
    let w = words(SamplerDesc {
        filter: 0x94,
        address_u: 4,
        address_v: 4,
        address_w: 4,
        comparison_func: 4,
        border_color: [1.0; 4],
        max_lod: 0.0,
        ..SamplerDesc::default()
    });
    assert_eq!(w, [0x31B6, 0, (1 << 20) | (1 << 22) | (1 << 26), 2 << 30]);
}

#[test]
fn a_never_comparison_is_told_apart_from_no_comparison() {
    let never = words(SamplerDesc {
        filter: 0x80,
        comparison_func: 1,
        ..SamplerDesc::default()
    });
    let plain = words(SamplerDesc::default());
    assert_ne!(never, plain);
    assert_eq!(never[0] & (7 << 12), 0);
}

#[test]
fn address_modes_map_to_the_console_clamp_modes() {
    let modes = |m: u32| {
        sampler(
            &SamplerDesc {
                address_u: m,
                ..SamplerDesc::default()
            },
            &mut BorderTable::default(),
        )
        .address_u
    };
    assert_eq!(modes(1), AddressMode::Wrap);
    assert_eq!(modes(2), AddressMode::Mirror);
    assert_eq!(modes(3), AddressMode::ClampLastTexel);
    assert_eq!(modes(4), AddressMode::ClampBorder);
    assert_eq!(modes(5), AddressMode::MirrorOnceLastTexel);
}

#[test]
fn custom_border_colours_get_an_index_and_are_kept() {
    let mut borders = BorderTable::default();
    let desc = SamplerDesc {
        address_u: 4,
        border_color: [0.25, 0.5, 0.75, 1.0],
        ..SamplerDesc::default()
    };
    let a = sampler(&desc, &mut borders);
    let b = sampler(&desc, &mut borders);
    let c = sampler(
        &SamplerDesc {
            border_color: [1.0, 0.0, 0.0, 1.0],
            ..desc
        },
        &mut borders,
    );
    assert_eq!((a.border, a.border_index), (Border::Table, 0));
    assert_eq!(b.border_index, 0);
    assert_eq!(c.border_index, 1);
    assert_eq!(borders.get(1), Some([1.0, 0.0, 0.0, 1.0]));
    let unused = sampler(
        &SamplerDesc {
            address_u: 1,
            border_color: [0.3; 4],
            ..SamplerDesc::default()
        },
        &mut borders,
    );
    assert_eq!(unused.border, Border::TransparentBlack);
}

#[test]
fn minimum_and_maximum_filters_and_integer_borders() {
    let min = sampler(
        &SamplerDesc {
            filter: 0x100 | 0x15,
            ..SamplerDesc::default()
        },
        &mut BorderTable::default(),
    );
    assert_eq!(min.reduction, Reduction::Minimum);
    let max = sampler(
        &SamplerDesc {
            filter: 0x180 | 0x15,
            flags: FLAG_UINT_BORDER_COLOR,
            ..SamplerDesc::default()
        },
        &mut BorderTable::default(),
    );
    assert_eq!(max.reduction, Reduction::Maximum);
    assert!(max.integer_border);
}
