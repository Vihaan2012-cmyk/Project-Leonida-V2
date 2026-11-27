use std::any::Any;
use std::sync::Arc;

use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

use crate::device::{Device, Queue, SignalSemaphore, Submission, WaitSemaphore};
use crate::error::{RenderError, Result};
use crate::format::Format;
use crate::image::{Image, ImageView, ViewDesc};

/// Anything with window and display handles that can outlive the swapchain, such as an `Arc<winit::window::Window>`.
pub trait WindowTarget: HasDisplayHandle + HasWindowHandle + Send + Sync + 'static {}
impl<T: HasDisplayHandle + HasWindowHandle + Send + Sync + 'static> WindowTarget for T {}

/// How finished frames reach the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    /// Vertical sync; always available.
    Fifo,
    /// Vertical sync that tears when a frame is late.
    FifoRelaxed,
    /// Vertical sync with the newest frame replacing queued ones: no tearing, low latency.
    Mailbox,
    /// No sync; tears.
    Immediate,
}

impl PresentMode {
    fn vk(self) -> vk::PresentModeKHR {
        match self {
            PresentMode::Fifo => vk::PresentModeKHR::FIFO,
            PresentMode::FifoRelaxed => vk::PresentModeKHR::FIFO_RELAXED,
            PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
            PresentMode::Immediate => vk::PresentModeKHR::IMMEDIATE,
        }
    }

    /// The mode to use when `self` is wanted: itself if listed, else FIFO.
    pub fn choose(self, available: &[PresentMode]) -> PresentMode {
        if available.contains(&self) {
            self
        } else {
            PresentMode::Fifo
        }
    }
}

/// The colour space the caller wants the surface in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// 8-bit sRGB-encoded.
    Sdr,
    /// 10-bit BT.2020 with the PQ curve.
    Hdr10,
    /// 16-bit float scRGB (linear, BT.709 primaries, 1.0 = 80 nits).
    ScRgb,
}

/// What `acquire`-time recreation and creation use.
#[derive(Debug, Clone, Copy)]
pub struct SwapchainDesc {
    /// Window size in pixels; used when the surface does not dictate one.
    pub extent: [u32; 2],
    pub present_mode: PresentMode,
    pub color_space: ColorSpace,
    /// Images wanted (at least the surface's minimum). Default 3.
    pub image_count: u32,
    /// Extra image usage on top of colour attachment and transfer destination, when the surface allows it.
    pub usage: vk::ImageUsageFlags,
}

impl Default for SwapchainDesc {
    fn default() -> Self {
        SwapchainDesc {
            extent: [1280, 720],
            present_mode: PresentMode::Fifo,
            color_space: ColorSpace::Sdr,
            image_count: 3,
            usage: vk::ImageUsageFlags::empty(),
        }
    }
}

/// Which HDR surface formats the display offers. The policy for using them belongs to the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HdrSupport {
    pub hdr10: bool,
    pub scrgb: bool,
}

fn known(format: &vk::SurfaceFormatKHR) -> bool {
    Format::from_vk(format.format).is_some()
}

/// Picks a surface format for `want` from what the surface lists; the flag says whether it had to fall back to
/// SDR. `None` when the surface lists nothing `render` knows.
pub fn pick_format(
    formats: &[vk::SurfaceFormatKHR],
    want: ColorSpace,
) -> Option<(vk::SurfaceFormatKHR, bool)> {
    let find = |spaces: &[vk::ColorSpaceKHR], kinds: &[vk::Format]| {
        kinds.iter().find_map(|kind| {
            formats
                .iter()
                .find(|f| f.format == *kind && spaces.contains(&f.color_space))
                .copied()
        })
    };
    let hdr = match want {
        ColorSpace::Sdr => None,
        ColorSpace::Hdr10 => find(
            &[vk::ColorSpaceKHR::HDR10_ST2084_EXT],
            &[
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::Format::A2R10G10B10_UNORM_PACK32,
            ],
        ),
        ColorSpace::ScRgb => find(
            &[vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT],
            &[vk::Format::R16G16B16A16_SFLOAT],
        ),
    };
    if let Some(format) = hdr {
        return Some((format, false));
    }
    let sdr = find(
        &[vk::ColorSpaceKHR::SRGB_NONLINEAR],
        &[
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::B8G8R8A8_SRGB,
            vk::Format::R8G8B8A8_SRGB,
        ],
    )
    .or_else(|| formats.iter().find(|f| known(f)).copied())?;
    Some((sdr, want != ColorSpace::Sdr))
}

