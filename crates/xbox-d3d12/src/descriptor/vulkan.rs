//! What a registered descriptor means to Vulkan: the image view a T# asks for and the sampler an S# asks for.

use ash::vk;

use super::format::{FORMATS, Format, Kind, format};
use super::gfx10::{
    self, AddressMode, Border, Compare, Image, ImageType, MipFilter, Reduction, SELECT_ONE,
    SELECT_W, SELECT_X, Sampler, Swizzle,
};

/// The Vulkan view of a texture a T# describes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewInfo {
    pub view_type: vk::ImageViewType,
    pub format: vk::Format,
    pub base_mip: u32,
    pub mips: u32,
    pub base_layer: u32,
    pub layers: u32,
    pub components: [vk::ComponentSwizzle; 4],
    pub min_lod: f32,
    pub samples: u32,
}

/// Which format a view of a resource in `resource` takes when its T# names `code`: the resource's own when the
/// codes agree, else the format of that code in the resource's channel order (an sRGB view of a UNORM image),
/// else the first of that code. Depth and stencil formats are only viewed as themselves.
pub fn view_format(resource: &Format, code: u32) -> Option<&'static Format> {
    if resource.gfx10 == code && !resource.typeless {
        return format(resource.dxgi);
    }
    if resource.kind != Kind::Color {
        return FORMATS
            .iter()
            .find(|f| f.kind == resource.kind && f.gfx10 == code && f.vulkan == resource.vulkan);
    }
    let candidates = || {
        FORMATS
            .iter()
            .filter(move |f| f.gfx10 == code && f.kind == Kind::Color && !f.typeless)
    };
    candidates()
        .find(|f| f.channels == resource.channels)
        .or_else(|| candidates().next())
}

/// `DST_SEL` as a component mapping: the hardware's channels X to W are the view format's red to alpha as its
/// channel order says, and a channel the format lacks reads as zero.
pub fn components(swizzle: Swizzle, view: &Format) -> [vk::ComponentSwizzle; 4] {
    swizzle.map(|select| {
        if select == SELECT_ONE {
            return vk::ComponentSwizzle::ONE;
        }
        if (SELECT_X..=SELECT_W).contains(&select)
            && let Some(i) = view.channels.iter().position(|c| *c == select)
        {
            return [
                vk::ComponentSwizzle::R,
                vk::ComponentSwizzle::G,
                vk::ComponentSwizzle::B,
                vk::ComponentSwizzle::A,
            ][i];
        }
        vk::ComponentSwizzle::ZERO
    })
}

/// The view type shaders sample a T# of this type through: a cube is a six-layer 2D array there, because the
/// shader has already chosen the face.
pub fn view_type(ty: ImageType) -> vk::ImageViewType {
    match ty {
        ImageType::D1 => vk::ImageViewType::TYPE_1D,
        ImageType::D1Array => vk::ImageViewType::TYPE_1D_ARRAY,
        ImageType::D3 => vk::ImageViewType::TYPE_3D,
        ImageType::Cube | ImageType::D2Array | ImageType::D2MsaaArray => {
            vk::ImageViewType::TYPE_2D_ARRAY
        }
        ImageType::D2 | ImageType::D2Msaa => vk::ImageViewType::TYPE_2D,
    }
}

/// `None` when no format has the code the T# names, or the resource cannot be viewed as it.
pub fn view_info(image: &Image, resource: &Format) -> Option<ViewInfo> {
    let view = view_format(resource, image.format)?;
    Some(ViewInfo {
        view_type: view_type(image.ty),
        format: view.vulkan,
        base_mip: image.first_mip,
        mips: image.mip_count,
        base_layer: image.first_layer,
        layers: if image.ty == ImageType::D3 {
            1
        } else {
            image.layer_count
        },
        components: components(image.swizzle, view),
        min_lod: image.min_lod,
        samples: image.samples,
    })
}

/// The sampler an S# asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerParams {
    pub mag: vk::Filter,
    pub min: vk::Filter,
    pub mipmap: vk::SamplerMipmapMode,
    pub address: [vk::SamplerAddressMode; 3],
    pub lod_bias: f32,
    pub anisotropy: Option<f32>,
    pub compare: Option<vk::CompareOp>,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border: vk::BorderColor,
    /// The index into the border colour table when the colour is custom.
    pub custom_border: Option<u32>,
    pub reduction: vk::SamplerReductionMode,
    pub unnormalized: bool,
}

