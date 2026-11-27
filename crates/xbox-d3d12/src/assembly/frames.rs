//! Frame events and presentation: the device's frame calls (slots 78 to 81), the queue's `PresentX`,
//! `SuspendX` and `ResumeX`, and the policy that turns a frame limit into the interval the grid runs at.
//!
//! The title waits on two event types: type 0, the frame origin, which paces it, and type 2, whose meaning is not
//! known (the render thread waits on it with the token the main thread stored; the executable's own frame-rate
//! notes say a presenter should not feed it to the origin grid). A type 2 wait returns at once without a token, and
//! its `ScheduleFrameEventX` is accepted and ignored. The same thread polls the current token with a type 0 wait of
//! timeout 0 and flag 2; that returns the latest token and `S_FALSE` without taking a new origin.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ash::vk;

use super::objects::resource_of;
use super::state::{Assembly, current};
use crate::com::{self, S_FALSE, S_OK};
use crate::device::FrameEvents;
use crate::pacing::{FrameLimit, Pacer, SyncHint};
use crate::present::convert::Rect;
use crate::present::frames::WAIT_FLAG_TOKEN_ON_TIMEOUT;
use crate::present::{
    FramePolicy, Pacing, Plane, PlaneParameters, PresentParameters, ResourceResolver, SourceImage,
    SyncMode, ViewRect,
};

/// The origin frame event, and the one whose meaning is not known.
pub const EVENT_ORIGIN: u32 = 0;
pub const EVENT_RENDER: u32 = 2;

/// Turns the frame limit into the grid the presenter runs: the interval the title asked for under the console
/// limit, one from the cap, none (no wait) when uncapped or left to vertical sync.
pub struct PacerPolicy {
    pacer: Arc<Pacer>,
}

impl PacerPolicy {
    pub fn new(pacer: Arc<Pacer>) -> PacerPolicy {
        PacerPolicy { pacer }
    }
}

impl FramePolicy for PacerPolicy {
    fn pacing(&self, requested_interval_us: u32) -> Pacing {
        let sync = match self.pacer.sync_hint() {
            SyncHint::Vsync => SyncMode::Vsync,
            SyncHint::Paced => SyncMode::Mailbox,
            SyncHint::Immediate => SyncMode::Immediate,
        };
        let interval_us = match self.pacer.limit() {
            FrameLimit::Console => requested_interval_us,
            FrameLimit::Cap(fps) => (1_000_000.0 / fps).round().max(1.0) as u32,
            FrameLimit::Uncapped | FrameLimit::VSync => 0,
        };
        Pacing { interval_us, sync }
    }
}

/// The device's frame calls over `Presentation`.
pub struct AssemblyFrames {
    asm: Arc<Assembly>,
}

impl AssemblyFrames {
    pub fn new(asm: Arc<Assembly>) -> AssemblyFrames {
        AssemblyFrames { asm }
    }
}

impl FrameEvents for AssemblyFrames {
    fn set_interval(&self, output: usize, interval_us: u32, period: usize, flags: u32) -> u32 {
        self.asm
            .presentation
            .set_frame_interval_x(output as u64, interval_us, period as u32, flags) as u32
    }

    fn schedule(&self, event_type: u32, offset_us: u32, _ancillary: usize, flags: u32) -> u32 {
        if event_type == EVENT_RENDER {
            com::note_once("ScheduleFrameEventX with event type 2 is accepted and not scheduled");
            return S_OK;
        }
        self.asm
            .presentation
            .schedule_frame_event_x(event_type, offset_us, flags) as u32
    }

    unsafe fn wait(
        &self,
        event_type: u32,
        timeout_ms: u32,
        _ancillary: usize,
        flags: u32,
        token: *mut u64,
    ) -> u32 {
        let write = |value: u64| {
            if !token.is_null() {
                // SAFETY: the title's out parameter.
                unsafe { *token = value };
            }
        };
        if event_type == EVENT_RENDER {
            com::note_once(
                "WaitFrameEventX with event type 2 returns at once: its meaning is not known",
            );
            return S_OK;
        }
        if timeout_ms == 0 && flags & WAIT_FLAG_TOKEN_ON_TIMEOUT != 0 {
            write(self.asm.latest_token.load(Ordering::Acquire));
            return S_FALSE;
        }
        let result = self
            .asm
            .presentation
            .wait_frame_event_x(event_type, timeout_ms, flags);
        if result.hresult == S_OK as i32 {
            self.asm.latest_token.store(result.token, Ordering::Release);
            write(result.token);
        } else if flags & WAIT_FLAG_TOKEN_ON_TIMEOUT != 0 {
            write(result.token);
        }
        result.hresult as u32
    }

    unsafe fn statistics(
        &self,
        token: u64,
        type_mask: u32,
        count: *mut u32,
        _stats: *mut c_void,
    ) -> u32 {
        // SAFETY: `count` is writable or null.
        let capacity = unsafe { count.as_ref() }.copied().unwrap_or(0) as usize;
        let (hresult, entries) = self
            .asm
            .presentation
            .get_frame_statistics_x(token, type_mask, capacity);
        if !count.is_null() {
            // SAFETY: as above.
            unsafe { *count = entries.len() as u32 };
        }
        if !entries.is_empty() {
            com::note_once(
                "GetFrameStatisticsX counted its entries; the console entry layout is not measured, so none are written",
            );
        }
        hresult as u32
    }
}