pub fn hdr_support(formats: &[vk::SurfaceFormatKHR]) -> HdrSupport {
    HdrSupport {
        hdr10: pick_format(formats, ColorSpace::Hdr10).is_some_and(|(_, fallback)| !fallback),
        scrgb: pick_format(formats, ColorSpace::ScRgb).is_some_and(|(_, fallback)| !fallback),
    }
}

/// The extent to create with: the surface's own when it has one, else `wanted` clamped to its limits.
pub fn choose_extent(caps: &vk::SurfaceCapabilitiesKHR, wanted: [u32; 2]) -> [u32; 2] {
    if caps.current_extent.width != u32::MAX {
        return [caps.current_extent.width, caps.current_extent.height];
    }
    [
        wanted[0].clamp(caps.min_image_extent.width, caps.max_image_extent.width),
        wanted[1].clamp(caps.min_image_extent.height, caps.max_image_extent.height),
    ]
}

struct Surface {
    device: Device,
    handle: vk::SurfaceKHR,
    _window: Arc<dyn Any + Send + Sync>,
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(ext) = &self.device.inner.surface_ext {
            // SAFETY: the surface was created from this instance and no swapchain uses it any more.
            unsafe { ext.destroy_surface(self.handle, None) };
        }
    }
}

fn create_surface<W: WindowTarget>(device: &Device, window: Arc<W>) -> Result<Surface> {
    let display = window
        .display_handle()
        .map_err(|e| RenderError::Window(e.to_string()))?
        .as_raw();
    let handle = window
        .window_handle()
        .map_err(|e| RenderError::Window(e.to_string()))?
        .as_raw();
    let entry = &device.inner.entry;
    let instance = device.instance();
    // SAFETY: the raw handles come from a live window that `Surface` keeps alive, and the surface is destroyed
    // before it.
    let surface = unsafe {
        match (display, handle) {
            (_, RawWindowHandle::Win32(h)) => {
                let info = vk::Win32SurfaceCreateInfoKHR::default()
                    .hinstance(h.hinstance.map_or(0, |i| i.get()))
                    .hwnd(h.hwnd.get());
                ash::khr::win32_surface::Instance::new(entry, instance)
                    .create_win32_surface(&info, None)?
            }
            (RawDisplayHandle::Xlib(d), RawWindowHandle::Xlib(h)) => {
                let info = vk::XlibSurfaceCreateInfoKHR::default()
                    .dpy(
                        d.display
                            .map_or(std::ptr::null_mut(), |p| p.as_ptr())
                            .cast(),
                    )
                    .window(h.window);
                ash::khr::xlib_surface::Instance::new(entry, instance)
                    .create_xlib_surface(&info, None)?
            }
            (RawDisplayHandle::Xcb(d), RawWindowHandle::Xcb(h)) => {
                let info = vk::XcbSurfaceCreateInfoKHR::default()
                    .connection(d.connection.map_or(std::ptr::null_mut(), |p| p.as_ptr()))
                    .window(h.window.get());
                ash::khr::xcb_surface::Instance::new(entry, instance)
                    .create_xcb_surface(&info, None)?
            }
            (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(h)) => {
                let info = vk::WaylandSurfaceCreateInfoKHR::default()
                    .display(d.display.as_ptr())
                    .surface(h.surface.as_ptr());
                ash::khr::wayland_surface::Instance::new(entry, instance)
                    .create_wayland_surface(&info, None)?
            }
            _ => {
                return Err(RenderError::Window(
                    "this window system is not supported".into(),
                ));
            }
        }
    };
    Ok(Surface {
        device: device.clone(),
        handle: surface,
        _window: window,
    })
}

