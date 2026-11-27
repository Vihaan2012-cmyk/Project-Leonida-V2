//! Vulkan side of presentation: swap chain and offscreen targets, and the renderer that puts a source image on one.
//!
//! The renderer works on the device it is given (the title's images live there), so it takes handles rather than making
//! its own. `OwnedVulkan` makes a standalone context for tests and for a run with no renderer wired in yet.

use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use ash::vk;

use super::frames::SyncMode;
use super::{NativeWindow, PresentError};

/// Handles of the device the title renders on. `queue` is the one lock for the queue: every submit takes it.
#[derive(Clone)]
pub struct VulkanContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: Arc<Mutex<vk::Queue>>,
    pub family: u32,
}

/// Instance extensions a window needs (and the colour space one HDR10 needs, which is optional).
pub fn instance_extensions() -> Vec<&'static CStr> {
    vec![
        ash::khr::surface::NAME,
        #[cfg(windows)]
        ash::khr::win32_surface::NAME,
    ]
}

pub const COLORSPACE_EXTENSION: &CStr = ash::ext::swapchain_colorspace::NAME;

/// Device extensions presentation needs.
pub fn device_extensions() -> Vec<&'static CStr> {
    vec![ash::khr::swapchain::NAME]
}

pub(super) fn find_memory(
    props: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..props.memory_type_count).find(|i| {
        bits & (1 << i) != 0
            && props.memory_types[*i as usize]
                .property_flags
                .contains(flags)
    })
}

/// A context the layer made itself.
pub struct OwnedVulkan {
    pub ctx: VulkanContext,
    pub name: String,
}

impl OwnedVulkan {
    /// Picks a GPU with a graphics queue; `window` also asks for the surface and swap chain extensions.
    pub fn new(window: bool) -> Result<OwnedVulkan, PresentError> {
        // SAFETY: loading the system Vulkan loader; everything created below is destroyed in Drop.
        unsafe {
            let entry = ash::Entry::load().map_err(|e| PresentError::Vulkan(e.to_string()))?;
            let mut names: Vec<*const i8> = Vec::new();
            if window {
                let have = entry.enumerate_instance_extension_properties(None)?;
                for want in instance_extensions()
                    .into_iter()
                    .chain(std::iter::once(COLORSPACE_EXTENSION))
                {
                    if have.iter().any(|e| e.extension_name_as_c_str() == Ok(want)) {
                        names.push(want.as_ptr());
                    }
                }
            }
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_2);
            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&names),
                None,
            )?;
            let mut best = None;
            for physical in instance.enumerate_physical_devices()? {
                let families = instance.get_physical_device_queue_family_properties(physical);
                let Some(family) = families
                    .iter()
                    .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                else {
                    continue;
                };
                if window {
                    let exts = instance.enumerate_device_extension_properties(physical)?;
                    if !exts
                        .iter()
                        .any(|e| e.extension_name_as_c_str() == Ok(ash::khr::swapchain::NAME))
                    {
                        continue;
                    }
                }
                let props = instance.get_physical_device_properties(physical);
                let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                if best.is_none() || discrete {
                    best = Some((physical, family as u32, props));
                }
            }
            let Some((physical, family, props)) = best else {
                instance.destroy_instance(None);
                return Err(PresentError::Vulkan("no GPU with a graphics queue".into()));
            };
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let exts: Vec<*const i8> = if window {
                device_extensions().iter().map(|e| e.as_ptr()).collect()
            } else {
                Vec::new()
            };
            let device = match instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&exts),
                None,
            ) {
                Ok(d) => d,
                Err(e) => {
                    instance.destroy_instance(None);
                    return Err(e.into());
                }
            };
            let queue = device.get_device_queue(family, 0);
            let name = props
                .device_name_as_c_str()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(OwnedVulkan {
                ctx: VulkanContext {
                    entry,
                    instance,
                    physical,
                    device,
                    queue: Arc::new(Mutex::new(queue)),
                    family,
                },
                name,
            })
        }
    }
}

impl Drop for OwnedVulkan {
    fn drop(&mut self) {
        // SAFETY: the owner outlives every object made on this device, so nothing else uses it any more.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            self.ctx.device.destroy_device(None);
            self.ctx.instance.destroy_instance(None);
        }
    }
}

