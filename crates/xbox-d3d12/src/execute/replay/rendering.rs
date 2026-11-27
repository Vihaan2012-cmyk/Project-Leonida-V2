//! The dynamic-rendering scope: when it opens, what it loads, and clears of the targets it holds.

use ash::vk;

use super::Replayer;
use super::util::{attachment, rect2d};
use crate::execute::command::*;
use crate::pipeline::state::has_stencil;

fn color_value(color: [f32; 4]) -> vk::ClearValue {
    vk::ClearValue {
        color: vk::ClearColorValue { float32: color },
    }
}

fn depth_value(depth: f32, stencil: u8) -> vk::ClearValue {
    vk::ClearValue {
        depth_stencil: vk::ClearDepthStencilValue {
            depth,
            stencil: u32::from(stencil),
        },
    }
}

/// The `vkCmdClearAttachments` entry for a clear of `color`, or of the aspects `ds` names in a view of `format`.
fn clear_attachment(
    format: vk::Format,
    color: Option<[f32; 4]>,
    ds: Option<(Option<f32>, Option<u8>)>,
    slot: u32,
) -> Option<vk::ClearAttachment> {
    match (color, ds) {
        (Some(c), _) => Some(vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            color_attachment: slot,
            clear_value: color_value(c),
        }),
        (None, Some((d, s))) => {
            let mut aspect = vk::ImageAspectFlags::empty();
            if d.is_some() {
                aspect |= vk::ImageAspectFlags::DEPTH;
            }
            if s.is_some() && has_stencil(format) {
                aspect |= vk::ImageAspectFlags::STENCIL;
            }
            Some(vk::ClearAttachment {
                aspect_mask: aspect,
                color_attachment: 0,
                clear_value: depth_value(d.unwrap_or(0.0), s.unwrap_or(0)),
            })
        }
        (None, None) => None,
    }
}

fn clear_rects(rects: &[Rect], view: &TargetView) -> Vec<vk::ClearRect> {
    if rects.is_empty() {
        return vec![vk::ClearRect {
            rect: vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D {
                    width: view.extent[0],
                    height: view.extent[1],
                },
            },
            base_array_layer: 0,
            layer_count: 1,
        }];
    }
    rects
        .iter()
        .map(|r| vk::ClearRect {
            rect: rect2d(*r),
            base_array_layer: 0,
            layer_count: 1,
        })
        .collect()
}

