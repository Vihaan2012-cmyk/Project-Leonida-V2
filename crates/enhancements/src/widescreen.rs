//! Ultrawide and other aspect ratios: field of view, HUD placement and the patches that set them.
//!
//! Two halves. The output half (`Size`, `Rect`, `plan`) decides how a game frame reaches a window of any shape.
//! The game half (`Config::groups`) describes code patches as plain data for `patch.rs`; every group is off by
//! default and cites the addresses it was derived from in `EVIDENCE`.


use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const FULL_HD: Size = Size::new(1920, 1080);
    pub const UHD: Size = Size::new(3840, 2160);

    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn aspect(self) -> f32 {
        if self.height == 0 {
            return 0.0;
        }
        self.width as f32 / self.height as f32
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn whole(size: Size) -> Self {
        Self {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        }
    }
}

/// Black margins around a presented frame, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bars {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl Bars {
    pub fn around(window: Size, dest: Rect) -> Self {
        Self {
            left: dest.x,
            right: window.width.saturating_sub(dest.x + dest.width),
            top: dest.y,
            bottom: window.height.saturating_sub(dest.y + dest.height),
        }
    }
}

/// How a frame of one shape is shown in a window of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FitMode {
    /// Largest centred rectangle of the frame's own shape: pillarbox in a wider window, letterbox in a taller one.
    #[default]
    Fit,
    /// The frame covers the whole window; the part that does not fit is cropped.
    Fill,
    /// The frame is scaled to the window's shape and distorted.
    Stretch,
}

/// Where the frame goes in the window. `Fit` bars are split evenly; an odd leftover pixel goes to the right/bottom.
pub fn fit_rect(frame: Size, window: Size) -> Rect {
    if frame.is_empty() || window.is_empty() {
        return Rect::whole(Size::new(0, 0));
    }
    let (fw, fh) = (u64::from(frame.width), u64::from(frame.height));
    let (ww, wh) = (u64::from(window.width), u64::from(window.height));
    let (width, height) = if ww * fh >= wh * fw {
        ((wh * fw + fh / 2) / fh, wh)
    } else {
        (ww, (ww * fh + fw / 2) / fw)
    };
    Rect {
        x: ((ww - width) / 2) as u32,
        y: ((wh - height) / 2) as u32,
        width: width as u32,
        height: height as u32,
    }
}

/// The part of the frame that stays visible when it fills the window (`FitMode::Fill`).
pub fn fill_source(frame: Size, window: Size) -> Rect {
    if frame.is_empty() || window.is_empty() {
        return Rect::whole(Size::new(0, 0));
    }
    let (fw, fh) = (u64::from(frame.width), u64::from(frame.height));
    let (ww, wh) = (u64::from(window.width), u64::from(window.height));
    let (width, height) = if ww * fh >= wh * fw {
        (fw, (fw * wh + ww / 2) / ww)
    } else {
        ((fh * ww + wh / 2) / wh, fh)
    };
    Rect {
        x: ((fw - width) / 2) as u32,
        y: ((fh - height) / 2) as u32,
        width: width as u32,
        height: height as u32,
    }
}

/// What to render and how to put it in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// The resolution the game should render at (the size its display globals hold).
    pub render: Size,
    /// The part of the rendered frame that is shown.
    pub source: Rect,
    /// Where it lands in the window.
    pub dest: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanOptions {
    pub mode: FitMode,
    /// Render at the window's own shape (needs the aspect patches) instead of presenting a 16:9 frame.
    pub native: bool,
    /// Most render lines the game is asked for in native mode.
    pub max_render_height: u32,
    /// Render sizes are rounded to a multiple of this (the game's effect buffers use 16).
    pub align: u32,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            mode: FitMode::Fit,
            native: false,
            max_render_height: 2160,
            align: 16,
        }
    }
}

/// A frame of the game's own size (`base`, normally 16:9) shown in `window`, or a frame of the window's shape when
/// `options.native` is set.
pub fn plan(base: Size, window: Size, options: PlanOptions) -> Plan {
    let render = if options.native && !window.is_empty() {
        native_render_size(window, options.max_render_height, options.align)
    } else {
        base
    };
    let whole = Rect::whole(render);
    let (source, dest) = match options.mode {
        FitMode::Fit => (whole, fit_rect(render, window)),
        FitMode::Fill => (fill_source(render, window), Rect::whole(window)),
        FitMode::Stretch => (whole, Rect::whole(window)),
    };
    Plan {
        render,
        source,
        dest,
    }
}