/// An image the title hands over, in the layout it is in when presented.
#[derive(Clone)]
pub struct SourceImage {
    pub image: vk::Image,
    pub format: vk::Format,
    pub extent: (u32, u32),
    pub layout: vk::ImageLayout,
    pub usage: vk::ImageUsageFlags,
    pub samples: vk::SampleCountFlags,
    /// Timeline semaphore values that mark the end of the work that wrote the image; none when it is on the same queue.
    pub waits: Vec<(vk::Semaphore, u64)>,
}

pub fn is_srgb(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::R8G8B8A8_SRGB | vk::Format::B8G8R8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32
    )
}

/// Colour formats a blit or a sampled read turns into floats.
pub fn is_displayable(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::R8G8B8A8_UNORM
            | vk::Format::R8G8B8A8_SRGB
            | vk::Format::B8G8R8A8_UNORM
            | vk::Format::B8G8R8A8_SRGB
            | vk::Format::A8B8G8R8_UNORM_PACK32
            | vk::Format::A8B8G8R8_SRGB_PACK32
            | vk::Format::A2B10G10R10_UNORM_PACK32
            | vk::Format::A2R10G10B10_UNORM_PACK32
            | vk::Format::R16G16B16A16_UNORM
            | vk::Format::R16G16B16A16_SFLOAT
            | vk::Format::B10G11R11_UFLOAT_PACK32
            | vk::Format::R5G6B5_UNORM_PACK16
            | vk::Format::R8_UNORM
            | vk::Format::R8G8_UNORM
            | vk::Format::R16_UNORM
            | vk::Format::R16G16_UNORM
            | vk::Format::R16_SFLOAT
            | vk::Format::R16G16_SFLOAT
            | vk::Format::R32G32B32A32_SFLOAT
    )
}

/// What the swap chain has to be like for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetWant {
    pub hdr10: bool,
    pub srgb: bool,
    pub sync: SyncMode,
    /// The size to use when the surface does not say.
    pub size: (u32, u32),
}

