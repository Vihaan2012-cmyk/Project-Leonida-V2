//! The smoothing of blocks that carry only a DC value, in the 16-bit vector arithmetic the game uses.
//!
//! The game's Bink DLL renders such an intra block as a gentle surface instead of a flat square: the block's DC
//! value and those of its neighbours in the rows above and below are mixed (a neighbour counts only while it lies
//! within the frame's threshold of the block), and the 8x8 samples are interpolated from the mixed corners. Three
//! routines do this for a pair of horizontally adjacent blocks, one for the left block (RVA 0xbbc0), one for the
//! right (0xbf20) and one for both (0xc280). They share one first stage and differ in how the results are laid out,
//! so the left and right layouts are written out in the tests and checked against the combined one used here.
//!
//! Every helper mirrors one SSE instruction on eight 16-bit lanes.

type V = [i16; 8];

const K1: V = [0x1c00; 8];
const K2: V = [0x3000; 8];
const K3: V = [0x4800; 8];
const K4: V = [0x6800; 8];
const FOUR: V = [4; 8];

fn map(a: V, b: V, f: impl Fn(i16, i16) -> i16) -> V {
    std::array::from_fn(|i| f(a[i], b[i]))
}
fn paddw(a: V, b: V) -> V {
    map(a, b, i16::wrapping_add)
}
fn psubw(a: V, b: V) -> V {
    map(a, b, i16::wrapping_sub)
}
fn pand(a: V, b: V) -> V {
    map(a, b, |x, y| x & y)
}
fn pandn(a: V, b: V) -> V {
    map(a, b, |x, y| !x & y)
}
fn por(a: V, b: V) -> V {
    map(a, b, |x, y| x | y)
}
fn pmaxsw(a: V, b: V) -> V {
    map(a, b, i16::max)
}
fn pcmpgtw(a: V, b: V) -> V {
    map(a, b, |x, y| if x > y { -1 } else { 0 })
}
fn pmulhw(a: V, b: V) -> V {
    map(a, b, |x, y| ((i32::from(x) * i32::from(y)) >> 16) as i16)
}
fn psrlw(a: V, n: u32) -> V {
    a.map(|x| ((x as u16) >> n) as i16)
}
fn psllw(a: V, n: u32) -> V {
    a.map(|x| ((x as u16) << n) as i16)
}
fn psraw(a: V, n: u32) -> V {
    a.map(|x| x >> n)
}
fn punpcklwd(a: V, b: V) -> V {
    [a[0], b[0], a[1], b[1], a[2], b[2], a[3], b[3]]
}
fn punpckhwd(a: V, b: V) -> V {
    [a[4], b[4], a[5], b[5], a[6], b[6], a[7], b[7]]
}
fn punpcklqdq(a: V, b: V) -> V {
    [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]
}
fn punpckldq(a: V, b: V) -> V {
    [a[0], a[1], b[0], b[1], a[2], a[3], b[2], b[3]]
}
/// Shift right by one 16-bit lane (`vpsrldq` by two bytes).
fn psrldq2(a: V) -> V {
    [a[1], a[2], a[3], a[4], a[5], a[6], a[7], 0]
}
fn pshufd(a: V, imm: u8) -> V {
    let mut out = [0i16; 8];
    for d in 0..4 {
        let src = usize::from((imm >> (2 * d)) & 3);
        out[2 * d] = a[2 * src];
        out[2 * d + 1] = a[2 * src + 1];
    }
    out
}
/// `shufps`: the two low dwords come from `a`, the two high ones from `b`.
fn shufps(a: V, b: V, imm: u8) -> V {
    let pick = |v: V, i: u8| {
        let i = usize::from(i & 3);
        (v[2 * i], v[2 * i + 1])
    };
    let parts = [
        pick(a, imm),
        pick(a, imm >> 2),
        pick(b, imm >> 4),
        pick(b, imm >> 6),
    ];
    let mut out = [0i16; 8];
    for (d, (lo, hi)) in parts.into_iter().enumerate() {
        out[2 * d] = lo;
        out[2 * d + 1] = hi;
    }
    out
}
fn packuswb(a: V, b: V) -> [u8; 16] {
    let sat = |x: i16| x.clamp(0, 255) as u8;
    std::array::from_fn(|i| if i < 8 { sat(a[i]) } else { sat(b[i - 8]) })
}