fn address_mode(mode: AddressMode) -> vk::SamplerAddressMode {
    match mode {
        AddressMode::Wrap => vk::SamplerAddressMode::REPEAT,
        AddressMode::Mirror => vk::SamplerAddressMode::MIRRORED_REPEAT,
        AddressMode::ClampBorder => vk::SamplerAddressMode::CLAMP_TO_BORDER,
        AddressMode::MirrorOnceLastTexel
        | AddressMode::MirrorOnceHalfBorder
        | AddressMode::MirrorOnceBorder => vk::SamplerAddressMode::MIRROR_CLAMP_TO_EDGE,
        AddressMode::ClampLastTexel | AddressMode::ClampHalfBorder => {
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        }
    }
}

fn compare_op(compare: Compare) -> vk::CompareOp {
    vk::CompareOp::from_raw(compare as i32)
}

/// As the hardware reads the S#, with what Vulkan requires of an unnormalised sampler applied (no mips, no
/// anisotropy, no comparison, clamped addressing) and a sampler without a mip filter held to the base level.
pub fn sampler_params(s: &Sampler, max_bias: f32) -> SamplerParams {
    let filter = |f: gfx10::Filter| {
        if f.linear() {
            vk::Filter::LINEAR
        } else {
            vk::Filter::NEAREST
        }
    };
    let mut p = SamplerParams {
        mag: filter(s.mag),
        min: filter(s.min),
        mipmap: if s.mip == MipFilter::Linear {
            vk::SamplerMipmapMode::LINEAR
        } else {
            vk::SamplerMipmapMode::NEAREST
        },
        address: [s.address_u, s.address_v, s.address_w].map(address_mode),
        lod_bias: s.lod_bias.clamp(-max_bias, max_bias),
        anisotropy: (s.max_anisotropy > 1).then_some(s.max_anisotropy as f32),
        compare: (s.compare_never || s.compare != Compare::Never).then(|| compare_op(s.compare)),
        min_lod: s.min_lod,
        max_lod: s.max_lod,
        border: match (s.border, s.integer_border) {
            (Border::OpaqueBlack, false) => vk::BorderColor::FLOAT_OPAQUE_BLACK,
            (Border::OpaqueBlack, true) => vk::BorderColor::INT_OPAQUE_BLACK,
            (Border::OpaqueWhite, false) => vk::BorderColor::FLOAT_OPAQUE_WHITE,
            (Border::OpaqueWhite, true) => vk::BorderColor::INT_OPAQUE_WHITE,
            (_, false) => vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
            (_, true) => vk::BorderColor::INT_TRANSPARENT_BLACK,
        },
        custom_border: (s.border == Border::Table).then_some(s.border_index),
        reduction: match s.reduction {
            Reduction::Minimum => vk::SamplerReductionMode::MIN,
            Reduction::Maximum => vk::SamplerReductionMode::MAX,
            _ => vk::SamplerReductionMode::WEIGHTED_AVERAGE,
        },
        unnormalized: s.unnormalized,
    };
    if s.mip == MipFilter::None {
        p.min_lod = 0.0;
        p.max_lod = 0.25;
    }
    if s.unnormalized {
        let edge = |m: vk::SamplerAddressMode| {
            if m == vk::SamplerAddressMode::CLAMP_TO_BORDER {
                m
            } else {
                vk::SamplerAddressMode::CLAMP_TO_EDGE
            }
        };
        p.address = [edge(p.address[0]), edge(p.address[1]), p.address[2]];
        p.min = p.mag;
        p.mipmap = vk::SamplerMipmapMode::NEAREST;
        p.min_lod = 0.0;
        p.max_lod = 0.0;
        p.anisotropy = None;
        p.compare = None;
    }
    p
}

impl SamplerParams {
    /// The create info; chain `SamplerReductionModeCreateInfo` for a reduction other than weighted average, and
    /// a custom border colour for `custom_border`.
    pub fn create_info(&self) -> vk::SamplerCreateInfo<'static> {
        vk::SamplerCreateInfo::default()
            .mag_filter(self.mag)
            .min_filter(self.min)
            .mipmap_mode(self.mipmap)
            .address_mode_u(self.address[0])
            .address_mode_v(self.address[1])
            .address_mode_w(self.address[2])
            .mip_lod_bias(self.lod_bias)
            .anisotropy_enable(self.anisotropy.is_some())
            .max_anisotropy(self.anisotropy.unwrap_or(1.0))
            .compare_enable(self.compare.is_some())
            .compare_op(self.compare.unwrap_or(vk::CompareOp::NEVER))
            .min_lod(self.min_lod)
            .max_lod(self.max_lod)
            .border_color(self.border)
            .unnormalized_coordinates(self.unnormalized)
    }
}