pub fn choose_surface_format(
    formats: &[vk::SurfaceFormatKHR],
    hdr10: bool,
    srgb: bool,
) -> Option<vk::SurfaceFormatKHR> {
    let pick = |wanted: &[(vk::Format, vk::ColorSpaceKHR)]| {
        wanted.iter().find_map(|(f, c)| {
            formats
                .iter()
                .find(|s| s.format == *f && s.color_space == *c)
                .copied()
        })
    };
    let srgb_cs = vk::ColorSpaceKHR::SRGB_NONLINEAR;
    if hdr10 {
        let found = pick(&[
            (
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            (
                vk::Format::A2R10G10B10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
        ]);
        if found.is_some() {
            return found;
        }
    }
    let wanted: [(vk::Format, vk::ColorSpaceKHR); 4] = if srgb {
        [
            (vk::Format::B8G8R8A8_SRGB, srgb_cs),
            (vk::Format::R8G8B8A8_SRGB, srgb_cs),
            (vk::Format::B8G8R8A8_UNORM, srgb_cs),
            (vk::Format::R8G8B8A8_UNORM, srgb_cs),
        ]
    } else {
        [
            (vk::Format::B8G8R8A8_UNORM, srgb_cs),
            (vk::Format::R8G8B8A8_UNORM, srgb_cs),
            (vk::Format::B8G8R8A8_SRGB, srgb_cs),
            (vk::Format::R8G8B8A8_SRGB, srgb_cs),
        ]
    };
    pick(&wanted).or_else(|| formats.iter().find(|f| f.color_space == srgb_cs).copied())
}

pub fn choose_present_mode(modes: &[vk::PresentModeKHR], sync: SyncMode) -> vk::PresentModeKHR {
    let has = |m| modes.contains(&m);
    match sync {
        SyncMode::Vsync => vk::PresentModeKHR::FIFO,
        SyncMode::Mailbox if has(vk::PresentModeKHR::MAILBOX) => vk::PresentModeKHR::MAILBOX,
        SyncMode::Mailbox | SyncMode::Immediate => {
            if has(vk::PresentModeKHR::IMMEDIATE) {
                vk::PresentModeKHR::IMMEDIATE
            } else if has(vk::PresentModeKHR::MAILBOX) {
                vk::PresentModeKHR::MAILBOX
            } else {
                vk::PresentModeKHR::FIFO
            }
        }
    }
}

pub enum Acquire {
    Image(u32, vk::Image),
    /// Nothing to draw on (the window has no area).
    Skip,
    /// The target was rebuilt; ask again.
    Recreated,
}

/// Something a frame is drawn into and shown from.
pub trait Target: Send {
    fn extent(&self) -> (u32, u32);
    fn format(&self) -> vk::Format;
    fn hdr10(&self) -> bool;
    fn supports_hdr10(&self) -> bool;
    fn final_layout(&self) -> vk::ImageLayout;
    fn configure(&mut self, want: &TargetWant) -> Result<(), PresentError>;
    fn acquire(&mut self, signal: vk::Semaphore) -> Result<Acquire, PresentError>;
    /// The semaphore the submit signals for image `index`, or null when nothing waits on one.
    fn done_semaphore(&self, index: u32) -> vk::Semaphore;
    fn present(&mut self, queue: vk::Queue, index: u32) -> Result<(), PresentError>;
}

#[cfg(windows)]
fn create_surface(
    ctx: &VulkanContext,
    window: NativeWindow,
) -> Result<vk::SurfaceKHR, PresentError> {
    let hinstance = window.hinstance.unwrap_or_else(|| {
        // SAFETY: querying the handle of this process's own module.
        unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as isize
        }
    });
    let loader = ash::khr::win32_surface::Instance::new(&ctx.entry, &ctx.instance);
    let info = vk::Win32SurfaceCreateInfoKHR::default()
        .hinstance(hinstance as vk::HINSTANCE)
        .hwnd(window.hwnd as vk::HWND);
    // SAFETY: the window handle names a live window; the surface is destroyed with the target.
    Ok(unsafe { loader.create_win32_surface(&info, None)? })
}

#[cfg(not(windows))]
fn create_surface(_: &VulkanContext, _: NativeWindow) -> Result<vk::SurfaceKHR, PresentError> {
    Err(PresentError::Vulkan(
        "window surfaces are only made on Windows".into(),
    ))
}

/// A swap chain on a window.
pub struct SwapchainTarget {
    ctx: VulkanContext,
    surface_fn: ash::khr::surface::Instance,
    swapchain_fn: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    done: Vec<vk::Semaphore>,
    extent: vk::Extent2D,
    format: vk::SurfaceFormatKHR,
    mode: vk::PresentModeKHR,
    has_hdr10: bool,
    want: Option<TargetWant>,
    dirty: bool,
}

impl SwapchainTarget {
    pub fn new(ctx: &VulkanContext, window: NativeWindow) -> Result<SwapchainTarget, PresentError> {
        let surface = create_surface(ctx, window)?;
        let surface_fn = ash::khr::surface::Instance::new(&ctx.entry, &ctx.instance);
        let swapchain_fn = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
        // SAFETY: the surface was just made on this instance.
        let supported = unsafe {
            surface_fn.get_physical_device_surface_support(ctx.physical, ctx.family, surface)
        };
        if !matches!(supported, Ok(true)) {
            // SAFETY: nothing else holds the surface yet.
            unsafe { surface_fn.destroy_surface(surface, None) };
            return Err(PresentError::Vulkan(
                "the graphics queue family cannot present to this window".into(),
            ));
        }
        // SAFETY: as above.
        let has_hdr10 =
            unsafe { surface_fn.get_physical_device_surface_formats(ctx.physical, surface) }
                .map(|f| {
                    f.iter()
                        .any(|s| s.color_space == vk::ColorSpaceKHR::HDR10_ST2084_EXT)
                })
                .unwrap_or(false);
        Ok(SwapchainTarget {
            ctx: ctx.clone(),
            surface_fn,
            swapchain_fn,
            surface,
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            done: Vec::new(),
            extent: vk::Extent2D::default(),
            format: vk::SurfaceFormatKHR::default(),
            mode: vk::PresentModeKHR::FIFO,
            has_hdr10,
            want: None,
            dirty: true,
        })
    }

    fn destroy_semaphores(&mut self) {
        for s in self.done.drain(..) {
            // SAFETY: the device is idle when swap chain objects are replaced.
            unsafe { self.ctx.device.destroy_semaphore(s, None) };
        }
    }

    fn rebuild(&mut self, want: &TargetWant) -> Result<(), PresentError> {
        let ctx = self.ctx.clone();
        // SAFETY: the surface and physical device are valid; the old swap chain is retired after the new one exists.
        unsafe {
            let caps = self
                .surface_fn
                .get_physical_device_surface_capabilities(ctx.physical, self.surface)?;
            let formats = self
                .surface_fn
                .get_physical_device_surface_formats(ctx.physical, self.surface)?;
            let modes = self
                .surface_fn
                .get_physical_device_surface_present_modes(ctx.physical, self.surface)?;
            self.has_hdr10 = formats
                .iter()
                .any(|s| s.color_space == vk::ColorSpaceKHR::HDR10_ST2084_EXT);
            let extent = if caps.current_extent.width == u32::MAX {
                vk::Extent2D {
                    width: want
                        .size
                        .0
                        .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                    height: want
                        .size
                        .1
                        .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
                }
            } else {
                caps.current_extent
            };
            if extent.width == 0 || extent.height == 0 {
                self.extent = extent;
                self.want = Some(*want);
                self.dirty = false;
                return Ok(());
            }
            let format = choose_surface_format(&formats, want.hdr10, want.srgb)
                .ok_or_else(|| PresentError::Vulkan("the surface lists no usable format".into()))?;
            let mode = choose_present_mode(&modes, want.sync);
            let mut count = caps.min_image_count + 1;
            if caps.max_image_count != 0 {
                count = count.min(caps.max_image_count);
            }
            let usage = (vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT)
                & caps.supported_usage_flags;
            if !usage.contains(vk::ImageUsageFlags::TRANSFER_DST) {
                return Err(PresentError::Vulkan(
                    "the surface cannot be the target of a transfer".into(),
                ));
            }
            let alpha = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
            ]
            .into_iter()
            .find(|a| caps.supported_composite_alpha.contains(*a))
            .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
            let _ = ctx.device.device_wait_idle();
            let info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(count)
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(usage)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(alpha)
                .present_mode(mode)
                .clipped(true)
                .old_swapchain(self.swapchain);
            let new = self.swapchain_fn.create_swapchain(&info, None)?;
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_fn.destroy_swapchain(self.swapchain, None);
            }
            self.destroy_semaphores();
            self.swapchain = new;
            self.images = self.swapchain_fn.get_swapchain_images(new)?;
            for _ in &self.images {
                self.done.push(
                    ctx.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?,
                );
            }
            self.extent = extent;
            self.format = format;
            self.mode = mode;
        }
        self.want = Some(*want);
        self.dirty = false;
        Ok(())
    }
}