/// The eight line vectors the 8x8 samples are interpolated from.
fn first_stage(above: V, centre: V, below: V, threshold: V) -> [V; 8] {
    let c = pshufd(psrldq2(centre), 0);
    let lower_upper = punpcklqdq(below, above);
    let row = punpcklqdq(centre, punpckldq(psrldq2(above), psrldq2(below)));
    let near_row = psrlw(pcmpgtw(threshold, pmaxsw(psubw(c, row), psubw(row, c))), 3);
    let near_other = psrlw(
        pcmpgtw(
            threshold,
            pmaxsw(psubw(c, lower_upper), psubw(lower_upper, c)),
        ),
        3,
    );
    let dc = psrlw(psllw(c, 3), 3);
    let mixed = por(pandn(near_row, dc), pand(near_row, row));
    let other = por(
        pandn(near_other, pshufd(mixed, 0x44)),
        pand(near_other, lower_upper),
    );
    let base = shufps(mixed, dc, 0xe);
    let biased = paddw(base, FOUR);
    let d1 = psubw(shufps(other, mixed, 2), base);
    let d2 = psubw(shufps(other, mixed, 0x57), base);
    [
        paddw(pmulhw(K4, d1), biased),
        paddw(paddw(pmulhw(K1, d2), biased), psraw(d1, 4)),
        paddw(paddw(pmulhw(K2, d1), biased), psraw(d2, 5)),
        paddw(paddw(pmulhw(K3, d2), biased), psraw(d1, 7)),
        paddw(paddw(pmulhw(K3, d1), biased), psraw(d2, 7)),
        paddw(paddw(pmulhw(K2, d2), biased), psraw(d1, 5)),
        paddw(paddw(pmulhw(K1, d1), biased), psraw(d2, 4)),
        paddw(pmulhw(K4, d2), biased),
    ]
}

/// One output row: the multiplier rows combined with the base and an optional shifted correction.
fn row(mul: V, x: V, base: V, correction: Option<(V, u32)>) -> V {
    let mut sum = paddw(pmulhw(mul, x), base);
    if let Some((y, shift)) = correction {
        sum = paddw(sum, psraw(y, shift));
    }
    psraw(sum, 3)
}

/// The eight rows of the pair: eight bytes of the left block then eight of the right (RVA 0xc280's layout).
fn rows_both(lines: [V; 8]) -> [[u8; 16]; 8] {
    let [l0, l1, l2, l3, l4, l5, l6, l7] = lines;
    let lo01 = punpcklwd(l0, l1);
    let lo23 = punpcklwd(l2, l3);
    let lo45 = punpcklwd(l4, l5);
    let lo67 = punpcklwd(l6, l7);
    let a = punpcklwd(lo01, lo23);
    let b = punpcklwd(lo45, lo67);
    let t = punpcklwd(punpckhwd(l0, l1), punpckhwd(l2, l3));
    let u = punpcklwd(punpckhwd(l4, l5), punpckhwd(l6, l7));
    let m = punpcklwd(t, u);
    let n = punpckhwd(t, u);
    let e = psubw(punpcklwd(a, b), m);
    let f = psubw(punpckhwd(a, b), n);
    let g = punpckhwd(lo45, lo67);
    let h = punpckhwd(lo01, lo23);
    let p = psubw(punpcklwd(h, g), m);
    let q = psubw(punpckhwd(h, g), n);
    let out = |mul: V, left: V, right: V, cl: Option<(V, u32)>, cr: Option<(V, u32)>| {
        packuswb(row(mul, left, m, cl), row(mul, right, n, cr))
    };
    [
        out(K4, e, f, None, None),
        out(K3, e, f, Some((p, 7)), Some((q, 7))),
        out(K2, e, f, Some((p, 5)), Some((q, 5))),
        out(K1, e, f, Some((p, 4)), Some((q, 4))),
        out(K1, p, q, Some((e, 4)), Some((f, 4))),
        out(K2, p, q, Some((e, 5)), Some((f, 5))),
        out(K3, p, q, Some((e, 7)), Some((f, 7))),
        out(K4, p, q, None, None),
    ]
}

fn widen(w: [u16; 4]) -> V {
    [
        w[0] as i16,
        w[1] as i16,
        w[2] as i16,
        w[3] as i16,
        0,
        0,
        0,
        0,
    ]
}