/// A presentable image and the semaphores that order work around it. Submit the work that draws into
/// [`Frame::image`] waiting on [`Frame::wait`] and signalling [`Frame::signal`], then hand the frame to
/// [`Swapchain::present`] with that submission. The image is in `UNDEFINED` layout on acquire; leave it in
/// `PRESENT_SRC_KHR` before presenting.
pub struct Frame {
    index: u32,
    slot: usize,
    image: Image,
    view: vk::ImageView,
    extent: [u32; 2],
    format: vk::SurfaceFormatKHR,
    acquire: vk::Semaphore,
    release: vk::Semaphore,
}

impl Frame {
    pub fn image(&self) -> &Image {
        &self.image
    }

    pub fn view(&self) -> vk::ImageView {
        self.view
    }

    pub fn extent(&self) -> [u32; 2] {
        self.extent
    }

    pub fn format(&self) -> Format {
        self.image.format()
    }

    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        self.format
    }

    pub fn image_index(&self) -> u32 {
        self.index
    }

    /// What the frame's first submission waits on: the image is ready.
    pub fn wait(&self) -> WaitSemaphore {
        WaitSemaphore {
            semaphore: self.acquire,
            value: 0,
            stage: vk::PipelineStageFlags2::ALL_COMMANDS,
        }
    }

    /// What the frame's last submission signals: drawing is done, present may go.
    pub fn signal(&self) -> SignalSemaphore {
        SignalSemaphore {
            semaphore: self.release,
            value: 0,
            stage: vk::PipelineStageFlags2::ALL_COMMANDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome {
    Presented,
    /// Presented, but the surface changed; the next `acquire` recreates the swapchain.
    Suboptimal,
    /// Not presented; the next `acquire` recreates the swapchain.
    OutOfDate,
}

/// A surface and its swapchain. Recreates itself after a resize or when the surface says it is out of date.
pub struct Swapchain {
    device: Device,
    queue: Queue,
    handle: vk::SwapchainKHR,
    images: Vec<Image>,
    views: Vec<ImageView>,
    format: vk::SurfaceFormatKHR,
    extent: [u32; 2],
    want: SwapchainDesc,
    present_mode: PresentMode,
    available_modes: Vec<PresentMode>,
    surface_formats: Vec<vk::SurfaceFormatKHR>,
    color_fallback: bool,
    need_recreate: bool,
    acquire: Vec<vk::Semaphore>,
    acquire_use: Vec<Option<Submission>>,
    acquire_next: usize,
    release: Vec<vk::Semaphore>,
    surface: Surface,
}

impl Swapchain {
    /// Creates a swapchain for `window`, presenting on `queue`. The device must have been made with
    /// `DeviceOptions::present`.
    pub fn new<W: WindowTarget>(
        queue: &Queue,
        window: Arc<W>,
        desc: &SwapchainDesc,
    ) -> Result<Swapchain> {
        let device = queue.device();
        if device.inner.surface_ext.is_none() || device.inner.swapchain_ext.is_none() {
            return Err(RenderError::Unsupported(
                "surface and swapchain extensions (DeviceOptions::present)",
            ));
        }
        let surface = create_surface(device, window)?;
        let mut this = Swapchain {
            device: device.clone(),
            queue: queue.clone(),
            handle: vk::SwapchainKHR::null(),
            images: Vec::new(),
            views: Vec::new(),
            format: vk::SurfaceFormatKHR::default(),
            extent: [0, 0],
            want: *desc,
            present_mode: PresentMode::Fifo,
            available_modes: Vec::new(),
            surface_formats: Vec::new(),
            color_fallback: false,
            need_recreate: true,
            acquire: Vec::new(),
            acquire_use: Vec::new(),
            acquire_next: 0,
            release: Vec::new(),
            surface,
        };
        // SAFETY: the surface and physical device are live and belong to this instance.
        let supported = unsafe {
            device
                .inner
                .surface_ext
                .as_ref()
                .expect("checked above")
                .get_physical_device_surface_support(
                    device.physical_device(),
                    device.queue_family(),
                    this.surface.handle,
                )?
        };
        if !supported {
            return Err(RenderError::NoPresentSupport);
        }
        this.recreate()?;
        Ok(this)
    }

    pub fn extent(&self) -> [u32; 2] {
        self.extent
    }

    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        self.format
    }

    pub fn format(&self) -> Option<Format> {
        Format::from_vk(self.format.format)
    }

    /// The mode in use: the one asked for if the surface lists it, else FIFO.
    pub fn present_mode(&self) -> PresentMode {
        self.present_mode
    }

    pub fn available_present_modes(&self) -> &[PresentMode] {
        &self.available_modes
    }

    /// Every format and colour space the surface lists.
    pub fn surface_formats(&self) -> &[vk::SurfaceFormatKHR] {
        &self.surface_formats
    }

    pub fn hdr_support(&self) -> HdrSupport {
        hdr_support(&self.surface_formats)
    }

    /// True when the colour space asked for was not available and SDR was chosen.
    pub fn color_space_fell_back(&self) -> bool {
        self.color_fallback
    }

    /// Tells the swapchain the window's new size in pixels; takes effect at the next `acquire`.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.want.extent = [width, height];
        self.need_recreate = true;
    }

    pub fn set_present_mode(&mut self, mode: PresentMode) {
        self.want.present_mode = mode;
        self.need_recreate = true;
    }

    pub fn set_color_space(&mut self, color_space: ColorSpace) {
        self.want.color_space = color_space;
        self.need_recreate = true;
    }

    fn destroy_swapchain_objects(&mut self) {
        // SAFETY: callers wait for the device to go idle first, so no work uses these objects.
        unsafe {
            for sem in self.acquire.drain(..).chain(self.release.drain(..)) {
                self.device.raw().destroy_semaphore(sem, None);
            }
            self.views.clear();
            self.images.clear();
            self.acquire_use.clear();
            if self.handle != vk::SwapchainKHR::null()
                && let Some(ext) = &self.device.inner.swapchain_ext
            {
                ext.destroy_swapchain(self.handle, None);
            }
            self.handle = vk::SwapchainKHR::null();
        }
    }

    /// Rebuilds the swapchain for the current size and settings. Returns false, and leaves the swapchain
    /// unusable until the next call, while the surface has no area (a minimised window).
    fn recreate(&mut self) -> Result<bool> {
        let surface_ext = self
            .device
            .inner
            .surface_ext
            .as_ref()
            .expect("checked in new");
        let swapchain_ext = self
            .device
            .inner
            .swapchain_ext
            .as_ref()
            .expect("checked in new");
        let physical = self.device.physical_device();
        // SAFETY: the surface and physical device are live; the old swapchain is retired by the create call and
        // destroyed after the device is idle.
        unsafe {
            let caps = surface_ext
                .get_physical_device_surface_capabilities(physical, self.surface.handle)?;
            let extent = choose_extent(&caps, self.want.extent);
            if extent[0] == 0 || extent[1] == 0 {
                self.extent = [0, 0];
                self.need_recreate = true;
                return Ok(false);
            }
            self.surface_formats =
                surface_ext.get_physical_device_surface_formats(physical, self.surface.handle)?;
            self.available_modes = surface_ext
                .get_physical_device_surface_present_modes(physical, self.surface.handle)?
                .into_iter()
                .filter_map(|m| {
                    [
                        PresentMode::Fifo,
                        PresentMode::FifoRelaxed,
                        PresentMode::Mailbox,
                        PresentMode::Immediate,
                    ]
                    .into_iter()
                    .find(|p| p.vk() == m)
                })
                .collect();
            let (format, fallback) = pick_format(&self.surface_formats, self.want.color_space)
                .ok_or(RenderError::Unsupported("a surface format render knows"))?;
            self.color_fallback = fallback;
            self.present_mode = self.want.present_mode.choose(&self.available_modes);
            let mut count = self.want.image_count.max(caps.min_image_count);
            if caps.max_image_count > 0 {
                count = count.min(caps.max_image_count);
            }
            let base_usage =
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST;
            let usage = (base_usage | self.want.usage) & caps.supported_usage_flags;
            let alpha = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            ]
            .into_iter()
            .find(|a| caps.supported_composite_alpha.contains(*a))
            .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
            self.device.wait_idle()?;
            let old = self.handle;
            let info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface.handle)
                .min_image_count(count)
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(vk::Extent2D {
                    width: extent[0],
                    height: extent[1],
                })
                .image_array_layers(1)
                .image_usage(usage)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(alpha)
                .present_mode(self.present_mode.vk())
                .clipped(true)
                .old_swapchain(old);
            let created = swapchain_ext.create_swapchain(&info, None);
            self.views.clear();
            self.images.clear();
            for sem in self.acquire.drain(..).chain(self.release.drain(..)) {
                self.device.raw().destroy_semaphore(sem, None);
            }
            self.acquire_use.clear();
            if old != vk::SwapchainKHR::null() {
                swapchain_ext.destroy_swapchain(old, None);
            }
            self.handle = vk::SwapchainKHR::null();
            self.handle = created?;
            let raw_images = swapchain_ext.get_swapchain_images(self.handle)?;
            let known_format =
                Format::from_vk(format.format).expect("pick_format returns known formats");
            for raw in &raw_images {
                let image = Image::from_swapchain(&self.device, *raw, known_format, extent, usage);
                self.views
                    .push(image.create_view(&ViewDesc::new(vk::ImageViewType::TYPE_2D))?);
                self.images.push(image);
            }
            for _ in 0..raw_images.len() + 1 {
                self.acquire.push(
                    self.device
                        .raw()
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?,
                );
                self.acquire_use.push(None);
            }
            for _ in 0..raw_images.len() {
                self.release.push(
                    self.device
                        .raw()
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?,
                );
            }
            self.format = format;
            self.extent = extent;
            self.acquire_next = 0;
            self.need_recreate = false;
        }
        Ok(true)
    }

    /// Gets the next image to draw into, recreating the swapchain first when it is out of date. `None` while the
    /// window has no area.
    pub fn acquire(&mut self) -> Result<Option<Frame>> {
        let mut attempts = 0;
        loop {
            if self.need_recreate && !self.recreate()? {
                return Ok(None);
            }
            let slot = self.acquire_next % self.acquire.len();
            if let Some(submission) = self.acquire_use[slot].take() {
                submission.wait()?;
            }
            let semaphore = self.acquire[slot];
            let ext = self
                .device
                .inner
                .swapchain_ext
                .as_ref()
                .expect("checked in new");
            // SAFETY: the swapchain and semaphore are live; the semaphore is unsignalled (its last use finished).
            let result = unsafe {
                ext.acquire_next_image(self.handle, u64::MAX, semaphore, vk::Fence::null())
            };
            match result {
                Ok((index, suboptimal)) => {
                    if suboptimal {
                        self.need_recreate = true;
                    }
                    self.acquire_next += 1;
                    let index = index as usize;
                    return Ok(Some(Frame {
                        index: index as u32,
                        slot,
                        image: self.images[index].clone(),
                        view: self.views[index].handle(),
                        extent: self.extent,
                        format: self.format,
                        acquire: semaphore,
                        release: self.release[index],
                    }));
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) if attempts < 3 => {
                    attempts += 1;
                    self.need_recreate = true;
                }
                Err(vk::Result::ERROR_SURFACE_LOST_KHR) => return Err(RenderError::SurfaceLost),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Presents `frame` once `submission` (the one that waited on `frame.wait()` and signalled `frame.signal()`)
    /// has run.
    pub fn present(&mut self, frame: Frame, submission: &Submission) -> Result<PresentOutcome> {
        self.acquire_use[frame.slot] = Some(submission.clone());
        let waits = [frame.release];
        let swapchains = [self.handle];
        let indices = [frame.index];
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices);
        match self.queue.present(&info) {
            Ok(false) => Ok(PresentOutcome::Presented),
            Ok(true) => {
                self.need_recreate = true;
                Ok(PresentOutcome::Suboptimal)
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.need_recreate = true;
                Ok(PresentOutcome::OutOfDate)
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => Err(RenderError::SurfaceLost),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        let _ = self.device.wait_idle();
        self.destroy_swapchain_objects();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sf(format: vk::Format, color_space: vk::ColorSpaceKHR) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format,
            color_space,
        }
    }

    #[test]
    fn present_mode_falls_back_to_fifo() {
        let modes = [PresentMode::Fifo, PresentMode::Mailbox];
        assert_eq!(PresentMode::Mailbox.choose(&modes), PresentMode::Mailbox);
        assert_eq!(PresentMode::Immediate.choose(&modes), PresentMode::Fifo);
    }

    #[test]
    fn sdr_prefers_unorm_over_srgb() {
        let list = [
            sf(vk::Format::B8G8R8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
            sf(
                vk::Format::B8G8R8A8_UNORM,
                vk::ColorSpaceKHR::SRGB_NONLINEAR,
            ),
        ];
        let (picked, fallback) = pick_format(&list, ColorSpace::Sdr).unwrap();
        assert_eq!(picked.format, vk::Format::B8G8R8A8_UNORM);
        assert!(!fallback);
    }

    #[test]
    fn hdr_is_found_or_reported_as_a_fallback() {
        let sdr_only = [sf(
            vk::Format::B8G8R8A8_UNORM,
            vk::ColorSpaceKHR::SRGB_NONLINEAR,
        )];
        let (_, fallback) = pick_format(&sdr_only, ColorSpace::Hdr10).unwrap();
        assert!(fallback);
        assert_eq!(hdr_support(&sdr_only), HdrSupport::default());
        let both = [
            sf(
                vk::Format::B8G8R8A8_UNORM,
                vk::ColorSpaceKHR::SRGB_NONLINEAR,
            ),
            sf(
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            ),
            sf(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
            ),
        ];
        assert_eq!(
            hdr_support(&both),
            HdrSupport {
                hdr10: true,
                scrgb: true
            }
        );
        let (picked, fallback) = pick_format(&both, ColorSpace::ScRgb).unwrap();
        assert_eq!(picked.format, vk::Format::R16G16B16A16_SFLOAT);
        assert!(!fallback);
    }

    #[test]
    fn extent_follows_the_surface_or_clamps() {
        let mut caps = vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: 800,
                height: 600,
            },
            ..Default::default()
        };
        assert_eq!(choose_extent(&caps, [1, 1]), [800, 600]);
        caps.current_extent = vk::Extent2D {
            width: u32::MAX,
            height: u32::MAX,
        };
        caps.min_image_extent = vk::Extent2D {
            width: 16,
            height: 16,
        };
        caps.max_image_extent = vk::Extent2D {
            width: 4096,
            height: 2160,
        };
        assert_eq!(choose_extent(&caps, [8000, 4]), [4096, 16]);
    }
}
