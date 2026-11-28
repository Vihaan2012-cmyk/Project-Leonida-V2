//! NGX initialised for frame generation, and the feature instance on top of it.
//!
//! A [`Session`] owns one NGX initialisation for the device it was given and at most one frame generation feature.
//! Every NGX call runs under the NGX lock (`dlss::lock::ngx_lock`, shared with super resolution). The commands NGX records are the caller's to submit:
//! creation and evaluation both record into a command buffer that is open and recording on a graphics or compute
//! queue of the device, and the caller restores any state it relies on afterwards.

use std::mem::ManuallyDrop;
use std::path::Path;
use std::sync::Arc;

use ash::vk;

use super::names;
use super::params::{self, CreateParams, EvalOptions, Resources};
use super::support::Capabilities;
use crate::dlss::ffi;
use crate::dlss::lock::ngx_lock;
use crate::dlss::modes::DlssOptions;
use crate::dlss::ngx::{Api, Ngx, NgxError, Parameters, VkInit};

fn check(call: &'static str, result: ffi::Result) -> Result<(), NgxError> {
    if ffi::failed(result) {
        Err(NgxError::Call { call, result })
    } else {
        Ok(())
    }
}

struct Instance {
    api: Arc<Api>,
    handle: *mut ffi::Handle,
    params: ManuallyDrop<Parameters>,
    create: CreateParams,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _turn = ngx_lock();
        // SAFETY: the handle came from CreateFeature and is released once; the map is destroyed after the feature.
        unsafe {
            (self.api.release_feature)(self.handle);
            ManuallyDrop::drop(&mut self.params);
        }
    }
}

/// NGX for one device, with the frame generation feature once [`Session::prepare`] has made it.
pub struct Session {
    api: Arc<Api>,
    device: vk::Device,
    capabilities: Capabilities,
    instance: Option<Instance>,
    // Dropped by hand, under the NGX lock, after the feature.
    ngx: ManuallyDrop<Ngx>,
}

// SAFETY: every NGX and Vulkan object is used only through `&mut self`, and every NGX call takes the process-wide lock.
unsafe impl Send for Session {}

impl Session {
    /// Initialises NGX on the device. `dlssg_dir` is the folder holding `nvngx_dlssg.dll`, searched in addition to
    /// NGX's own places. Fails with the reason when NGX cannot be initialised or reports frame generation unavailable.
    pub fn new(
        api: Arc<Api>,
        vk_init: &VkInit,
        options: &DlssOptions,
        dlssg_dir: Option<&Path>,
    ) -> Result<Session, NgxError> {
        let _turn = ngx_lock();
        let ngx = Ngx::init(api.clone(), vk_init, options, dlssg_dir)?;
        let capabilities = Capabilities::read(ngx.capability_map());
        let session = Session {
            api,
            device: vk_init.device,
            capabilities,
            instance: None,
            ngx: ManuallyDrop::new(ngx),
        };
        if let Some(reason) = session.capabilities.reason() {
            drop(_turn);
            drop(session);
            return Err(NgxError::Other(reason));
        }
        Ok(session)
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The parameters of the live feature, if there is one.
    pub fn created_for(&self) -> Option<&CreateParams> {
        self.instance.as_ref().map(|i| &i.create)
    }

    /// Makes the feature for `create` unless the live one already has exactly these parameters. Returns whether a new
    /// feature was made, in which case its first evaluation must be a reset. The commands NGX records into `cmd` must
    /// be submitted and finished before the feature is evaluated, and the GPU must be done with the previous feature
    /// when one is replaced.
    pub fn prepare(
        &mut self,
        cmd: vk::CommandBuffer,
        create: &CreateParams,
    ) -> Result<bool, NgxError> {
        create.check().map_err(NgxError::Other)?;
        if self.created_for() == Some(create) {
            return Ok(false);
        }
        self.instance = None;
        let _turn = ngx_lock();
        let mut map = Parameters::allocate(&self.api)?;
        params::apply_create(&mut *map, create);
        let mut handle = std::ptr::null_mut();
        // SAFETY: `cmd` is recording, the map is live and `handle` an out-pointer.
        let (call, result) = unsafe {
            match self.api.create_feature1 {
                Some(make) => (
                    "NVSDK_NGX_VULKAN_CreateFeature1",
                    make(
                        self.device,
                        cmd,
                        names::FEATURE_FRAME_GENERATION,
                        map.as_ptr(),
                        &mut handle,
                    ),
                ),
                None => (
                    "NVSDK_NGX_VULKAN_CreateFeature",
                    (self.api.create_feature)(
                        cmd,
                        names::FEATURE_FRAME_GENERATION,
                        map.as_ptr(),
                        &mut handle,
                    ),
                ),
            }
        };
        check(call, result)?;
        self.instance = Some(Instance {
            api: self.api.clone(),
            handle,
            params: ManuallyDrop::new(map),
            create: create.clone(),
        });
        Ok(true)
    }

    /// Records one evaluation into `cmd`: the frame generated between the real frame given last time and `resources`'s
    /// back buffer. Every image must be in a layout a shader can read (the output in `GENERAL`) and stay alive until
    /// the command buffer has finished.
    pub fn evaluate(
        &mut self,
        cmd: vk::CommandBuffer,
        resources: &Resources<'_>,
        options: &EvalOptions,
    ) -> Result<(), NgxError> {
        if let Some(missing) = resources.missing_required() {
            return Err(NgxError::Other(format!(
                "frame generation was given no {missing}"
            )));
        }
        options.check().map_err(NgxError::Other)?;
        if options.multi_frame_count > self.capabilities.multi_frame_count_max {
            return Err(NgxError::Other(format!(
                "NGX generates at most {} frames between real ones on this GPU, {} were asked for",
                self.capabilities.multi_frame_count_max, options.multi_frame_count
            )));
        }
        let Some(instance) = self.instance.as_mut() else {
            return Err(NgxError::Other("prepare was not called".into()));
        };
        let _turn = ngx_lock();
        let map = &mut **instance.params;
        // SAFETY: `resources` and `options` are borrowed until the evaluate call below returns, and NGX only reads
        // through the pointers during it.
        unsafe {
            params::apply_resources(map, resources);
            params::apply_options(map, options);
        }
        // SAFETY: `cmd` is recording, the handle belongs to this feature and the map points at live resources.
        check("NVSDK_NGX_VULKAN_EvaluateFeature", unsafe {
            (self.api.evaluate_feature)(cmd, instance.handle, map.as_ptr(), None)
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.instance = None;
        let _turn = ngx_lock();
        // SAFETY: dropped once, here, after the feature was released and under the NGX lock.
        unsafe { ManuallyDrop::drop(&mut self.ngx) };
    }
}
