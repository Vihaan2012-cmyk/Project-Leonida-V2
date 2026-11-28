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

/// The title's sampler settings as the builder at rva 0x012b96b0 reads them.
struct Settings {
    modes: [u32; 3],
    filter: u32,
    max_anisotropy: u32,
    compare: u32,
    border: [f32; 4],
}

impl Settings {
    fn new(filter: u32) -> Settings {
        Settings {
            modes: [1; 3],
            filter,
            max_anisotropy: 1,
            compare: 1,
            border: [0.0; 4],
        }
    }

    /// What the builder writes: the XY codes from filter bits 2 and 4 (3 for both when bit 6 is set), the mip and Z
    /// filters from bits 0 and 2, the reduction table indexed by the filter shifted right by 7, and the comparison
    /// function kept only when bit 7 is set.
    fn console(&self) -> ConsoleSampler {
        let f = self.filter;
        let xy = |bit: u32| {
            if f & 0x40 != 0 {
                3
            } else if f & bit != 0 {
                1
            } else {
                2
            }
        };
        ConsoleSampler {
            mag: xy(4),
            min: xy(16),
            mip: if f & 1 != 0 { 2 } else { 1 },
            reduction: [0, 0, 1, 2][((f >> 7) & 3) as usize],
            clamp: self.modes,
            min_lod: 0.0,
            max_lod: 12.0,
            lod_bias: 0.0,
            max_anisotropy: self.max_anisotropy & 0x1F,
            compare: if f & 0x80 != 0 { self.compare } else { 1 },
            border_color: self.border,
        }
    }

    fn sampler(&self) -> Sampler {
        from_console(&self.console(), &mut BorderTable::default())
    }
}

#[test]
fn the_title_s_trilinear_wrap_sampler_stays_trilinear_and_wrapping() {
    let s = Settings::new(0x15).sampler();
    assert_eq!((s.mag, s.min), (Filter::Bilinear, Filter::Bilinear));
    assert_eq!(s.mip, MipFilter::Linear);
    assert_eq!(s.max_anisotropy, 1);
    assert_eq!(
        [s.address_u, s.address_v, s.address_w],
        [AddressMode::Wrap; 3]
    );
    assert_eq!(s.reduction, Reduction::Blend);
    assert_eq!(s.compare, Compare::Never);
    assert!(!s.compare_never);
    assert_eq!(s.max_lod, 12.0);
}

#[test]
fn a_point_sampler_has_point_filters_and_a_point_mip_filter() {
    let s = Settings::new(0).sampler();
    assert_eq!((s.mag, s.min), (Filter::AnisoPoint, Filter::AnisoPoint));
    assert!(!s.mag.linear() && !s.min.linear());
    assert_eq!(s.mip, MipFilter::Point);
    assert_eq!(s.max_anisotropy, 1);
}

#[test]
fn mixed_filters_follow_their_own_bits() {
    let s = Settings::new(0x10).sampler();
    assert!(!s.mag.linear() && s.min.linear());
    assert_eq!(s.mip, MipFilter::Point);
    let s = Settings::new(0x05).sampler();
    assert!(s.mag.linear() && !s.min.linear());
    assert_eq!(s.mip, MipFilter::Linear);
}

#[test]
fn the_title_s_address_modes_are_d3d12_numbered() {
    let modes = |m: [u32; 3]| {
        let mut set = Settings::new(0x15);
        set.modes = m;
        let s = set.sampler();
        [s.address_u, s.address_v, s.address_w]
    };
    assert_eq!(modes([1, 1, 1]), [AddressMode::Wrap; 3]);
    assert_eq!(
        modes([2, 3, 5]),
        [
            AddressMode::Mirror,
            AddressMode::ClampLastTexel,
            AddressMode::MirrorOnceLastTexel
        ]
    );
    assert_eq!(modes([4, 1, 3])[0], AddressMode::ClampBorder);
}

#[test]
fn anisotropic_samplers_take_the_ratio_the_title_set() {
    for ratio in [2, 4, 8, 16] {
        let mut set = Settings::new(0x55);
        set.max_anisotropy = ratio;
        let s = set.sampler();
        assert_eq!(
            (s.mag, s.min),
            (Filter::AnisoBilinear, Filter::AnisoBilinear)
        );
        assert_eq!(s.mip, MipFilter::Linear);
        assert_eq!(s.max_anisotropy, ratio);
    }
    let mut set = Settings::new(0x15);
    set.max_anisotropy = 8;
    assert_eq!(set.sampler().max_anisotropy, 1);
}

#[test]
fn a_comparison_filter_carries_its_function_and_others_never_compare() {
    let mut set = Settings::new(0x94);
    set.compare = 5;
    set.modes = [4; 3];
    set.border = [1.0; 4];
    let s = set.sampler();
    assert_eq!(s.compare, Compare::Greater);
    assert_eq!(s.border, Border::OpaqueWhite);
    assert_eq!(s.reduction, Reduction::Blend);
    for (func, expected) in [
        (2, Compare::Less),
        (3, Compare::Equal),
        (4, Compare::LessEqual),
        (6, Compare::NotEqual),
        (7, Compare::GreaterEqual),
        (8, Compare::Always),
    ] {
        set.compare = func;
        assert_eq!(set.sampler().compare, expected);
    }
    let mut plain = Settings::new(0x15);
    plain.compare = 8;
    assert_eq!(plain.sampler().compare, Compare::Never);
}

#[test]
fn minimum_and_maximum_filters_map_through_the_reduction_table() {
    assert_eq!(
        Settings::new(0x100 | 0x15).sampler().reduction,
        Reduction::Minimum
    );
    assert_eq!(
        Settings::new(0x180 | 0x15).sampler().reduction,
        Reduction::Maximum
    );
    assert_eq!(
        Settings::new(0x80 | 0x15).sampler().reduction,
        Reduction::Blend
    );
}

#[test]
fn custom_border_colours_go_to_the_table() {
    let mut set = Settings::new(0x15);
    set.modes = [4, 4, 1];
    set.border = [0.25, 0.5, 0.75, 1.0];
    let mut borders = BorderTable::default();
    let s = from_console(&set.console(), &mut borders);
    assert_eq!(s.border, Border::Table);
    assert_eq!(borders.get(s.border_index), Some([0.25, 0.5, 0.75, 1.0]));
    set.border = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(set.sampler().border, Border::OpaqueBlack);
    set.modes = [3; 3];
    set.border = [0.25; 4];
    assert_eq!(set.sampler().border, Border::TransparentBlack);
}