impl Target for SwapchainTarget {
    fn extent(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }

    fn format(&self) -> vk::Format {
        self.format.format
    }

    fn hdr10(&self) -> bool {
        self.format.color_space == vk::ColorSpaceKHR::HDR10_ST2084_EXT
    }

    fn supports_hdr10(&self) -> bool {
        self.has_hdr10
    }

    fn final_layout(&self) -> vk::ImageLayout {
        vk::ImageLayout::PRESENT_SRC_KHR
    }

    fn configure(&mut self, want: &TargetWant) -> Result<(), PresentError> {
        if let Some(have) = self.want {
            let same_look =
                have.hdr10 == want.hdr10 && have.srgb == want.srgb && have.sync == want.sync;
            let sized = self.extent.width == 0 || self.extent.height == 0;
            if !self.dirty && same_look && !sized {
                return Ok(());
            }
        }
        self.rebuild(want)
    }

    fn acquire(&mut self, signal: vk::Semaphore) -> Result<Acquire, PresentError> {
        if self.extent.width == 0
            || self.extent.height == 0
            || self.swapchain == vk::SwapchainKHR::null()
        {
            return Ok(Acquire::Skip);
        }
        // SAFETY: the swap chain is valid and the semaphore is unsignalled (its last wait finished with the slot fence).
        let got = unsafe {
            self.swapchain_fn.acquire_next_image(
                self.swapchain,
                1_000_000_000,
                signal,
                vk::Fence::null(),
            )
        };
        match got {
            Ok((index, suboptimal)) => {
                self.dirty |= suboptimal;
                Ok(Acquire::Image(index, self.images[index as usize]))
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.dirty = true;
                let want = self.want.unwrap_or(TargetWant {
                    hdr10: false,
                    srgb: false,
                    sync: SyncMode::Vsync,
                    size: (1, 1),
                });
                self.rebuild(&want)?;
                Ok(Acquire::Recreated)
            }
            Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => Ok(Acquire::Skip),
            Err(e) => Err(e.into()),
        }
    }