/// Renders the pair of blocks from the information words of the rows above, at and below it, each four words
/// starting one block left of the pair. Eight bytes of the left block come first in each row, then the right.
pub fn render(above: [u16; 4], centre: [u16; 4], below: [u16; 4], threshold: i16) -> [[u8; 16]; 8] {
    rows_both(first_stage(
        widen(above),
        widen(centre),
        widen(below),
        [threshold; 8],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The left-block routine (RVA 0xbbc0) lays the same stage out through low halves.
    fn rows_left(lines: [V; 8]) -> [[u8; 8]; 8] {
        let [l0, l1, l2, l3, l4, l5, l6, l7] = lines;
        let v29 = punpcklwd(l0, l1);
        let v30 = punpcklwd(l2, l3);
        let v31 = punpcklwd(l4, l5);
        let v38 = punpcklwd(l6, l7);
        let v36 = punpckhwd(l2, l3);
        let v40 = punpckhwd(l0, l1);
        let base = punpcklwd(
            punpcklwd(v40, v36),
            punpcklwd(punpckhwd(l4, l5), punpckhwd(l6, l7)),
        );
        let x = psubw(punpcklwd(punpcklwd(v29, v30), punpcklwd(v31, v38)), base);
        let v44 = punpckhwd(v31, v38);
        let y = psubw(punpcklwd(punpckhwd(v29, v30), v44), base);
        let one = |mul: V, a: V, c: Option<(V, u32)>| -> [u8; 8] {
            let full = packuswb(row(mul, a, base, c), [0; 8]);
            std::array::from_fn(|i| full[i])
        };
        [
            one(K4, x, None),
            one(K3, x, Some((y, 7))),
            one(K2, x, Some((y, 5))),
            one(K1, x, Some((y, 4))),
            one(K1, y, Some((x, 4))),
            one(K2, y, Some((x, 5))),
            one(K3, y, Some((x, 7))),
            one(K4, y, None),
        ]
    }

    /// The right-block routine (RVA 0xbf20) takes the high halves.
    fn rows_right(lines: [V; 8]) -> [[u8; 8]; 8] {
        let [l0, l1, l2, l3, l4, l5, l6, l7] = lines;
        let v31 = punpcklwd(l0, l1);
        let v32 = punpcklwd(l2, l3);
        let v33 = punpcklwd(l4, l5);
        let v38 = punpcklwd(l6, l7);
        let v36 = punpckhwd(l2, l3);
        let v39 = punpckhwd(l0, l1);
        let base = punpckhwd(
            punpcklwd(v39, v36),
            punpcklwd(punpckhwd(l4, l5), punpckhwd(l6, l7)),
        );
        let x = psubw(punpckhwd(punpcklwd(v31, v32), punpcklwd(v33, v38)), base);
        let v43 = punpckhwd(v33, v38);
        let y = psubw(punpckhwd(punpckhwd(v31, v32), v43), base);
        let one = |mul: V, a: V, c: Option<(V, u32)>| -> [u8; 8] {
            let full = packuswb(row(mul, a, base, c), [0; 8]);
            std::array::from_fn(|i| full[i])
        };
        [
            one(K4, x, None),
            one(K3, x, Some((y, 7))),
            one(K2, x, Some((y, 5))),
            one(K1, x, Some((y, 4))),
            one(K1, y, Some((x, 4))),
            one(K2, y, Some((x, 5))),
            one(K3, y, Some((x, 7))),
            one(K4, y, None),
        ]
    }

    fn noise(seed: &mut u32) -> u16 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        0xE000 | ((*seed >> 16) as u16 % 0x300)
    }

    #[test]
    fn the_three_layouts_describe_the_same_surface() {
        let mut seed = 7;
        for _ in 0..200 {
            let mut rows = [[0u16; 4]; 3];
            for r in rows.iter_mut() {
                for w in r.iter_mut() {
                    *w = noise(&mut seed);
                }
            }
            let lines = first_stage(widen(rows[0]), widen(rows[1]), widen(rows[2]), [0x100; 8]);
            let both = rows_both(lines);
            let (left, right) = (rows_left(lines), rows_right(lines));
            for y in 0..8 {
                assert_eq!(&both[y][..8], &left[y], "left row {y}");
                assert_eq!(&both[y][8..], &right[y], "right row {y}");
            }
        }
    }

    #[test]
    fn equal_neighbours_give_a_flat_block() {
        let w = 0xE000u16 | 400;
        let out = render([w; 4], [w; 4], [w; 4], 0x100);
        let expect = ((400 + 4) >> 3) as u8;
        for row in out {
            assert!(row.iter().all(|&v| v == expect), "{row:?}");
        }
    }

    #[test]
    fn a_distant_neighbour_is_ignored_and_a_close_one_pulls() {
        let flat = |dc: u16| 0xE000u16 | dc;
        let centre = [flat(400); 4];
        let mut left_far = centre;
        left_far[0] = flat(2000);
        let far = render([centre[0]; 4], left_far, [centre[0]; 4], 0x100);
        let alone = render([centre[0]; 4], centre, [centre[0]; 4], 0x100);
        assert_eq!(far, alone);
        let mut left_near = centre;
        left_near[0] = flat(440);
        let near = render([centre[0]; 4], left_near, [centre[0]; 4], 0x100);
        assert!(near[0][0] > alone[0][0], "{:?} {:?}", near[0], alone[0]);
    }
}
