use super::gfx10::*;

fn changed(image: Image) -> ImageWords {
    encode_image(&image)
}

#[test]
fn an_empty_image_is_a_2d_identity_swizzle_descriptor() {
    assert_eq!(
        encode_image(&Image::default()),
        [0, 0, 0, 0x9000_0FAC, 0, 0, 0, 0]
    );
}

#[test]
fn each_image_field_sits_at_its_bit_position() {
    let base = Image::default();
    let plain = encode_image(&base);
    let only = |image: Image, word: usize, bits: u32| {
        assert_eq!(changed(image)[word], plain[word] ^ bits);
    };
    assert_eq!(
        changed(Image {
            address: 0xAB12_3456_7800,
            ..base
        }),
        [0x1234_5678, 0xAB, 0, 0x9000_0FAC, 0, 0, 0, 0]
    );
    only(
        Image {
            format: 0x1FF,
            ..base
        },
        1,
        0x1FF0_0000,
    );
    only(
        Image {
            width: 16384,
            ..base
        },
        1,
        0xC000_0000,
    );
    only(
        Image {
            width: 16384,
            ..base
        },
        2,
        0xFFF,
    );
    only(
        Image {
            height: 16384,
            ..base
        },
        2,
        0x0FFF_C000,
    );
    only(
        Image {
            min_lod: 2.5,
            ..base
        },
        1,
        0x280 << 8,
    );
    only(
        Image {
            first_mip: 3,
            mip_count: 4,
            resource_mips: 12,
            ..base
        },
        3,
        0x63000,
    );
    only(
        Image {
            first_mip: 3,
            mip_count: 4,
            resource_mips: 12,
            ..base
        },
        5,
        0xB0,
    );
    only(
        Image {
            ty: ImageType::D2Array,
            first_layer: 5,
            layer_count: 7,
            ..base
        },
        4,
        0x0005_000B,
    );
    only(
        Image {
            ty: ImageType::D2Array,
            first_layer: 5,
            layer_count: 7,
            ..base
        },
        3,
        0x9000_0000 ^ 0xD000_0000,
    );
    only(
        Image {
            ty: ImageType::D3,
            depth: 8192,
            ..base
        },
        4,
        0x1FFF,
    );
}

#[test]
fn a_multisampled_image_keeps_its_sample_count_in_the_level_fields() {
    let words = encode_image(&Image {
        ty: ImageType::D2Msaa,
        samples: 8,
        ..Image::default()
    });
    assert_eq!(words[3], 0xE003_0FAC);
    assert_eq!(words[5], 0x30);
    let back = decode_image(&words).unwrap();
    assert_eq!((back.samples, back.mip_count), (8, 1));
}

#[test]
fn swizzle_goes_to_the_low_twelve_bits() {
    let words = encode_image(&Image {
        swizzle: [SELECT_ONE, SELECT_ZERO, SELECT_W, SELECT_X],
        ..Image::default()
    });
    assert_eq!(words[3] & 0xFFF, 1 | (7 << 6) | (4 << 9));
}

#[test]
fn an_rgba8_view_has_format_56_in_word_one() {
    let words = encode_image(&Image {
        format: 56,
        ..Image::default()
    });
    assert_eq!(words[1], 0x0380_0000);
    assert_eq!(
        shader_translate::images::number(&words),
        shader_translate::images::Number::Float
    );
}

#[test]
fn images_round_trip_including_the_largest_address() {
    let samples = [
        Image {
            address: 0x1_2345_6700,
            format: 56,
            width: 1920,
            height: 1080,
            mip_count: 11,
            resource_mips: 11,
            ..Image::default()
        },
        Image {
            address: 0xFFFF_FFFF_FF00,
            format: 0x1FF,
            width: 16384,
            height: 16384,
            first_mip: 3,
            mip_count: 4,
            resource_mips: 15,
            min_lod: 14.5,
            ..Image::default()
        },
        Image {
            ty: ImageType::Cube,
            format: 181,
            width: 256,
            height: 256,
            first_layer: 6,
            layer_count: 12,
            mip_count: 9,
            resource_mips: 9,
            ..Image::default()
        },
        Image {
            ty: ImageType::D3,
            width: 64,
            height: 64,
            depth: 33,
            swizzle: [SELECT_Z, SELECT_Y, SELECT_X, SELECT_ONE],
            ..Image::default()
        },
    ];
    for image in samples {
        assert_eq!(decode_image(&encode_image(&image)), Some(image));
    }
}

#[test]
fn contradictory_image_words_are_refused() {
    let good = encode_image(&Image {
        first_mip: 2,
        mip_count: 3,
        resource_mips: 5,
        first_layer: 3,
        layer_count: 2,
        ty: ImageType::D2Array,
        ..Image::default()
    });
    assert!(decode_image(&good).is_some());
    let mut bad = good;
    bad[3] &= 0x0FFF_FFFF;
    assert!(!is_image(&bad) && decode_image(&bad).is_none());
    let mut bad = good;
    bad[3] = (bad[3] & !0x7) | 2;
    assert!(decode_image(&bad).is_none());
    let mut bad = good;
    bad[3] &= !0xF0000;
    assert!(decode_image(&bad).is_none());
    let mut bad = good;
    bad[4] &= !0x1FFF;
    assert!(decode_image(&bad).is_none());
    let mut bad = encode_image(&Image {
        ty: ImageType::D2Msaa,
        samples: 4,
        ..Image::default()
    });
    bad[3] |= 0x1000;
    assert!(decode_image(&bad).is_none());
    let cube = encode_image(&Image {
        ty: ImageType::Cube,
        layer_count: 5,
        ..Image::default()
    });
    assert!(decode_image(&cube).is_none());
}