/// The render size for a window: its shape, no taller than `max_height`, each side rounded to `align` (at least one
/// step), and the shape kept as closely as the rounding allows.
pub fn native_render_size(window: Size, max_height: u32, align: u32) -> Size {
    let align = align.max(1);
    if window.is_empty() {
        return Size::new(align, align);
    }
    let height = window.height.min(max_height.max(align));
    let ideal_width = f64::from(height) * f64::from(window.width) / f64::from(window.height);
    let round = |value: f64| ((value / f64::from(align)).round() as u32).max(1) * align;
    Size::new(round(ideal_width), round(f64::from(height)))
}

/// The eight aspect ratios the game's shape snapping knows, in its table order.
pub const GAME_ASPECTS: [f32; 8] = [
    16.0 / 9.0,
    1.6,
    4.0 / 3.0,
    7.0 / 3.0,
    17.0 / 9.0,
    1.25,
    1.5,
    5.0 / 3.0,
];

/// The table entry closest to `aspect` by ratio.
pub fn nearest_game_aspect(aspect: f32) -> f32 {
    GAME_ASPECTS
        .iter()
        .copied()
        .min_by(|a, b| {
            let da = (aspect / a).ln().abs();
            let db = (aspect / b).ln().abs();
            da.total_cmp(&db)
        })
        .unwrap_or(16.0 / 9.0)
}

pub fn horizontal_fov(vertical_degrees: f32, aspect: f32) -> f32 {
    2.0 * ((vertical_degrees.to_radians() / 2.0).tan() * aspect)
        .atan()
        .to_degrees()
}

pub fn vertical_fov(horizontal_degrees: f32, aspect: f32) -> f32 {
    2.0 * ((horizontal_degrees.to_radians() / 2.0).tan() / aspect)
        .atan()
        .to_degrees()
}

