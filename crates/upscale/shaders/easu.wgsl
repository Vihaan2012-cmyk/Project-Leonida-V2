// AMD FidelityFX Super Resolution 1.0 EASU, ported from ffx_fsr1.h (MIT).
@group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> P: array<u32>;

fn ld(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(P[0]) - 1);
    let cy = clamp(y, 0, i32(P[1]) - 1);
    let c = src[u32(cy) * P[0] + u32(cx)].rgb;
    if P[4] != 0u { return to_perceptual(c); }
    return c;
}

fn lm(c: vec3<f32>) -> f32 { return c.b * 0.5 + (c.r * 0.5 + c.g); }

fn tap(off: vec2<f32>, dir: vec2<f32>, len: vec2<f32>, lob: f32, clp: f32, c: vec3<f32>) -> vec4<f32> {
    var v = vec2<f32>(off.x * dir.x + off.y * dir.y, off.x * (-dir.y) + off.y * dir.x);
    v = v * len;
    var d2 = v.x * v.x + v.y * v.y;
    d2 = min(d2, clp);
    var wb = 0.4 * d2 - 1.0;
    var wa = lob * d2 - 1.0;
    wb = wb * wb;
    wa = wa * wa;
    wb = (25.0 / 16.0) * wb - (25.0 / 16.0 - 1.0);
    let w = wb * wa;
    return vec4<f32>(c * w, w);
}

fn set_dir(w: f32, la: f32, lb: f32, lc: f32, ld_: f32, le: f32) -> vec3<f32> {
    var len_x = max(abs(ld_ - lc), abs(lc - lb));
    let dir_x = ld_ - lb;
    len_x = sat(abs(dir_x) * rcp_lo(len_x));
    len_x = len_x * len_x;
    var len_y = max(abs(le - lc), abs(lc - la));
    let dir_y = le - la;
    len_y = sat(abs(dir_y) * rcp_lo(len_y));
    len_y = len_y * len_y;
    return vec3<f32>(dir_x * w, dir_y * w, (len_x + len_y) * w);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[2] || id.y >= P[3] { return; }
    let scale = vec2<f32>(f32(P[0]) / f32(P[2]), f32(P[1]) / f32(P[3]));
    var pp = vec2<f32>(f32(id.x), f32(id.y)) * scale + (0.5 * scale - vec2<f32>(0.5));
    let fp = floor(pp);
    pp = pp - fp;
    let fx = i32(fp.x);
    let fy = i32(fp.y);
    let cb = ld(fx, fy - 1);
    let cc = ld(fx + 1, fy - 1);
    let ce = ld(fx - 1, fy);
    let cf = ld(fx, fy);
    let cg = ld(fx + 1, fy);
    let ch = ld(fx + 2, fy);
    let ci = ld(fx - 1, fy + 1);
    let cj = ld(fx, fy + 1);
    let ck = ld(fx + 1, fy + 1);
    let cl = ld(fx + 2, fy + 1);
    let cn = ld(fx, fy + 2);
    let co = ld(fx + 1, fy + 2);
    let bl = lm(cb);
    let cl_ = lm(cc);
    let el = lm(ce);
    let fl = lm(cf);
    let gl = lm(cg);
    let hl = lm(ch);
    let il = lm(ci);
    let jl = lm(cj);
    let kl = lm(ck);
    let ll = lm(cl);
    let nl = lm(cn);
    let ol = lm(co);

    var acc = vec3<f32>(0.0);
    acc += set_dir((1.0 - pp.x) * (1.0 - pp.y), bl, el, fl, gl, jl);
    acc += set_dir(pp.x * (1.0 - pp.y), cl_, fl, gl, hl, kl);
    acc += set_dir((1.0 - pp.x) * pp.y, fl, il, jl, kl, nl);
    acc += set_dir(pp.x * pp.y, gl, jl, kl, ll, ol);
    var dir = acc.xy;
    var len = acc.z;
    let dir2 = dir * dir;
    var dir_r = dir2.x + dir2.y;
    let zro = dir_r < (1.0 / 32768.0);
    dir_r = rsq_lo(dir_r);
    if zro {
        dir_r = 1.0;
        dir.x = 1.0;
    }
    dir = dir * dir_r;
    len = len * 0.5;
    len = len * len;
    let stretch = (dir.x * dir.x + dir.y * dir.y) * rcp_lo(max(abs(dir.x), abs(dir.y)));
    let len2 = vec2<f32>(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp = rcp_lo(lob);

    let min4 = min(min(min(cj, ck), cf), cg);
    let max4 = max(max(max(cj, ck), cf), cg);

    var a = vec4<f32>(0.0);
    a += tap(vec2<f32>(0.0, -1.0) - pp, dir, len2, lob, clp, cb);
    a += tap(vec2<f32>(1.0, -1.0) - pp, dir, len2, lob, clp, cc);
    a += tap(vec2<f32>(-1.0, 1.0) - pp, dir, len2, lob, clp, ci);
    a += tap(vec2<f32>(0.0, 1.0) - pp, dir, len2, lob, clp, cj);
    a += tap(vec2<f32>(0.0, 0.0) - pp, dir, len2, lob, clp, cf);
    a += tap(vec2<f32>(-1.0, 0.0) - pp, dir, len2, lob, clp, ce);
    a += tap(vec2<f32>(1.0, 1.0) - pp, dir, len2, lob, clp, ck);
    a += tap(vec2<f32>(2.0, 1.0) - pp, dir, len2, lob, clp, cl);
    a += tap(vec2<f32>(2.0, 0.0) - pp, dir, len2, lob, clp, ch);
    a += tap(vec2<f32>(1.0, 0.0) - pp, dir, len2, lob, clp, cg);
    a += tap(vec2<f32>(1.0, 2.0) - pp, dir, len2, lob, clp, co);
    a += tap(vec2<f32>(0.0, 2.0) - pp, dir, len2, lob, clp, cn);
    var pix = min(max4, max(min4, a.rgb * (1.0 / a.w)));
    if P[5] != 0u { pix = from_perceptual(pix); }
    dst[id.y * P[2] + id.x] = vec4<f32>(pix, 1.0);
}
