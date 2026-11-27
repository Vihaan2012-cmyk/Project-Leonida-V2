//! Image views of the textures the descriptors name: made on first use and kept until the image goes.

use std::collections::HashMap;
use std::sync::Mutex;

use ash::vk;

use crate::descriptor::vulkan::ViewInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ViewKey {
    view_type: i32,
    format: i32,
    aspect: u32,
    mips: (u32, u32),
    layers: (u32, u32),
    components: [i32; 4],
}

pub struct ViewCache {
    device: ash::Device,
    views: Mutex<HashMap<(vk::Image, ViewKey), vk::ImageView>>,
}

pub fn identity() -> [vk::ComponentSwizzle; 4] {
    [vk::ComponentSwizzle::IDENTITY; 4]
}

impl ViewCache {
    pub fn new(device: ash::Device) -> ViewCache {
        ViewCache {
            device,
            views: Mutex::new(HashMap::new()),
        }
    }

    /// The view of `image` that `info` describes, over `aspect`, with `components` as the channel mapping.
    pub fn get(
        &self,
        image: vk::Image,
        info: &ViewInfo,
        aspect: vk::ImageAspectFlags,
        components: [vk::ComponentSwizzle; 4],
    ) -> Result<vk::ImageView, vk::Result> {
        let key = ViewKey {
            view_type: info.view_type.as_raw(),
            format: info.format.as_raw(),
            aspect: aspect.as_raw(),
            mips: (info.base_mip, info.mips),
            layers: (info.base_layer, info.layers),
            components: components.map(|c| c.as_raw()),
        };
        let mut views = self.views.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(view) = views.get(&(image, key)) {
            return Ok(*view);
        }
        let create = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(info.view_type)
            .format(info.format)
            .components(vk::ComponentMapping {
                r: components[0],
                g: components[1],
                b: components[2],
                a: components[3],
            })
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(aspect)
                    .base_mip_level(info.base_mip)
                    .level_count(info.mips)
                    .base_array_layer(info.base_layer)
                    .layer_count(info.layers),
            );
        // SAFETY: a plain create call on a live image of this device; the view is destroyed by `forget` or Drop.
        let view = unsafe { self.device.create_image_view(&create, None)? };
        views.insert((image, key), view);
        Ok(view)
    }

    /// Destroys every view of `image`, which is about to be destroyed; the GPU must be done with them.
    pub fn forget(&self, image: vk::Image) {
        let mut views = self.views.lock().unwrap_or_else(|e| e.into_inner());
        views.retain(|(owner, _), view| {
            if *owner == image {
                // SAFETY: the caller promises no submitted work still uses the view.
                unsafe { self.device.destroy_image_view(*view, None) };
                false
            } else {
                true
            }
        });
    }
}

impl Drop for ViewCache {
    fn drop(&mut self) {
        let views = self.views.get_mut().unwrap_or_else(|e| e.into_inner());
        for (_, view) in views.drain() {
            // SAFETY: the cache's owner is gone, so nothing uses the views.
            unsafe { self.device.destroy_image_view(view, None) };
        }
    }
}