/// The factor on the game's vertical field of view that gives `target_horizontal` degrees at `aspect`.
pub fn fov_scale_for_horizontal(
    base_vertical: f32,
    target_horizontal: f32,
    aspect: f32,
) -> Option<f32> {
    let scale = vertical_fov(target_horizontal, aspect) / base_vertical;
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

/// The game turns a field of view in degrees into a half-angle in radians with this constant (pi / 360).
pub const HALF_ANGLE_PER_DEGREE: f32 = PI / 360.0;

/// The same constant for a field of view scaled by `scale`.
pub fn scaled_half_angle(scale: f32) -> f32 {
    HALF_ANGLE_PER_DEGREE * scale
}

pub const FOV_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.8;

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn aspect_of_common_sizes() {
        assert!(near(Size::FULL_HD.aspect(), 16.0 / 9.0));
        assert!(near(Size::new(3440, 1440).aspect(), 43.0 / 18.0));
        assert!(near(Size::new(5120, 1440).aspect(), 32.0 / 9.0));
        assert_eq!(Size::new(10, 0).aspect(), 0.0);
    }

    #[test]
    fn fit_pillarboxes_a_wide_window() {
        let rect = fit_rect(Size::FULL_HD, Size::new(3440, 1440));
        assert_eq!(rect.height, 1440);
        assert_eq!(rect.width, 2560);
        assert_eq!((rect.x, rect.y), (440, 0));
        let bars = Bars::around(Size::new(3440, 1440), rect);
        assert_eq!(
            (bars.left, bars.right, bars.top, bars.bottom),
            (440, 440, 0, 0)
        );
    }

    #[test]
    fn fit_pillarboxes_super_ultrawide() {
        let rect = fit_rect(Size::UHD, Size::new(5120, 1440));
        assert_eq!((rect.width, rect.height), (2560, 1440));
        assert_eq!(rect.x, 1280);
    }

    #[test]
    fn fit_letterboxes_a_tall_window() {
        let rect = fit_rect(Size::FULL_HD, Size::new(1280, 1024));
        assert_eq!((rect.width, rect.height), (1280, 720));
        assert_eq!((rect.x, rect.y), (0, 152));
    }

    #[test]
    fn fit_leaves_a_matching_window_alone() {
        let rect = fit_rect(Size::FULL_HD, Size::new(1280, 720));
        assert_eq!(rect, Rect::whole(Size::new(1280, 720)));
    }

    #[test]
    fn fit_odd_leftover_goes_to_the_far_side() {
        let rect = fit_rect(Size::new(16, 9), Size::new(101, 9));
        assert_eq!((rect.x, rect.width), (42, 16));
        let bars = Bars::around(Size::new(101, 9), rect);
        assert_eq!((bars.left, bars.right), (42, 43));
    }

    #[test]
    fn fit_never_leaves_the_window() {
        for (ww, wh) in [(1, 1), (7, 3), (2561, 1079), (800, 2000), (3, 5000)] {
            let window = Size::new(ww, wh);
            let rect = fit_rect(Size::FULL_HD, window);
            assert!(rect.x + rect.width <= ww && rect.y + rect.height <= wh);
            assert!(rect.width == ww || rect.height == wh);
        }
        assert_eq!(fit_rect(Size::FULL_HD, Size::new(0, 5)).width, 0);
    }

    #[test]
    fn fill_crops_the_frame_to_the_window_shape() {
        let source = fill_source(Size::FULL_HD, Size::new(3440, 1440));
        assert_eq!(source.width, 1920);
        assert_eq!(source.height, 804);
        assert_eq!(source.y, 138);
        let source = fill_source(Size::FULL_HD, Size::new(1000, 1000));
        assert_eq!((source.width, source.height, source.x), (1080, 1080, 420));
    }

    #[test]
    fn native_size_follows_the_window_in_steps() {
        let size = native_render_size(Size::new(3440, 1440), 2160, 16);
        assert_eq!(size, Size::new(3440, 1440));
        let size = native_render_size(Size::new(5120, 1440), 1080, 16);
        assert_eq!(size.height, 1088);
        assert_eq!(size.width % 16, 0);
        assert!((size.aspect() - 32.0 / 9.0).abs() < 0.03);
        assert_eq!(
            native_render_size(Size::new(0, 0), 1080, 16),
            Size::new(16, 16)
        );
    }

    #[test]
    fn native_size_is_capped_by_the_height_limit() {
        let size = native_render_size(Size::new(7680, 2160), 1080, 16);
        assert!(size.height <= 1088);
        assert!((size.aspect() - 7680.0 / 2160.0).abs() < 0.05);
    }

    #[test]
    fn plan_presents_sixteen_nine_with_bars_by_default() {
        let plan = plan(Size::FULL_HD, Size::new(3440, 1440), PlanOptions::default());
        assert_eq!(plan.render, Size::FULL_HD);
        assert_eq!(plan.source, Rect::whole(Size::FULL_HD));
        assert_eq!(plan.dest.width, 2560);
    }

    #[test]
    fn plan_native_fills_the_window() {
        let options = PlanOptions {
            native: true,
            max_render_height: 1440,
            ..PlanOptions::default()
        };
        let plan = plan(Size::FULL_HD, Size::new(3440, 1440), options);
        assert_eq!(plan.render, Size::new(3440, 1440));
        assert_eq!(plan.dest, Rect::whole(Size::new(3440, 1440)));
    }

    #[test]
    fn plan_stretch_and_fill() {
        let window = Size::new(3440, 1440);
        let stretch = plan(
            Size::FULL_HD,
            window,
            PlanOptions {
                mode: FitMode::Stretch,
                ..PlanOptions::default()
            },
        );
        assert_eq!(stretch.dest, Rect::whole(window));
        assert_eq!(stretch.source, Rect::whole(Size::FULL_HD));
        let fill = plan(
            Size::FULL_HD,
            window,
            PlanOptions {
                mode: FitMode::Fill,
                ..PlanOptions::default()
            },
        );
        assert_eq!(fill.dest, Rect::whole(window));
        assert_eq!(fill.source.width, 1920);
    }

    #[test]
    fn game_aspect_table_snaps_nearby_shapes() {
        assert!(near(nearest_game_aspect(1.78), 16.0 / 9.0));
        assert!(near(nearest_game_aspect(2.39), 7.0 / 3.0));
        assert!(near(nearest_game_aspect(1.33), 4.0 / 3.0));
        assert!(near(nearest_game_aspect(1.59), 1.6));
        assert!(near(nearest_game_aspect(1.26), 1.25));
    }

    #[test]
    fn fov_conversions_round_trip() {
        let horizontal = horizontal_fov(60.0, 16.0 / 9.0);
        assert!((horizontal - 91.49).abs() < 0.05, "{horizontal}");
        assert!((vertical_fov(horizontal, 16.0 / 9.0) - 60.0).abs() < 1e-3);
        assert!((horizontal_fov(60.0, 1.0) - 60.0).abs() < 1e-3);
        assert!(horizontal_fov(60.0, 7.0 / 3.0) > horizontal);
    }

    #[test]
    fn fov_scale_for_a_target_horizontal_angle() {
        let scale = fov_scale_for_horizontal(60.0, 91.49, 16.0 / 9.0).unwrap();
        assert!((scale - 1.0).abs() < 0.001);
        let wide = fov_scale_for_horizontal(60.0, 120.0, 16.0 / 9.0).unwrap();
        assert!(wide > 1.0);
        assert!(fov_scale_for_horizontal(0.0, 90.0, 1.0).is_none());
    }

    #[test]
    fn half_angle_constant_is_the_games() {
        assert_eq!(HALF_ANGLE_PER_DEGREE.to_bits(), 0x3c0e_fa35);
        assert_eq!(scaled_half_angle(1.0), HALF_ANGLE_PER_DEGREE);
        assert!(near(scaled_half_angle(2.0) / HALF_ANGLE_PER_DEGREE, 2.0));
    }

}
