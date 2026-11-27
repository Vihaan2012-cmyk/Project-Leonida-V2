use std::sync::Arc;

use ash::vk;

use crate::device::Device;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerDesc {
    pub mag: vk::Filter,
    pub min: vk::Filter,
    pub mip: vk::SamplerMipmapMode,
    pub address: [vk::SamplerAddressMode; 3],
    pub mip_lod_bias: f32,
    /// 1.0 or less turns anisotropic filtering off.
    pub max_anisotropy: f32,
    /// Depth comparison; `None` for an ordinary sampler.
    pub compare: Option<vk::CompareOp>,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border: vk::BorderColor,
}

impl Default for SamplerDesc {
    /// Nearest filtering, clamping to the edge, every mip level.
    fn default() -> Self {
        SamplerDesc {
            mag: vk::Filter::NEAREST,
            min: vk::Filter::NEAREST,
            mip: vk::SamplerMipmapMode::NEAREST,
            address: [vk::SamplerAddressMode::CLAMP_TO_EDGE; 3],
            mip_lod_bias: 0.0,
            max_anisotropy: 1.0,
            compare: None,
            min_lod: 0.0,
            max_lod: vk::LOD_CLAMP_NONE,
            border: vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
        }
    }
}

impl SamplerDesc {
    pub fn linear() -> SamplerDesc {
        SamplerDesc {
            mag: vk::Filter::LINEAR,
            min: vk::Filter::LINEAR,
            mip: vk::SamplerMipmapMode::LINEAR,
            ..SamplerDesc::default()
        }
    }
}

struct SamplerInner {
    device: Device,
    handle: vk::Sampler,
    desc: SamplerDesc,
}

impl Drop for SamplerInner {
    fn drop(&mut self) {
        // SAFETY: the sampler belongs to this device; descriptors that name it keep a `Sampler` clone.
        unsafe { self.device.raw().destroy_sampler(self.handle, None) };
    }
}

/// A `VkSampler`. Clones share it.
#[derive(Clone)]
pub struct Sampler {
    inner: Arc<SamplerInner>,
}

impl Sampler {
    pub fn new(device: &Device, desc: &SamplerDesc) -> Result<Sampler> {
        let anisotropy = desc.max_anisotropy > 1.0;
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(desc.mag)
            .min_filter(desc.min)
            .mipmap_mode(desc.mip)
            .address_mode_u(desc.address[0])
            .address_mode_v(desc.address[1])
            .address_mode_w(desc.address[2])
            .mip_lod_bias(desc.mip_lod_bias)
            .anisotropy_enable(anisotropy)
            .max_anisotropy(if anisotropy {
                desc.max_anisotropy
                    .min(device.limits().max_sampler_anisotropy)
            } else {
                1.0
            })
            .compare_enable(desc.compare.is_some())
            .compare_op(desc.compare.unwrap_or(vk::CompareOp::NEVER))
            .min_lod(desc.min_lod)
            .max_lod(desc.max_lod)
            .border_color(desc.border);
        // SAFETY: plain creation on a live device.
        let handle = unsafe { device.raw().create_sampler(&info, None)? };
        Ok(Sampler {
            inner: Arc::new(SamplerInner {
                device: device.clone(),
                handle,
                desc: *desc,
            }),
        })
    }

    pub fn handle(&self) -> vk::Sampler {
        self.inner.handle
    }

    pub fn desc(&self) -> &SamplerDesc {
        &self.inner.desc
    }
}