fn one_field(sampler: Sampler) -> SamplerWords {
    encode_sampler(&sampler)
}

#[test]
fn each_sampler_field_sits_at_its_bit_position() {
    let d = Sampler::default();
    assert_eq!(one_field(d), [0; 4]);
    assert_eq!(one_field(Sampler { min_lod: 1.5, ..d }), [0, 0x180, 0, 0]);
    assert_eq!(
        one_field(Sampler { max_lod: 15.0, ..d }),
        [0, 0xF00 << 12, 0, 0]
    );
    assert_eq!(
        one_field(Sampler {
            max_lod: 1000.0,
            ..d
        }),
        [0, 0xF00 << 12, 0, 0]
    );
    assert_eq!(one_field(Sampler { lod_bias: 1.0, ..d }), [0, 0, 0x100, 0]);
    assert_eq!(
        one_field(Sampler {
            lod_bias: -1.5,
            ..d
        }),
        [0, 0, 0x3E80, 0]
    );
    assert_eq!(
        one_field(Sampler {
            lod_bias: 100.0,
            ..d
        }),
        [0, 0, 0x1F00, 0]
    );
    assert_eq!(
        one_field(Sampler {
            lod_bias: -100.0,
            ..d
        }),
        [0, 0, 0x2000, 0]
    );
    assert_eq!(
        one_field(Sampler {
            border: Border::Table,
            border_index: 0xABC,
            ..d
        }),
        [0, 0, 0, (3 << 30) | 0xABC]
    );
}

#[test]
fn a_trilinear_sixteen_times_anisotropic_sampler() {
    let words = encode_sampler(&Sampler {
        max_anisotropy: 16,
        mag: Filter::AnisoBilinear,
        min: Filter::AnisoBilinear,
        mip: MipFilter::Linear,
        max_lod: 15.0,
        ..Sampler::default()
    });
    assert_eq!(
        words,
        [4 << 9, 0xF00 << 12, (3 << 20) | (3 << 22) | (2 << 26), 0]
    );
}

#[test]
fn a_shadow_sampler_clamps_to_a_white_border_and_compares() {
    let shadow = Sampler {
        address_u: AddressMode::ClampBorder,
        address_v: AddressMode::ClampBorder,
        address_w: AddressMode::ClampBorder,
        mag: Filter::Bilinear,
        min: Filter::Bilinear,
        compare: Compare::LessEqual,
        border: Border::OpaqueWhite,
        ..Sampler::default()
    };
    assert_eq!(
        encode_sampler(&shadow),
        [0x31B6, 0, (1 << 20) | (1 << 22), 2 << 30]
    );
}

#[test]
fn samplers_round_trip_with_the_layers_own_marks() {
    let samples = [
        Sampler {
            address_u: AddressMode::MirrorOnceBorder,
            mag: Filter::Bilinear,
            min: Filter::AnisoPoint,
            mip: MipFilter::Point,
            max_anisotropy: 8,
            compare: Compare::GreaterEqual,
            min_lod: 0.25,
            max_lod: 12.0,
            lod_bias: -3.5,
            border: Border::OpaqueBlack,
            reduction: Reduction::Maximum,
            integer_border: true,
            ..Sampler::default()
        },
        Sampler {
            compare_never: true,
            unnormalized: true,
            border: Border::Table,
            border_index: 0x123,
            ..Sampler::default()
        },
    ];
    for sampler in samples {
        assert_eq!(decode_sampler(&encode_sampler(&sampler)), sampler);
    }
    assert_eq!(decode_sampler(&[0, 0, 0x2000, 0]).lod_bias, -32.0);
    let plain = encode_sampler(&Sampler::default());
    let never = encode_sampler(&Sampler {
        compare_never: true,
        ..Sampler::default()
    });
    assert_ne!(plain, never);
}

#[test]
fn buffer_words() {
    assert_eq!(encode_buffer(&Buffer::default()), [0, 0, 0, 0xFAC]);
    let buffer = Buffer {
        address: 0xABCD_1234_5678,
        stride: 0x3FFF,
        num_records: 0xDEAD_BEEF,
        format: 0x7F,
        swizzle: [SELECT_X, SELECT_Y, SELECT_ZERO, SELECT_W],
        out_of_bounds: OutOfBounds::Raw,
    };
    let words = encode_buffer(&buffer);
    assert_eq!(words[0], 0x1234_5678);
    assert_eq!(words[1], 0xABCD | (0x3FFF << 16));
    assert_eq!(words[2], 0xDEAD_BEEF);
    assert_eq!(
        words[3],
        (4 | (5 << 3) | (7 << 9)) | (0x7F << 12) | (3 << 28)
    );
    assert_eq!(decode_buffer(&words), Some(buffer));
}

#[test]
fn an_image_is_not_a_buffer_and_a_reserved_swizzle_is_refused() {
    let image = encode_image(&Image::default());
    assert_eq!(
        decode_buffer(&[image[0], image[1], image[2], image[3]]),
        None
    );
    assert_eq!(decode_buffer(&[0, 0, 0, 0xFAC & !7 | 2]), None);
}
