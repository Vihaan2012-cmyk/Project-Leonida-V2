//! The in-loop deblocking of a decoded frame.
//!
//! The game's Bink DLL decodes a frame macroblock by macroblock and runs its filters a block or two behind the
//! decoder (they need the neighbours to the right and in the next block row). The part of it that works on samples
//! is `sub_00010760` (RVA 0x10760): it first renders flat intra blocks as smooth surfaces (see `smooth`), then
//! filters vertical block edges (`sub_0001c5e0`, 0x1c5e0), then horizontal ones (`sub_0001c780`, 0x1c780). The
//! decoder never reads a sample the filters have not finished with, so running the same three steps over the whole
//! frame afterwards gives the same picture.
//!
//! Which edges are filtered, and how hard:
//! * An edge between two 8x8 blocks is a candidate on every 8-sample line. A frame flag (0x4000 for vertical
//!   edges, 0x8000 for horizontal ones) switches a direction off, and when the frame carries edge flags (0x10000)
//!   each line has one more flag that switches that line off. The picture's own border is never filtered.
//! * When either block is intra the strength comes from the two blocks' coefficient classes (a block with eight or
//!   more coefficients is left alone, one with four to seven is touched on its inner sample only, the others on two
//!   samples), and both blocks' samples move only on their own side.
//! * When both blocks are inter, only edges between 16x16 motion quarters are filtered, with full strength, and
//!   only if their vectors differ by more than one unit in either component. The filters of the residue itself
//!   (RVA 0x159e0, 0x15dc0) are dead for these streams: they run only while frame flag 0x40000 is clear, and every
//!   frame of the game's movies sets it.
//! * Every edge filter moves up to two samples on each side of the edge: with `d = q0 - p0` the outer samples
//!   change by `(d + 4) >> 3` and the inner ones by `(d + 2) >> 2`, towards each other.
//!
//! Further facts read from the library:
//! * The class of a block is counted where the coefficients are read (RVA 0x150f0): fewer than 8 coefficients
//!   raise it by one, fewer than 4 by one more, none by a third; the class sits in bits 13 and 14 of the block's
//!   information word beside the DC value (13 bits) and the intra bit (15). Blocks of inter macroblocks keep an
//!   all-zero word (RVA 0x17f00 clears them), so next to an intra block only the intra side of the edge moves.
//! * Chroma blocks are 8x8 as well, so a chroma edge stands for 16 luma samples: its edge flag is the one of the
//!   luma line at twice the position, and every chroma block boundary is a motion quarter boundary.
//! * The edge flag lists are read per luma line of 8 samples (index 0 is the border and never set); a set flag
//!   leaves that line alone.
//! * Block rows past the picture (macroblock rows that hold only 16 rows of picture) are not filtered, and the
//!   smoothing of the last picture row looks at the row below it as parsed.
//! * Slice boundaries need no care: the library defers the edges across a boundary when slices are decoded by
//!   different threads and ends with the same picture.

use super::motion::Mvs;
use super::smooth;
use super::{Frame, Plane};

/// Set in the information word of an intra block.
pub const INTRA: u16 = 0x8000;

/// The information word of a block: whether it is intra, a class from the number of coefficients it carries, and
/// its DC value.
pub fn info_word(intra: bool, coefficients: u32, dc: i32) -> u16 {
    let class: u16 = match coefficients {
        0 => 3,
        1..=3 => 2,
        4..=7 => 1,
        _ => 0,
    };
    (if intra { INTRA } else { 0 }) | (class << 13) | (dc as u16 & 0x1fff)
}

/// A block of an intra macroblock with no coefficients: smoothed instead of flat.
fn flat(info: u16) -> bool {
    info >= 0xE000
}

fn class(info: u16) -> u16 {
    (info >> 13) & 3
}

/// What the decoder records for the filters: one information word per 8x8 block of every plane and the vectors of
/// every macroblock.
pub struct Meta {
    pub planes: Vec<Grid>,
    pub mvs: Vec<Mvs>,
    pub mb_width: usize,
}

pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub info: Vec<u16>,
}

impl Grid {
    fn new(width: usize, height: usize) -> Grid {
        Grid {
            width,
            height,
            info: vec![0; width * height],
        }
    }
}

impl Meta {
    pub fn new(aligned_width: usize, aligned_height: usize, alpha: bool) -> Meta {
        let mut planes = vec![
            Grid::new(aligned_width / 8, aligned_height / 8),
            Grid::new(aligned_width / 16, aligned_height / 16),
            Grid::new(aligned_width / 16, aligned_height / 16),
        ];
        if alpha {
            planes.push(Grid::new(aligned_width / 8, aligned_height / 8));
        }
        Meta {
            planes,
            mvs: vec![[[0; 2]; 4]; aligned_width / 32 * (aligned_height / 32)],
            mb_width: aligned_width / 32,
        }
    }

