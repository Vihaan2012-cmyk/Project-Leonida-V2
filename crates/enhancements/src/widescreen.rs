//! Ultrawide and other aspect ratios: field of view, HUD placement and the patches that set them.
//!
//! Two halves. The output half (`Size`, `Rect`, `plan`) decides how a game frame reaches a window of any shape.
//! The game half (`Config::groups`) describes code patches as plain data for `patch.rs`; every group is off by
//! default and cites the addresses it was derived from in `EVIDENCE`.


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

}