    fn done_semaphore(&self, index: u32) -> vk::Semaphore {
        self.done[index as usize]
    }

    fn present(&mut self, queue: vk::Queue, index: u32) -> Result<(), PresentError> {
        let waits = [self.done[index as usize]];
        let swapchains = [self.swapchain];
        let indices = [index];
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: the queue is locked by the caller and the image index came from `acquire`.
        match unsafe { self.swapchain_fn.queue_present(queue, &info) } {
            Ok(suboptimal) => {
                self.dirty |= suboptimal;
                Ok(())
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.dirty = true;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for SwapchainTarget {
    fn drop(&mut self) {
        // SAFETY: the owner stops presenting before it drops the target; the device is idle for the teardown.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            self.destroy_semaphores();
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_fn.destroy_swapchain(self.swapchain, None);
            }
            self.surface_fn.destroy_surface(self.surface, None);
        }
    }
}

/// A plain image in place of a swap chain; what a headless run presents to and what the tests read back.
pub struct OffscreenTarget {
    ctx: VulkanContext,
    image: vk::Image,
    memory: vk::DeviceMemory,
    size: (u32, u32),
    format: vk::Format,
    hdr10: bool,
}

impl OffscreenTarget {
    pub fn new(
        ctx: &VulkanContext,
        size: (u32, u32),
        format: vk::Format,
        hdr10: bool,
    ) -> Result<OffscreenTarget, PresentError> {
        let mut t = OffscreenTarget {
            ctx: ctx.clone(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            size: (0, 0),
            format,
            hdr10,
        };
        t.resize(size)?;
        Ok(t)
    }

    pub fn resize(&mut self, size: (u32, u32)) -> Result<(), PresentError> {
        if size == self.size && self.image != vk::Image::null() {
            return Ok(());
        }
        let device = self.ctx.device.clone();
        // SAFETY: the old image is idle (checked by waiting) and the new one is bound once to its own memory.
        unsafe {
            let _ = device.device_wait_idle();
            self.release();
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(self.format)
                .extent(vk::Extent3D {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            self.image = device.create_image(&info, None)?;
            let needs = device.get_image_memory_requirements(self.image);
            let props = self
                .ctx
                .instance
                .get_physical_device_memory_properties(self.ctx.physical);
            let kind = find_memory(
                &props,
                needs.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|| {
                find_memory(
                    &props,
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )
            })
            .ok_or_else(|| PresentError::Vulkan("no memory for the offscreen target".into()))?;
            self.memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            device.bind_image_memory(self.image, self.memory, 0)?;
        }
        self.size = size;
        Ok(())
    }

    fn release(&mut self) {
        // SAFETY: callers wait for the device first.
        unsafe {
            if self.image != vk::Image::null() {
                self.ctx.device.destroy_image(self.image, None);
                self.ctx.device.free_memory(self.memory, None);
            }
        }
        self.image = vk::Image::null();
        self.memory = vk::DeviceMemory::null();
    }

    /// The pixels as stored (4 bytes per texel for the 8-bit and 10-bit formats, 8 for RGBA16F), row by row.
    pub fn read_pixels(&self) -> Result<Vec<u8>, PresentError> {
        let texel = match self.format {
            vk::Format::R16G16B16A16_SFLOAT => 8,
            _ => 4,
        };
        let bytes = u64::from(self.size.0) * u64::from(self.size.1) * texel;
        let device = &self.ctx.device;
        // SAFETY: a one-time copy on a private pool; every object made here is destroyed before returning.
        unsafe {
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(buffer);
            let props = self
                .ctx
                .instance
                .get_physical_device_memory_properties(self.ctx.physical);
            let kind = find_memory(
                &props,
                needs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| PresentError::Vulkan("no host memory for the read-back".into()))?;
            let memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            device.bind_buffer_memory(buffer, memory, 0)?;
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(self.ctx.family),
                None,
            )?;
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )?[0];
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let region = vk::BufferImageCopy::default()
                .image_subresource(color_layers())
                .image_extent(vk::Extent3D {
                    width: self.size.0,
                    height: self.size.1,
                    depth: 1,
                });
            device.cmd_copy_image_to_buffer(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            device.end_command_buffer(cmd)?;
            let cmds = [cmd];
            {
                let queue = self.ctx.queue.lock().unwrap_or_else(|e| e.into_inner());
                device.queue_submit(
                    *queue,
                    &[vk::SubmitInfo::default().command_buffers(&cmds)],
                    vk::Fence::null(),
                )?;
                device.queue_wait_idle(*queue)?;
            }
            let ptr =
                device.map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())? as *const u8;
            let data = std::slice::from_raw_parts(ptr, bytes as usize).to_vec();
            device.unmap_memory(memory);
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(buffer, None);
            device.free_memory(memory, None);
            Ok(data)
        }
    }
}

impl Target for OffscreenTarget {
    fn extent(&self) -> (u32, u32) {
        self.size
    }

    fn format(&self) -> vk::Format {
        self.format
    }

    fn hdr10(&self) -> bool {
        self.hdr10
    }

    fn supports_hdr10(&self) -> bool {
        self.hdr10
    }

    fn final_layout(&self) -> vk::ImageLayout {
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL
    }

    fn configure(&mut self, _: &TargetWant) -> Result<(), PresentError> {
        Ok(())
    }

    fn acquire(&mut self, _: vk::Semaphore) -> Result<Acquire, PresentError> {
        Ok(Acquire::Image(0, self.image))
    }

    fn done_semaphore(&self, _: u32) -> vk::Semaphore {
        vk::Semaphore::null()
    }

    fn present(&mut self, _: vk::Queue, _: u32) -> Result<(), PresentError> {
        Ok(())
    }
}

impl Drop for OffscreenTarget {
    fn drop(&mut self) {
        // SAFETY: the device is idle before the image goes.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }
        self.release();
    }
}

pub(super) fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

pub(super) fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(f: vk::Format, c: vk::ColorSpaceKHR) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format: f,
            color_space: c,
        }
    }