    pub fn set(&mut self, plane: usize, bx: usize, by: usize, info: u16) {
        let g = &mut self.planes[plane];
        g.info[by * g.width + bx] = info;
    }
}

/// The per-frame switches.
pub struct Params<'a> {
    /// `(flags & 0xff) << 3`
    pub threshold: i16,
    pub vertical: bool,
    pub horizontal: bool,
    /// One flag per 8-sample line of luma, set where the line is not to be filtered; index 0 is the border.
    pub columns: &'a [bool],
    pub rows: &'a [bool],
    /// The picture height rounded up to 16: macroblock rows are only partly decoded beyond it.
    pub height16: usize,
}

/// Which of the four samples across an edge (outer, inner on the first side, then the second side) move.
fn lanes(strength: u16) -> [bool; 4] {
    let (p, q) = (strength & 3, strength >> 2);
    [p >= 2, p >= 1, q >= 1, q >= 2]
}

fn filter_line(s: [u8; 4], lanes: [bool; 4]) -> [u8; 4] {
    let (p1, p0, q0, q1) = (
        i32::from(s[0]),
        i32::from(s[1]),
        i32::from(s[2]),
        i32::from(s[3]),
    );
    let d = q0 - p0;
    let outer = (d + 4) >> 3;
    let inner = (d + 2) >> 2;
    let sat = |v: i32| v.clamp(0, 255) as u8;
    [
        if lanes[0] { sat(p1 + outer) } else { s[0] },
        if lanes[1] { sat(p0 + inner) } else { s[1] },
        if lanes[2] { sat(q0 - inner) } else { s[2] },
        if lanes[3] { sat(q1 - outer) } else { s[3] },
    ]
}

/// Filters eight lines across a vertical edge at column `x` of rows `y..y + 8`.
fn filter_vertical(plane: &mut Plane, x: usize, y: usize, strength: u16) {
    let on = lanes(strength);
    for row in y..y + 8 {
        let at = row * plane.stride + x - 2;
        let line: [u8; 4] = plane.data[at..at + 4].try_into().unwrap();
        plane.data[at..at + 4].copy_from_slice(&filter_line(line, on));
    }
}

/// Filters eight lines across a horizontal edge at row `y` of columns `x..x + 8`.
fn filter_horizontal(plane: &mut Plane, x: usize, y: usize, strength: u16) {
    let on = lanes(strength);
    for col in x..x + 8 {
        let at = |k: usize| (y - 2 + k) * plane.stride + col;
        let line = [0, 1, 2, 3].map(|k| plane.data[at(k)]);
        for (k, v) in filter_line(line, on).into_iter().enumerate() {
            plane.data[at(k)] = v;
        }
    }
}

fn mvs_differ(a: [i32; 2], b: [i32; 2]) -> bool {
    (a[0] - b[0]).abs() > 1 || (a[1] - b[1]).abs() > 1
}

/// The shape of one plane's blocks.
struct Shape {
    /// Blocks per macroblock side.
    per_mb: usize,
    /// Factor from this plane's edge lines to luma lines (the edge flags are per luma line).
    flag_scale: usize,
    /// Rows of 8x8 blocks that exist in the picture, rounded up to 16 luma rows.
    valid_rows: usize,
}

impl Shape {
    fn of(plane: usize, height16: usize) -> Shape {
        if plane == 1 || plane == 2 {
            Shape {
                per_mb: 2,
                flag_scale: 2,
                valid_rows: height16 / 16,
            }
        } else {
            Shape {
                per_mb: 4,
                flag_scale: 1,
                valid_rows: height16 / 8,
            }
        }
    }

    /// The macroblock index and the quarter (0..4) holding block (`bx`, `by`).
    fn quarter(&self, bx: usize, by: usize, mb_width: usize) -> (usize, usize) {
        let mb = (by / self.per_mb) * mb_width + bx / self.per_mb;
        let (qx, qy) = if self.per_mb == 4 {
            ((bx % 4) / 2, (by % 4) / 2)
        } else {
            (bx % 2, by % 2)
        };
        (mb, qy * 2 + qx)
    }
}

fn strength(meta: &Meta, shape: &Shape, grid: &Grid, p: (usize, usize), q: (usize, usize)) -> u16 {
    let (ip, iq) = (
        grid.info[p.1 * grid.width + p.0],
        grid.info[q.1 * grid.width + q.0],
    );
    if (ip | iq) & INTRA != 0 {
        return class(ip) | (class(iq) << 2);
    }
    let (mp, qp) = shape.quarter(p.0, p.1, meta.mb_width);
    let (mq, qq) = shape.quarter(q.0, q.1, meta.mb_width);
    if (mp, qp) != (mq, qq) && mvs_differ(meta.mvs[mp][qp], meta.mvs[mq][qq]) {
        10
    } else {
        0
    }
}

