//! Colour and geometry for the compose pass: transfer functions, primaries, tone mapping and scaling rectangles.
//!
//! The functions here are the CPU model of the WGSL in `COMPOSE_WGSL`; the tests compare the two on the GPU.

/// `DXGI_COLOR_SPACE_TYPE` values a plane can carry that the layer composes.
pub const COLOR_SPACE_SDR: i32 = 0;
pub const COLOR_SPACE_HDR10: i32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// Rec.709 primaries, gamma-encoded.
    Sdr,
    /// Rec.2020 primaries, ST.2084 (PQ).
    Hdr10,
}

impl Space {
    pub fn from_dxgi(value: i32) -> (Space, bool) {
        match value {
            COLOR_SPACE_SDR => (Space::Sdr, true),
            COLOR_SPACE_HDR10 => (Space::Hdr10, true),
            _ => (Space::Sdr, false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SdrEotf {
    /// The piecewise curve of IEC 61966-2-1.
    #[default]
    Srgb,
    Gamma22,
}

pub const DEFAULT_SDR_WHITE_NITS: f32 = 203.0;
pub const DEFAULT_HDR_PEAK_NITS: f32 = 1000.0;
const KNEE: f32 = 0.8;

pub fn srgb_to_linear(e: f32) -> f32 {
    if e <= 0.04045 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

pub fn sdr_decode(e: f32, eotf: SdrEotf) -> f32 {
    match eotf {
        SdrEotf::Srgb => srgb_to_linear(e),
        SdrEotf::Gamma22 => e.max(0.0).powf(2.2),
    }
}

pub fn sdr_encode(l: f32, eotf: SdrEotf) -> f32 {
    match eotf {
        SdrEotf::Srgb => linear_to_srgb(l),
        SdrEotf::Gamma22 => l.max(0.0).powf(1.0 / 2.2),
    }
}

const M1: f32 = 2610.0 / 16384.0;
const M2: f32 = 2523.0 / 4096.0 * 128.0;
const C1: f32 = 3424.0 / 4096.0;
const C2: f32 = 2413.0 / 4096.0 * 32.0;
const C3: f32 = 2392.0 / 4096.0 * 32.0;

/// ST.2084 inverse EOTF: nits to the code value.
pub fn pq_encode(nits: f32) -> f32 {
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// ST.2084 EOTF: code value to nits.
pub fn pq_decode(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    let n = (p - C1).max(0.0) / (C2 - C3 * p);
    n.powf(1.0 / M1) * 10_000.0
}

pub type Mat3 = [[f32; 3]; 3];

/// Linear Rec.709 to linear Rec.2020 (ITU-R BT.2087).
pub const P709_TO_P2020: Mat3 = [
    [0.627_404, 0.329_283, 0.043_313],
    [0.069_097, 0.919_540, 0.011_362],
    [0.016_391, 0.088_013, 0.895_595],
];

/// Linear Rec.2020 to linear Rec.709.
pub const P2020_TO_P709: Mat3 = [
    [1.660_491, -0.587_641, -0.072_850],
    [-0.124_55, 1.132_9, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_73],
];

pub fn mul(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// The luminance shoulder for HDR content on an SDR output: identity up to the knee, an extended Reinhard above it that
/// maps `lw` (the content peak relative to SDR white) to exactly 1.
pub fn shoulder(y: f32, lw: f32) -> f32 {
    if y <= KNEE {
        return y;
    }
    let t = 1.0 - KNEE;
    let u = (y - KNEE) / t;
    let w = ((lw - KNEE) / t).max(1e-4);
    KNEE + t * u * (1.0 + u / (w * w)) / (1.0 + u)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub sdr_eotf: SdrEotf,
    pub sdr_white_nits: f32,
    pub hdr_peak_nits: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            sdr_eotf: SdrEotf::Srgb,
            sdr_white_nits: DEFAULT_SDR_WHITE_NITS,
            hdr_peak_nits: DEFAULT_HDR_PEAK_NITS,
        }
    }
}

/// Decodes one encoded pixel to linear light in nits, in the primaries of the output it is going to.
pub fn decode_to_nits(rgb: [f32; 3], from: Space, to: Space, p: &Params) -> [f32; 3] {
    match (from, to) {
        (Space::Sdr, Space::Sdr) => rgb.map(|e| sdr_decode(e, p.sdr_eotf) * p.sdr_white_nits),
        (Space::Sdr, Space::Hdr10) => mul(
            &P709_TO_P2020,
            rgb.map(|e| sdr_decode(e, p.sdr_eotf) * p.sdr_white_nits),
        ),
        (Space::Hdr10, Space::Hdr10) => rgb.map(pq_decode),
        (Space::Hdr10, Space::Sdr) => mul(&P2020_TO_P709, rgb.map(pq_decode)),
    }
}

/// Encodes linear nits (in the output's primaries) for an output.
pub fn encode_from_nits(nits: [f32; 3], space: Space, from_hdr: bool, p: &Params) -> [f32; 3] {
    match space {
        Space::Hdr10 => nits.map(|n| pq_encode(n.max(0.0))),
        Space::Sdr => {
            let mut rel = nits.map(|n| n / p.sdr_white_nits);
            if from_hdr {
                let y = 0.2126 * rel[0] + 0.7152 * rel[1] + 0.0722 * rel[2];
                if y > KNEE {
                    let lw = (p.hdr_peak_nits / p.sdr_white_nits).max(1.0);
                    let scale = shoulder(y, lw) / y;
                    rel = rel.map(|c| c * scale);
                }
            }
            rel.map(|c| sdr_encode(c.clamp(0.0, 1.0), p.sdr_eotf))
        }
    }
}

/// A whole conversion of one pixel; what the compose pass computes after filtering.
pub fn convert(rgb: [f32; 3], from: Space, to: Space, p: &Params) -> [f32; 3] {
    if from == to {
        return rgb;
    }
    encode_from_nits(
        decode_to_nits(rgb, from, to, p),
        to,
        from == Space::Hdr10,
        p,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// Fill the window, ignoring the aspect ratio.
    Stretch,
    /// Keep the aspect ratio, bars where it does not fit.
    #[default]
    Fit,
    /// Whole-number scale when one fits, else `Fit`.
    Integer,
}

/// A rectangle in pixels: left, top, right, bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }
}

/// Where a `src_w` x `src_h` image goes in a `dst_w` x `dst_h` window.
pub fn fit_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, mode: ScaleMode) -> Rect {
    let full = Rect {
        left: 0,
        top: 0,
        right: dst_w as i32,
        bottom: dst_h as i32,
    };
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 || mode == ScaleMode::Stretch {
        return full;
    }
    let (w, h) = (u64::from(src_w), u64::from(src_h));
    let (dw, dh) = (u64::from(dst_w), u64::from(dst_h));
    let whole = (dw / w).min(dh / h);
    let (out_w, out_h) = if mode == ScaleMode::Integer && whole >= 1 {
        (w * whole, h * whole)
    } else if dw * h <= dh * w {
        (dw, (h * dw + w / 2) / w)
    } else {
        ((w * dh + h / 2) / h, dh)
    };
    let left = ((dw - out_w.min(dw)) / 2) as i32;
    let top = ((dh - out_h.min(dh)) / 2) as i32;
    Rect {
        left,
        top,
        right: left + out_w.min(dw) as i32,
        bottom: top + out_h.min(dh) as i32,
    }
}

/// A plane's destination as fractions of the output (`pDestPlacementBase`), taken inside `canvas`.
pub fn place(canvas: Rect, fractions: [f32; 4]) -> Option<Rect> {
    let [l, t, r, b] = fractions;
    if fractions.iter().any(|f| !f.is_finite())
        || !(0.0..=1.0).contains(&l)
        || !(0.0..=1.0).contains(&t)
        || !(0.0..=1.0).contains(&r)
        || !(0.0..=1.0).contains(&b)
        || r <= l
        || b <= t
    {
        return None;
    }
    let (w, h) = (canvas.width() as f32, canvas.height() as f32);
    Some(Rect {
        left: canvas.left + (l * w).round() as i32,
        top: canvas.top + (t * h).round() as i32,
        right: canvas.left + (r * w).round() as i32,
        bottom: canvas.top + (b * h).round() as i32,
    })
}

/// The compose pass. Binding 0 is the source, 1 the output, 2 the parameters.
///
/// Each output pixel maps back into the source rectangle, the four nearest texels are decoded to linear light and
/// weighted there (or the nearest one is taken), and the result is converted to the output space and encoded once.
pub const COMPOSE_WGSL: &str = r#"
struct Params {
    src: vec4<f32>,
    dst: vec4<f32>,
    out_size: vec2<f32>,
    src_space: u32,
    out_space: u32,
    point_filter: u32,
    src_is_srgb_format: u32,
    gamma22: u32,
    pad: u32,
    sdr_white: f32,
    hdr_peak: f32,
    pad2: vec2<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> p: Params;

const M1: f32 = 0.1593017578125;
const M2: f32 = 78.84375;
const C1: f32 = 0.8359375;
const C2: f32 = 18.8515625;
const C3: f32 = 18.6875;
const KNEE: f32 = 0.8;

fn srgb_to_linear(e: f32) -> f32 {
    if (e <= 0.04045) { return e / 12.92; }
    return pow((e + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(l: f32) -> f32 {
    if (l <= 0.0031308) { return l * 12.92; }
    return 1.055 * pow(l, 1.0 / 2.4) - 0.055;
}

fn sdr_decode(e: f32) -> f32 {
    if (p.gamma22 != 0u) { return pow(max(e, 0.0), 2.2); }
    return srgb_to_linear(e);
}

fn sdr_encode(l: f32) -> f32 {
    if (p.gamma22 != 0u) { return pow(max(l, 0.0), 1.0 / 2.2); }
    return linear_to_srgb(l);
}

fn pq_encode(nits: f32) -> f32 {
    let y = pow(clamp(nits / 10000.0, 0.0, 1.0), M1);
    return pow((C1 + C2 * y) / (1.0 + C3 * y), M2);
}

fn pq_decode(e: f32) -> f32 {
    let q = pow(clamp(e, 0.0, 1.0), 1.0 / M2);
    let n = max(q - C1, 0.0) / (C2 - C3 * q);
    return pow(n, 1.0 / M1) * 10000.0;
}

fn p2020_to_p709(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        1.660491 * c.x - 0.587641 * c.y - 0.072850 * c.z,
        -0.124550 * c.x + 1.132900 * c.y - 0.008349 * c.z,
        -0.018151 * c.x - 0.100579 * c.y + 1.118730 * c.z);
}

fn p709_to_p2020(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.627404 * c.x + 0.329283 * c.y + 0.043313 * c.z,
        0.069097 * c.x + 0.919540 * c.y + 0.011362 * c.z,
        0.016391 * c.x + 0.088013 * c.y + 0.895595 * c.z);
}

fn decode_texel(t: vec3<f32>) -> vec3<f32> {
    var e = t;
    if (p.src_is_srgb_format != 0u) {
        e = vec3<f32>(linear_to_srgb(t.x), linear_to_srgb(t.y), linear_to_srgb(t.z));
    }
    if (p.src_space == 1u) {
        let n = vec3<f32>(pq_decode(e.x), pq_decode(e.y), pq_decode(e.z));
        if (p.out_space == 1u) { return n; }
        return p2020_to_p709(n);
    }
    let n = vec3<f32>(sdr_decode(e.x), sdr_decode(e.y), sdr_decode(e.z)) * p.sdr_white;
    if (p.out_space == 1u) { return p709_to_p2020(n); }
    return n;
}

fn shoulder(y: f32, lw: f32) -> f32 {
    if (y <= KNEE) { return y; }
    let t = 1.0 - KNEE;
    let u = (y - KNEE) / t;
    let w = max((lw - KNEE) / t, 0.0001);
    return KNEE + t * u * (1.0 + u / (w * w)) / (1.0 + u);
}

fn encode_out(nits: vec3<f32>) -> vec3<f32> {
    if (p.out_space == 1u) {
        return vec3<f32>(pq_encode(max(nits.x, 0.0)), pq_encode(max(nits.y, 0.0)), pq_encode(max(nits.z, 0.0)));
    }
    var rel = nits / p.sdr_white;
    if (p.src_space == 1u) {
        let y = 0.2126 * rel.x + 0.7152 * rel.y + 0.0722 * rel.z;
        if (y > KNEE) {
            let lw = max(p.hdr_peak / p.sdr_white, 1.0);
            rel = rel * (shoulder(y, lw) / y);
        }
    }
    rel = clamp(rel, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec3<f32>(sdr_encode(rel.x), sdr_encode(rel.y), sdr_encode(rel.z));
}

fn fetch(x: i32, y: i32, size: vec2<i32>) -> vec3<f32> {
    let c = vec2<i32>(clamp(x, i32(p.src.x), i32(p.src.z) - 1), clamp(y, i32(p.src.y), i32(p.src.w) - 1));
    let c2 = clamp(c, vec2<i32>(0), size - vec2<i32>(1));
    return decode_texel(textureLoad(src_tex, c2, 0).xyz);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (f32(id.x) >= p.out_size.x || f32(id.y) >= p.out_size.y) { return; }
    let px = vec2<f32>(f32(id.x) + 0.5, f32(id.y) + 0.5);
    var color = vec3<f32>(0.0);
    if (px.x >= p.dst.x && px.x < p.dst.z && px.y >= p.dst.y && px.y < p.dst.w) {
        let uv = (px - p.dst.xy) / (p.dst.zw - p.dst.xy);
        let pos = p.src.xy + uv * (p.src.zw - p.src.xy);
        let size = vec2<i32>(textureDimensions(src_tex));
        var nits = vec3<f32>(0.0);
        if (p.point_filter != 0u) {
            nits = fetch(i32(floor(pos.x)), i32(floor(pos.y)), size);
        } else {
            let q = pos - vec2<f32>(0.5);
            let f = fract(q);
            let b = vec2<i32>(floor(q));
            let a = fetch(b.x, b.y, size);
            let bb = fetch(b.x + 1, b.y, size);
            let c = fetch(b.x, b.y + 1, size);
            let d = fetch(b.x + 1, b.y + 1, size);
            nits = mix(mix(a, bb, f.x), mix(c, d, f.x), f.y);
        }
        color = encode_out(nits);
    }
    textureStore(out_tex, vec2<i32>(id.xy), vec4<f32>(color, 1.0));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} vs {b}");
    }

    #[test]
    fn srgb_curve_round_trips_and_matches_known_points() {
        for i in 0..=20 {
            let e = i as f32 / 20.0;
            close(linear_to_srgb(srgb_to_linear(e)), e, 1e-5);
        }
        close(srgb_to_linear(0.5), 0.214_04, 1e-4);
        close(sdr_decode(0.5, SdrEotf::Gamma22), 0.217_64, 1e-4);
    }

    #[test]
    fn pq_matches_reference_codes_and_round_trips() {
        close(pq_encode(0.0), 0.0, 1e-6);
        close(pq_encode(10_000.0), 1.0, 1e-5);
        close(pq_encode(203.0), 0.5807, 1e-3);
        for n in [0.5_f32, 10.0, 100.0, 1000.0, 4000.0] {
            close(pq_decode(pq_encode(n)) / n, 1.0, 2e-3);
        }
    }

    #[test]
    fn primaries_matrices_are_inverse_and_keep_white() {
        for row in P709_TO_P2020 {
            close(row.iter().sum::<f32>(), 1.0, 1e-4);
        }
        for row in P2020_TO_P709 {
            close(row.iter().sum::<f32>(), 1.0, 1e-4);
        }
        let v = [0.3, 0.6, 0.1];
        let back = mul(&P2020_TO_P709, mul(&P709_TO_P2020, v));
        for i in 0..3 {
            close(back[i], v[i], 1e-3);
        }
    }

    #[test]
    fn shoulder_is_identity_below_the_knee_and_hits_one_at_the_peak() {
        close(shoulder(0.5, 5.0), 0.5, 0.0);
        close(shoulder(KNEE, 5.0), KNEE, 1e-6);
        close(shoulder(5.0, 5.0), 1.0, 1e-5);
        let mut last = 0.0;
        for i in 0..100 {
            let y = shoulder(i as f32 * 0.05, 5.0);
            assert!(y >= last);
            last = y;
        }
        close(shoulder(KNEE + 1e-3, 5.0) - KNEE, 1e-3, 1e-4);
    }

    #[test]
    fn sdr_through_hdr10_and_back_is_the_identity() {
        let p = Params::default();
        for e in [0.0_f32, 0.1, 0.5, 0.9, 1.0] {
            let rgb = [e, 0.5 * e, 1.0 - e];
            let hdr = convert(rgb, Space::Sdr, Space::Hdr10, &p);
            let back = decode_to_nits(hdr, Space::Hdr10, Space::Sdr, &p)
                .map(|n| sdr_encode((n / p.sdr_white_nits).clamp(0.0, 1.0), p.sdr_eotf));
            for i in 0..3 {
                close(back[i], rgb[i], 0.01);
            }
        }
    }

    #[test]
    fn hdr10_white_at_sdr_white_lands_below_one_on_an_sdr_output() {
        let p = Params::default();
        let code = pq_encode(203.0);
        let out = convert([code; 3], Space::Hdr10, Space::Sdr, &p);
        assert!(out[0] > 0.9 && out[0] < 1.0, "{out:?}");
        let peak = convert([pq_encode(1000.0); 3], Space::Hdr10, Space::Sdr, &p);
        close(peak[0], 1.0, 0.01);
        let dark = convert([pq_encode(20.0); 3], Space::Hdr10, Space::Sdr, &p);
        close(dark[0], sdr_encode(20.0 / 203.0, p.sdr_eotf), 0.01);
    }

    #[test]
    fn same_space_is_untouched() {
        let p = Params::default();
        assert_eq!(
            convert([0.1, 0.2, 0.3], Space::Sdr, Space::Sdr, &p),
            [0.1, 0.2, 0.3]
        );
    }

    #[test]
    fn colour_space_values_map_with_a_flag_for_unknown_ones() {
        assert_eq!(Space::from_dxgi(0), (Space::Sdr, true));
        assert_eq!(Space::from_dxgi(12), (Space::Hdr10, true));
        assert_eq!(Space::from_dxgi(1), (Space::Sdr, false));
    }

    #[test]
    fn fit_keeps_the_aspect_ratio_and_centres() {
        let r = fit_rect(1920, 1080, 1000, 1000, ScaleMode::Fit);
        assert_eq!((r.width(), r.height()), (1000, 563));
        assert_eq!(r.left, 0);
        assert_eq!(r.top, (1000 - 563) / 2);
        let r = fit_rect(1920, 1080, 3840, 1080, ScaleMode::Fit);
        assert_eq!((r.left, r.right, r.top, r.bottom), (960, 2880, 0, 1080));
        let r = fit_rect(1920, 1080, 1920, 1080, ScaleMode::Fit);
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
    }

    #[test]
    fn stretch_fills_and_integer_uses_whole_multiples() {
        let r = fit_rect(640, 360, 1000, 700, ScaleMode::Stretch);
        assert_eq!((r.width(), r.height()), (1000, 700));
        let r = fit_rect(640, 360, 2000, 1000, ScaleMode::Integer);
        assert_eq!((r.width(), r.height()), (1280, 720));
        let r = fit_rect(640, 360, 1000, 700, ScaleMode::Integer);
        assert_eq!((r.width(), r.height()), (640, 360));
        let r = fit_rect(640, 360, 500, 300, ScaleMode::Integer);
        assert_eq!(r.height(), 281);
        assert!(r.right <= 500);
        assert!(fit_rect(0, 0, 10, 10, ScaleMode::Fit).width() == 10);
    }

    #[test]
    fn placement_fractions_are_taken_inside_the_canvas() {
        let canvas = Rect {
            left: 100,
            top: 0,
            right: 1100,
            bottom: 500,
        };
        let r = place(canvas, [0.0, 0.0, 0.5, 1.0]).unwrap();
        assert_eq!((r.left, r.top, r.right, r.bottom), (100, 0, 600, 500));
        assert!(place(canvas, [0.5, 0.0, 0.5, 1.0]).is_none());
        assert!(place(canvas, [0.0, 0.0, 1.5, 1.0]).is_none());
        assert!(place(canvas, [f32::NAN, 0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn the_shader_compiles_and_validates() {
        let module = naga::front::wgsl::parse_str(COMPOSE_WGSL).expect("wgsl");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("valid");
    }
}