    #[test]
    fn surface_format_follows_the_wanted_encoding() {
        let list = [
            fmt(
                vk::Format::R8G8B8A8_UNORM,
                vk::ColorSpaceKHR::SRGB_NONLINEAR,
            ),
            fmt(vk::Format::B8G8R8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
            fmt(
                vk::Format::B8G8R8A8_UNORM,
                vk::ColorSpaceKHR::SRGB_NONLINEAR,
            ),
            fmt(
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
        ];
        assert_eq!(
            choose_surface_format(&list, false, false).unwrap().format,
            vk::Format::B8G8R8A8_UNORM
        );
        assert_eq!(
            choose_surface_format(&list, false, true).unwrap().format,
            vk::Format::B8G8R8A8_SRGB
        );
        let hdr = choose_surface_format(&list, true, false).unwrap();
        assert_eq!(hdr.color_space, vk::ColorSpaceKHR::HDR10_ST2084_EXT);
        assert_eq!(
            choose_surface_format(&list[..3], true, false)
                .unwrap()
                .format,
            vk::Format::B8G8R8A8_UNORM
        );
        assert!(choose_surface_format(&[], false, false).is_none());
    }

    #[test]
    fn present_mode_follows_the_sync_mode_and_what_the_surface_lists() {
        use vk::PresentModeKHR as M;
        let all = [M::FIFO, M::MAILBOX, M::IMMEDIATE];
        assert_eq!(choose_present_mode(&all, SyncMode::Vsync), M::FIFO);
        assert_eq!(choose_present_mode(&all, SyncMode::Mailbox), M::MAILBOX);
        assert_eq!(choose_present_mode(&all, SyncMode::Immediate), M::IMMEDIATE);
        assert_eq!(
            choose_present_mode(&[M::FIFO, M::MAILBOX], SyncMode::Immediate),
            M::MAILBOX
        );
        assert_eq!(
            choose_present_mode(&[M::FIFO], SyncMode::Immediate),
            M::FIFO
        );
        assert_eq!(
            choose_present_mode(&[M::FIFO, M::IMMEDIATE], SyncMode::Mailbox),
            M::IMMEDIATE
        );
    }

    #[test]
    fn format_helpers_classify_back_buffer_formats() {
        assert!(is_srgb(vk::Format::B8G8R8A8_SRGB));
        assert!(!is_srgb(vk::Format::B8G8R8A8_UNORM));
        assert!(is_displayable(vk::Format::A2B10G10R10_UNORM_PACK32));
        assert!(!is_displayable(vk::Format::D32_SFLOAT));
        assert!(!is_displayable(vk::Format::R32_UINT));
    }
}