impl Replayer<'_> {
    /// Closes the scope, and gives clears that never met a draw their own scope.
    pub(super) fn end_rendering(&mut self) {
        if self.clears.any && !self.rendering {
            self.begin_rendering();
        }
        if self.rendering {
            // SAFETY: a rendering scope is open on `cb`.
            unsafe { self.ctx.device.cmd_end_rendering(self.cb) };
            self.rendering = false;
        }
    }

    pub(super) fn render_area(&self, views: impl Iterator<Item = TargetView>) -> vk::Rect2D {
        let (w, h) = views.fold((u32::MAX, u32::MAX), |(w, h), v| {
            (w.min(v.extent[0]), h.min(v.extent[1]))
        });
        vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: if w == u32::MAX { 1 } else { w },
                height: if h == u32::MAX { 1 } else { h },
            },
        }
    }

    /// Opens a scope on the bound targets, loading them or clearing those with a clear waiting.
    pub(super) fn begin_rendering(&mut self) {
        if self.rendering {
            return;
        }
        let clears = std::mem::take(&mut self.clears);
        let mut colors = [vk::RenderingAttachmentInfo::default(); 8];
        let count = self.colors.len().min(colors.len());
        for (i, v) in self.colors.iter().take(count).enumerate() {
            colors[i] = attachment(
                v.view,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                clears.colors[i].map(color_value),
            );
        }
        let layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
        let depth = self
            .depth
            .map(|v| attachment(v.view, layout, clears.depth.map(|d| depth_value(d, 0))));
        let stencil = self
            .depth
            .filter(|v| has_stencil(v.format))
            .map(|v| attachment(v.view, layout, clears.stencil.map(|s| depth_value(0.0, s))));
        let area = self.render_area(self.colors.iter().copied().chain(self.depth));
        self.open_rendering(area, &colors[..count], depth.as_ref(), stencil.as_ref());
    }

    fn open_rendering(
        &mut self,
        area: vk::Rect2D,
        colors: &[vk::RenderingAttachmentInfo<'_>],
        depth: Option<&vk::RenderingAttachmentInfo<'_>>,
        stencil: Option<&vk::RenderingAttachmentInfo<'_>>,
    ) {
        let mut info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(colors);
        if let Some(d) = depth {
            info = info.depth_attachment(d);
        }
        if let Some(s) = stencil {
            info = info.stencil_attachment(s);
        }
        // SAFETY: `cb` is recording outside a rendering scope; the views outlive the list.
        unsafe { self.ctx.device.cmd_begin_rendering(self.cb, &info) };
        self.rendering = true;
        self.report.stats.rendering_scopes += 1;
    }

    /// A clear of a target the list has bound with no rectangles waits for the scope that opens at the next
    /// draw and becomes its load operation; any other clear happens inside a scope, or in a scope of its own.
    pub(super) fn clear_target(
        &mut self,
        view: TargetView,
        color: Option<[f32; 4]>,
        ds: Option<(Option<f32>, Option<u8>)>,
        rects: &[Rect],
    ) {
        let color_slot = color.and(
            self.colors
                .iter()
                .position(|v| v.view == view.view)
                .filter(|&i| i < 8),
        );
        let depth_bound = ds.is_some() && self.depth.is_some_and(|v| v.view == view.view);
        let bound = color_slot.is_some() || depth_bound;
        if bound && rects.is_empty() && !self.rendering {
            if let (Some(slot), Some(c)) = (color_slot, color) {
                self.clears.colors[slot] = Some(c);
            }
            if let Some((d, s)) = ds {
                if d.is_some() {
                    self.clears.depth = d;
                }
                if s.is_some() && has_stencil(view.format) {
                    self.clears.stencil = s;
                }
            }
            self.clears.any = true;
            return;
        }
        if bound {
            self.begin_rendering();
        } else {
            self.end_rendering();
            self.clear_in_own_scope(view, color, ds, rects);
            return;
        }
        let Some(attachment) =
            clear_attachment(view.format, color, ds, color_slot.unwrap_or(0) as u32)
        else {
            return;
        };
        let regions = clear_rects(rects, &view);
        // SAFETY: a rendering scope with the view is open on `cb`.
        unsafe {
            self.ctx
                .device
                .cmd_clear_attachments(self.cb, &[attachment], &regions)
        };
    }

    /// Clears a view that is not one of the bound targets, in a scope that holds only it.
    fn clear_in_own_scope(
        &mut self,
        view: TargetView,
        color: Option<[f32; 4]>,
        ds: Option<(Option<f32>, Option<u8>)>,
        rects: &[Rect],
    ) {
        let whole = rects.is_empty();
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: view.extent[0],
                height: view.extent[1],
            },
        };
        let (colors, depth, stencil);
        if let Some(c) = color {
            colors = vec![attachment(
                view.view,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                whole.then(|| color_value(c)),
            )];
            depth = None;
            stencil = None;
        } else {
            let (d, s) = ds.unwrap_or((None, None));
            colors = Vec::new();
            let layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
            depth = Some(attachment(
                view.view,
                layout,
                (whole && d.is_some()).then(|| depth_value(d.unwrap_or(0.0), 0)),
            ));
            stencil = has_stencil(view.format).then(|| {
                attachment(
                    view.view,
                    layout,
                    (whole && s.is_some()).then(|| depth_value(0.0, s.unwrap_or(0))),
                )
            });
        }
        self.open_rendering(area, &colors, depth.as_ref(), stencil.as_ref());
        if !whole && let Some(attachment) = clear_attachment(view.format, color, ds, 0) {
            let regions = clear_rects(rects, &view);
            // SAFETY: the scope just opened holds the view.
            unsafe {
                self.ctx
                    .device
                    .cmd_clear_attachments(self.cb, &[attachment], &regions)
            };
        }
        // SAFETY: the scope opened above is still open.
        unsafe { self.ctx.device.cmd_end_rendering(self.cb) };
        self.rendering = false;
    }
}