fn smooth_plane(plane: &mut Plane, grid: &Grid, shape: &Shape, threshold: i16) {
    let word = |bx: isize, by: usize| -> u16 {
        if bx < 0 || bx as usize >= grid.width {
            0
        } else {
            grid.info[by * grid.width + bx as usize]
        }
    };
    for by in 0..shape.valid_rows.min(grid.height) {
        let above = by.saturating_sub(1);
        let below = if by + 1 < grid.height { by + 1 } else { by };
        for bx in (0..grid.width).step_by(2) {
            let left = flat(word(bx as isize, by));
            let right = flat(word(bx as isize + 1, by));
            if !left && !right {
                continue;
            }
            let four =
                |row: usize| std::array::from_fn(|k| word(bx as isize - 1 + k as isize, row));
            let rows = smooth::render(four(above), four(by), four(below), threshold);
            for (y, line) in rows.iter().enumerate() {
                let at = (by * 8 + y) * plane.stride + bx * 8;
                if left {
                    plane.data[at..at + 8].copy_from_slice(&line[..8]);
                }
                if right {
                    plane.data[at + 8..at + 16].copy_from_slice(&line[8..]);
                }
            }
        }
    }
}

/// Smooths flat blocks, then filters vertical and horizontal edges of every plane of `frame`.
pub fn apply(frame: &mut Frame, meta: &Meta, params: &Params) {
    let off = |flags: &[bool], line: usize| flags.get(line).copied().unwrap_or(false);
    for (index, plane) in frame.planes.iter_mut().enumerate() {
        let shape = Shape::of(index, params.height16);
        let grid = &meta.planes[index];
        smooth_plane(plane, grid, &shape, params.threshold);
        if params.vertical {
            for bx in 1..grid.width {
                if off(params.columns, bx * shape.flag_scale) {
                    continue;
                }
                for by in 0..shape.valid_rows.min(grid.height) {
                    let s = strength(meta, &shape, grid, (bx - 1, by), (bx, by));
                    if s != 0 {
                        filter_vertical(plane, bx * 8, by * 8, s);
                    }
                }
            }
        }
        if params.horizontal {
            for by in 1..shape.valid_rows.min(grid.height) {
                if off(params.rows, by * shape.flag_scale) {
                    continue;
                }
                for bx in 0..grid.width {
                    let s = strength(meta, &shape, grid, (bx, by - 1), (bx, by));
                    if s != 0 {
                        filter_horizontal(plane, bx * 8, by * 8, s);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Plane {
        let data = (0..w * h).map(|i| f(i % w, i / w)).collect();
        Plane {
            data,
            stride: w,
            width: w as u32,
            height: h as u32,
        }
    }

    #[test]
    fn the_filter_pulls_both_sides_of_a_step_together() {
        let out = filter_line([100, 100, 140, 140], [true; 4]);
        // d = 40: outer (40 + 4) >> 3 = 5, inner (40 + 2) >> 2 = 10.
        assert_eq!(out, [105, 110, 130, 135]);
        let out = filter_line([140, 140, 100, 100], [true; 4]);
        // d = -40: outer (-36) >> 3 = -5 (rounding down), inner (-38) >> 2 = -10.
        assert_eq!(out, [135, 130, 110, 105]);
    }

    #[test]
    fn strength_nibbles_pick_the_samples_that_move() {
        // 0b0110: the first side moves twice (class 2), the second side not at all (q class 1 touches the inner one).
        assert_eq!(lanes(0), [false; 4]);
        assert_eq!(lanes(1), [false, true, false, false]);
        assert_eq!(lanes(3), [true, true, false, false]);
        assert_eq!(lanes(4), [false, false, true, false]);
        assert_eq!(lanes(12), [false, false, true, true]);
        assert_eq!(lanes(10), [true, true, true, true]);
        assert_eq!(lanes(6), [true, true, true, false]);
    }

    #[test]
    fn information_words_carry_class_intra_and_dc() {
        assert_eq!(info_word(true, 0, 5), 0xE005);
        assert_eq!(info_word(true, 3, 5), 0xC005);
        assert_eq!(info_word(false, 5, -1), 0x3FFF);
        assert_eq!(info_word(true, 12, 0x7ff), 0x87ff);
        assert!(flat(info_word(true, 0, 100)));
        assert!(!flat(info_word(false, 0, 100)));
    }

    struct Fixture {
        frame: Frame,
        meta: Meta,
    }

    fn fixture(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Fixture {
        let mk = |pw: usize, ph: usize| plane(pw, ph, &f);
        Fixture {
            frame: Frame {
                index: 0,
                width: w as u32,
                height: h as u32,
                planes: vec![mk(w, h), mk(w / 2, h / 2), mk(w / 2, h / 2)],
            },
            meta: Meta::new(w, h, false),
        }
    }

    fn params<'a>(cols: &'a [bool], rows: &'a [bool]) -> Params<'a> {
        Params {
            threshold: 0,
            vertical: true,
            horizontal: true,
            columns: cols,
            rows,
            height16: 32,
        }
    }

    #[test]
    fn an_intra_edge_is_filtered_by_its_classes_and_flags_can_switch_it_off() {
        let step = |x: usize, _| if x < 8 { 100 } else { 140 };
        // Block (0,0) has no coefficients and its neighbour carries a few: both move two samples.
        let intra = |class_count| info_word(true, class_count, 0);
        let mut fx = fixture(32, 32, step);
        fx.meta.set(0, 0, 0, intra(1));
        fx.meta.set(0, 1, 0, intra(2));
        let p = params(&[], &[]);
        let mut with = Frame {
            planes: fx.frame.planes.clone(),
            ..fx.frame.clone()
        };
        apply(&mut with, &fx.meta, &p);
        let row = &with.planes[0].data[..12];
        assert_eq!(&row[6..10], &[105, 110, 130, 135]);
        assert_eq!(row[5], 100);
        assert_eq!(row[10], 140);
        // The flag for the line at x = 8 turns it off.
        let cols = [false, true];
        let mut without = fx.frame.clone();
        apply(&mut without, &fx.meta, &params(&cols, &[]));
        assert_eq!(&without.planes[0].data[6..10], &[100, 100, 140, 140]);
        // So does the frame switch.
        let mut off = fx.frame.clone();
        let mut q = params(&[], &[]);
        q.vertical = false;
        apply(&mut off, &fx.meta, &q);
        assert_eq!(&off.planes[0].data[6..10], &[100, 100, 140, 140]);
    }

    #[test]
    fn class_one_moves_only_the_inner_samples() {
        let step = |_, y: usize| if y < 8 { 100 } else { 140 };
        let mut fx = fixture(32, 32, step);
        fx.meta.set(0, 0, 0, info_word(true, 5, 0));
        fx.meta.set(0, 0, 1, info_word(true, 5, 0));
        apply(&mut fx.frame, &fx.meta, &params(&[], &[]));
        let col: Vec<u8> = (6..10).map(|y| fx.frame.planes[0].data[y * 32]).collect();
        assert_eq!(col, [100, 110, 130, 140]);
    }

    #[test]
    fn inter_edges_need_different_vectors_and_only_sit_on_quarter_boundaries() {
        // Macroblock 0 moves, macroblock 1 does not: the edge at x = 32 is a quarter edge between them.
        let mut fx = fixture(64, 32, |x, _| if x < 32 { 100 } else { 140 });
        fx.meta.mvs[0] = [[4, 0]; 4];
        apply(&mut fx.frame, &fx.meta, &params(&[], &[]));
        assert_eq!(&fx.frame.planes[0].data[30..34], &[105, 110, 130, 135]);
        // A difference of one unit is no difference.
        let mut fx2 = fixture(64, 32, |x, _| if x < 32 { 100 } else { 140 });
        fx2.meta.mvs[0] = [[1, -1]; 4];
        apply(&mut fx2.frame, &fx2.meta, &params(&[], &[]));
        assert_eq!(&fx2.frame.planes[0].data[30..34], &[100, 100, 140, 140]);
        // The 8-sample edges inside a quarter are left alone even where the picture steps.
        let mut fx3 = fixture(64, 32, |x, _| if x < 8 { 100 } else { 140 });
        fx3.meta.mvs[0] = [[4, 0]; 4];
        apply(&mut fx3.frame, &fx3.meta, &params(&[], &[]));
        assert_eq!(&fx3.frame.planes[0].data[6..10], &[100, 100, 140, 140]);
    }

    #[test]
    fn rows_past_the_picture_are_left_alone() {
        let step = |_, y: usize| if y < 16 { 100 } else { 140 };
        let mut fx = fixture(32, 32, step);
        fx.meta.mvs[0] = [[0, 0], [0, 0], [8, 0], [8, 0]];
        let mut p = params(&[], &[]);
        p.height16 = 16;
        apply(&mut fx.frame, &fx.meta, &p);
        let col: Vec<u8> = (14..18).map(|y| fx.frame.planes[0].data[y * 32]).collect();
        assert_eq!(col, [100, 100, 140, 140]);
        p.height16 = 32;
        apply(&mut fx.frame, &fx.meta, &p);
        let col: Vec<u8> = (14..18).map(|y| fx.frame.planes[0].data[y * 32]).collect();
        assert_eq!(col, [105, 110, 130, 135]);
    }
}
