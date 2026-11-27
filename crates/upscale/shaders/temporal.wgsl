// Temporal upscaler: jittered samples are reconstructed at output resolution and accumulated into a reprojected
// history that is rectified against the current neighbourhood.
@group(0) @binding(0) var<storage, read> color: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> depth: array<f32>;
@group(0) @binding(2) var<storage, read> motion: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read> reactive: array<f32>;
@group(0) @binding(4) var<storage, read> hist: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> hist_out: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read> P: array<u32>;
// P: 0 render w, 1 render h, 2 out w, 3 out h, 4 jitter x, 5 jitter y, 6 exposure, 7 mv scale x, 8 mv scale y,
//    9 flags (1 reset, 2 depth, 4 motion, 8 reactive, 16 reversed depth), 10 max history frames (f32)

fn pf(i: u32) -> f32 { return bitcast<f32>(P[i]); }

fn compress(c: vec3<f32>) -> vec3<f32> {
    let v = max(c, vec3<f32>(0.0));
    return v / (1.0 + luma(v));
}

fn expand(c: vec3<f32>) -> vec3<f32> {
    let v = max(c, vec3<f32>(0.0));
    return v / (1.0 - min(luma(v), 0.9999));
}

fn load_color(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(P[0]) - 1);
    let cy = clamp(y, 0, i32(P[1]) - 1);
    return compress(color[u32(cy) * P[0] + u32(cx)].rgb / pf(6u));
}

fn load_hist(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(P[2]) - 1);
    let cy = clamp(y, 0, i32(P[3]) - 1);
    return hist[u32(cy) * P[2] + u32(cx)];
}

fn cr_weights(f: f32) -> vec4<f32> {
    let f2 = f * f;
    let f3 = f2 * f;
    return vec4<f32>(
        -0.5 * f3 + f2 - 0.5 * f,
        1.5 * f3 - 2.5 * f2 + 1.0,
        -1.5 * f3 + 2.0 * f2 + 0.5 * f,
        0.5 * f3 - 0.5 * f2,
    );
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[2] || id.y >= P[3] { return; }
    let flags = P[9];
    let rs = vec2<f32>(f32(P[0]), f32(P[1]));
    let os = vec2<f32>(f32(P[2]), f32(P[3]));
    let scale = rs / os;
    let jitter = vec2<f32>(pf(4u), pf(5u));
    let r = (vec2<f32>(f32(id.x), f32(id.y)) + 0.5) * scale;
    let centre = vec2<i32>(floor(r + jitter));

    var csum = vec3<f32>(0.0);
    var wsum = 0.0;
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var near_d = -1.0;
    var near_t = vec2<i32>(clamp(centre.x, 0, i32(P[0]) - 1), clamp(centre.y, 0, i32(P[1]) - 1));
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let t = vec2<i32>(clamp(centre.x + dx, 0, i32(P[0]) - 1), clamp(centre.y + dy, 0, i32(P[1]) - 1));
            let pos = vec2<f32>(f32(t.x), f32(t.y)) + 0.5 - jitter;
            let d = pos - r;
            let w = exp(-2.0 * dot(d, d));
            let c = load_color(t.x, t.y);
            csum += c * w;
            wsum += w;
            m1 += c;
            m2 += c * c;
            if (flags & 2u) != 0u {
                var n = depth[u32(t.y) * P[0] + u32(t.x)];
                if (flags & 16u) == 0u { n = 1.0 - n; }
                if n > near_d {
                    near_d = n;
                    near_t = t;
                }
            }
        }
    }
    let cur = csum / wsum;
    m1 = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - m1 * m1, vec3<f32>(0.0)));
    let box_lo = m1 - 1.5 * sigma;
    let box_hi = m1 + 1.5 * sigma;

    var mv = vec2<f32>(0.0);
    if (flags & 4u) != 0u {
        mv = motion[u32(near_t.y) * P[0] + u32(near_t.x)] * vec2<f32>(pf(7u), pf(8u));
    }
    let prev_o = (vec2<f32>(f32(id.x), f32(id.y)) + 0.5) + mv / scale;
    let inside = prev_o.x >= 0.0 && prev_o.y >= 0.0 && prev_o.x <= os.x && prev_o.y <= os.y;

    var h = cur;
    var count = 0.0;
    if (flags & 1u) == 0u && inside {
        let s = prev_o - 0.5;
        let base = floor(s);
        let f = s - base;
        let wx = cr_weights(f.x);
        let wy = cr_weights(f.y);
        let bx = i32(base.x);
        let by = i32(base.y);
        var acc = vec3<f32>(0.0);
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let hv = load_hist(bx - 1 + i, by - 1 + j);
                acc += hv.rgb * (wx[i] * wy[j]);
            }
        }
        let c00 = load_hist(bx, by).a;
        let c10 = load_hist(bx + 1, by).a;
        let c01 = load_hist(bx, by + 1).a;
        let c11 = load_hist(bx + 1, by + 1).a;
        count = min(min(c00, c10), min(c01, c11));
        h = max(acc, vec3<f32>(0.0));
    }
    let hc = clamp(h, box_lo, box_hi);
    var keep = 1.0 - sat(length(h - hc) * 4.0);
    if (flags & 8u) != 0u {
        let rx = u32(clamp(i32(r.x), 0, i32(P[0]) - 1));
        let ry = u32(clamp(i32(r.y), 0, i32(P[1]) - 1));
        keep = keep * (1.0 - sat(reactive[ry * P[0] + rx]));
    }
    let speed = length(mv);
    let eff = count * keep;
    var alpha = 1.0 / (eff + 1.0);
    alpha = max(alpha, 0.15 * sat(speed / 2.0));
    if eff <= 0.0 { alpha = 1.0; }
    let out_c = mix(hc, cur, alpha);
    let new_count = min(eff + 1.0, pf(10u));
    let i = id.y * P[2] + id.x;
    hist_out[i] = vec4<f32>(out_c, new_count);
    dst[i] = vec4<f32>(expand(out_c) * pf(6u), 1.0);
}
