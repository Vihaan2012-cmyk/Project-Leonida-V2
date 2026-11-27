// AMD FidelityFX Super Resolution 1.0 RCAS, ported from ffx_fsr1.h (MIT). The optional denoise is left out.
@group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> P: array<u32>;
// P: 0 width, 1 height, 2 in_linear, 3 out_linear, 4 lobe scale (f32 bits)

fn ld(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(P[0]) - 1);
    let cy = clamp(y, 0, i32(P[1]) - 1);
    let c = src[u32(cy) * P[0] + u32(cx)].rgb;
    if P[2] != 0u { return to_perceptual(c); }
    return c;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[0] || id.y >= P[1] { return; }
    let x = i32(id.x);
    let y = i32(id.y);
    let b = ld(x, y - 1);
    let d = ld(x - 1, y);
    let e = ld(x, y);
    let f = ld(x + 1, y);
    let h = ld(x, y + 1);
    let mn4 = min(min(min(b, d), f), h);
    let mx4 = max(max(max(b, d), f), h);
    let hit_min = min(mn4, e) * (1.0 / max(4.0 * mx4, vec3<f32>(1.0e-6)));
    let hit_max = (vec3<f32>(1.0) - max(mx4, e)) * (1.0 / min(4.0 * mn4 - vec3<f32>(4.0), vec3<f32>(-1.0e-6)));
    let lobe_v = max(-hit_min, hit_max);
    let limit = 0.25 - 1.0 / 16.0;
    let lobe = max(-limit, min(max(max(lobe_v.r, lobe_v.g), lobe_v.b), 0.0)) * bitcast<f32>(P[4]);
    let rcp_l = rcp_med(4.0 * lobe + 1.0);
    var pix = (lobe * b + lobe * d + lobe * h + lobe * f + e) * rcp_l;
    if P[3] != 0u { pix = from_perceptual(pix); }
    dst[id.y * P[0] + id.x] = vec4<f32>(pix, src[id.y * P[0] + id.x].a);
}
