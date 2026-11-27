//! Creating and releasing resources on behalf of the title, with the content filter told of each.

use ash::vk;
use content_filter::{GpuKind, ResourceDesc as FilterDesc, ResourceId as FilterId};

use super::hooks;
use super::state::Assembly;
use crate::descriptor::format::dxgi;
use crate::descriptor::views::ViewResource;
use crate::execute::barrier;
use crate::execute::{Barrier, ImageRef, Resource as ReplayResource};
use crate::resource::desc::ResourceDesc;
use crate::resource::{HeapKind, Origin, Resource, ResourceError, ResourceId};

/// How a resource gets its memory.
#[derive(Debug, Clone, Copy)]
pub enum Placement {
    Committed(HeapKind),
    /// In memory the title mapped at this address.
    Placed(u64),
    Reserved,
}

/// An image as replay names it.
pub fn image_ref(resource: &Resource) -> Option<ImageRef> {
    let image = resource.image.as_ref()?;
    Some(ImageRef {
        image: image.image,
        format: image.format,
        aspect: image.aspect,
        extent: image.extent,
        mips: image.mips,
        layers: image.layers,
    })
}

impl Assembly {
    pub fn create_resource(
        &self,
        placement: Placement,
        desc: &ResourceDesc,
        state: u32,
    ) -> Result<ResourceId, ResourceError> {
        let (id, image, filter_desc) = {
            let mut resources = self.resources();
            let id = match placement {
                Placement::Committed(heap) => resources.create_committed(desc, heap)?,
                Placement::Placed(va) => resources.create_placed(va, desc)?,
                Placement::Reserved => resources.create_reserved(desc)?,
            };
            let r = resources.get(id).expect("just created");
            (
                id,
                image_ref(r),
                FilterDesc {
                    id: FilterId(u64::from(id)),
                    kind: if desc.is_buffer() {
                        GpuKind::Buffer
                    } else {
                        GpuKind::Texture
                    },
                    size: r.size,
                    gpu_address: Some(r.address),
                },
            )
        };
        if let Some(filter) = hooks::content_filter() {
            filter.on_create(filter_desc, None);
        }
        if let Some(image) = image
            && state != 0
            && state != 0xFFFF_FFFF
        {
            self.transition_to(image, state);
        }
        Ok(id)
    }

    /// A new image is in `GENERAL`; the title's initial state may mean another layout.
    fn transition_to(&self, image: ImageRef, state: u32) {
        let barriers = [Barrier::Transition {
            resource: ReplayResource::Image(image),
            subresource: None,
            before: 0,
            after: super::bridge::adjust_state(state),
        }];
        let exec = self.exec.lock().unwrap_or_else(|e| e.into_inner());
        let result = self.run_commands(exec.pool, exec.fence, |cb| {
            // SAFETY: `cb` is recording and outside a rendering scope.
            unsafe { barrier::record(&self.gpu.ctx.device, cb, &barriers) };
        });
        if let Err(e) = result {
            crate::com::note_once(format!("an initial resource state could not be set: {e}"));
        }
    }

    pub(super) fn release_resource(&self, id: ResourceId) {
        // Waits for a submission that may still be using the resource.
        let _exec = self.exec.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(filter) = hooks::content_filter() {
            filter.on_release(FilterId(u64::from(id)));
        }
        let image: Option<vk::Image> = self
            .resources()
            .get(id)
            .and_then(|r| r.image.as_ref().map(|i| i.image));
        if let Some(image) = image {
            self.views.forget(image);
        }
        let _ = self.resources().destroy(id);
    }

    /// A one-texel grey texture standing in for a texture the content filter hides.
    pub fn neutral_texture(&self) -> Option<ViewResource> {
        let mut slot = self.neutral.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 1, 1, 1);
            let id = self
                .create_resource(Placement::Committed(HeapKind::Default), &desc, 0)
                .ok()?;
            let resources = self.resources();
            let handles = resources.space().handles().clone();
            let image = resources.get(id)?.image.as_ref()?;
            let _ = image.upload(&handles, 0, 0, &[128, 128, 128, 255]);
            *slot = Some(id);
        }
        let id = (*slot)?;
        self.resources().get(id).map(ViewResource::from)
    }

    /// The texels a placed texture holds in game memory are read into its image the first time it is used.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn refresh_placed_texture(&self, resource: *mut std::ffi::c_void) {
        // SAFETY: the title's resource.
        let Some(object) = (unsafe { super::objects::resource_of(resource) }) else {
            return;
        };
        let resources = self.resources();
        let placed = resources
            .get(object.id)
            .is_some_and(|r| r.origin == Origin::Placed && r.image.is_some());
        if !placed
            || !self
                .uploaded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(object.id)
        {
            return;
        }
        if let Err(e) = resources.upload_texture(object.id) {
            crate::com::note_once(format!("placed texture texels were not read: {e}"));
        }
    }
}