/// Back buffers as the presenter reads them.
struct Resolver<'a>(&'a Assembly);

impl ResourceResolver for Resolver<'_> {
    fn source_image(&self, resource: *mut c_void) -> Option<SourceImage> {
        // SAFETY: the title's `ID3D12Resource*`.
        let id = unsafe { resource_of(resource) }?.id;
        let resources = self.0.resources();
        let r = resources.get(id)?;
        let image = r.image.as_ref()?;
        let mut usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        if !r.desc.has_flag(crate::resource::desc::DENY_SHADER_RESOURCE) {
            usage |= vk::ImageUsageFlags::SAMPLED;
        }
        Some(SourceImage {
            image: image.image,
            format: image.format,
            extent: (image.extent[0], image.extent[1]),
            layout: vk::ImageLayout::GENERAL,
            usage,
            samples: vk::SampleCountFlags::from_raw(image.samples),
            waits: Vec::new(),
        })
    }
}

fn rect_of(p: *const ViewRect) -> Option<ViewRect> {
    // SAFETY: a non-null rectangle pointer names at least one rectangle.
    (!p.is_null()).then(|| unsafe { *p })
}

/// `PresentX(plane_count, planes, params)`: the planes are resolved now, while the title's structures are alive,
/// and the present runs on the queue's worker after the work submitted before it.
unsafe extern "win64" fn q_present_x(
    this: *mut c_void,
    plane_count: u32,
    planes: *const PlaneParameters,
    params: *const PresentParameters,
) -> u32 {
    let Some(asm) = current() else {
        return crate::present::E_FAIL as u32;
    };
    if plane_count as usize > crate::present::MAX_PLANES || (plane_count > 0 && planes.is_null()) {
        return crate::present::E_INVALIDARG as u32;
    }
    let resolver = Resolver(&asm);
    let mut list = Vec::with_capacity(plane_count as usize);
    for i in 0..plane_count as usize {
        // SAFETY: `plane_count` valid entries.
        let p = unsafe { *planes.add(i) };
        let source = if p.resource_count >= 1 && !p.resources.is_null() {
            // SAFETY: the resource array holds `resource_count` entries.
            resolver.source_image(unsafe { *p.resources })
        } else {
            None
        };
        list.push(Plane {
            token: p.token,
            resource_count: p.resource_count,
            source,
            src_rect: rect_of(p.src_view_rects).map(|r| Rect {
                left: r.left.round() as i32,
                top: r.top.round() as i32,
                right: r.right.round() as i32,
                bottom: r.bottom.round() as i32,
            }),
            placement: rect_of(p.dest_placement_base).map(|r| [r.left, r.top, r.right, r.bottom]),
            color_space: p.color_space,
            scale_filter: p.scale_filter,
        });
    }
    let threshold = if params.is_null() {
        0.0
    } else {
        // SAFETY: a non-null parameter block is valid.
        unsafe { (*params).immediate_threshold_percent }
    };
    let queued = asm.clone();
    let run = Box::new(move || {
        let result = queued.presentation.present_planes(&list, threshold);
        if result != 0 {
            com::note_once(format!("PresentX failed with {result:#x}"));
        }
    });
    // SAFETY: `this` is the queue the title called.
    if unsafe { crate::queue::enqueue(this, run) } {
        S_OK
    } else {
        crate::present::E_INVALIDARG as u32
    }
}

unsafe extern "win64" fn q_suspend_x(_this: *mut c_void, flags: u32) -> u32 {
    current().map_or(S_OK, |a| a.presentation.suspend_x(flags) as u32)
}

unsafe extern "win64" fn q_resume_x(_this: *mut c_void) -> u32 {
    current().map_or(S_OK, |a| a.presentation.resume_x() as u32)
}

/// `PresentX` is slot 30 in the older console runtime layout and 32 in the newer; which one this title has is not
/// decided, so both answer. `SuspendX` and `ResumeX` are placed before the older `PresentX`, which is a guess:
/// a call to any other queue slot is noted by the trap with its number.
pub const PRESENT_SLOTS: [usize; 2] = [30, 32];
pub const SUSPEND_SLOT: usize = 28;
pub const RESUME_SLOT: usize = 29;

pub fn queue_table() -> Vec<(usize, usize)> {
    let present = q_present_x as *const () as usize;
    vec![
        (SUSPEND_SLOT, q_suspend_x as *const () as usize),
        (RESUME_SLOT, q_resume_x as *const () as usize),
        (PRESENT_SLOTS[0], present),
        (PRESENT_SLOTS[1], present),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_decides_the_interval_and_the_present_mode() {
        let policy = |limit| PacerPolicy::new(Arc::new(Pacer::new(limit)));
        let console = policy(FrameLimit::Console).pacing(33_333);
        assert_eq!(console.interval_us, 33_333);
        assert_eq!(console.sync, SyncMode::Mailbox);
        let cap = policy(FrameLimit::Cap(144.0)).pacing(16_667);
        assert_eq!(cap.interval_us, 6_944);
        assert_eq!(
            policy(FrameLimit::Uncapped).pacing(16_667).sync,
            SyncMode::Immediate
        );
        let vsync = policy(FrameLimit::VSync).pacing(16_667);
        assert_eq!((vsync.interval_us, vsync.sync), (0, SyncMode::Vsync));
    }
}
