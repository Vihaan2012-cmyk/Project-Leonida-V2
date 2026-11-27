@group(0) @binding(0) var<storage, read> cur_color: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> prev_color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> cur_depth: array<f32>;
@group(0) @binding(3) var<storage, read> prev_depth: array<f32>;
@group(0) @binding(4) var<storage, read> key: array<u32>;
@group(0) @binding(5) var<storage, read> field: array<vec2<f32>>;
@group(0) @binding(6) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read> P: array<u32>;

const HOLE_RADIUS = 6;

fn fetch(use_cur: bool, i: u32) -> vec3<f32> {
    if use_cur { return cur_color[i].rgb; }
    return prev_color[i].rgb;
}

fn bilinear(use_cur: bool, pos: vec2<f32>) -> vec3<f32> {
    let s = pos - 0.5;
    let b = floor(s);
    let f = s - b;
    let w = i32(P[0]);
    let h = i32(P[1]);
    let x0 = clamp(i32(b.x), 0, w - 1);
    let x1 = clamp(i32(b.x) + 1, 0, w - 1);
    let y0 = clamp(i32(b.y), 0, h - 1);
    let y1 = clamp(i32(b.y) + 1, 0, h - 1);
    let a = fetch(use_cur, u32(y0 * w + x0));
    let c = fetch(use_cur, u32(y0 * w + x1));
    let d = fetch(use_cur, u32(y1 * w + x0));
    let e = fetch(use_cur, u32(y1 * w + x1));
    return mix(mix(a, c, f.x), mix(d, e, f.x), f.y);
}

fn inside(pos: vec2<f32>) -> bool {
    return pos.x >= 0.0 && pos.y >= 0.0 && pos.x <= f32(P[0]) && pos.y <= f32(P[1]);
}

fn agrees(a: f32, b: f32) -> bool {
    return abs(a - b) <= pf(6u) * max(a, b) + 1.0e-5;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[0] || id.y >= P[1] { return; }
    let t = pf(4u);
    let w = i32(P[0]);
    let h = i32(P[1]);
    let q = vec2<i32>(i32(id.x), i32(id.y));
    let centre = vec2<f32>(f32(id.x), f32(id.y)) + 0.5;
    var mv = vec2<f32>(0.0);
    var nd = -1.0;
    let k = key[u32(q.y * w + q.x)];
    if k != 0u {
        mv = field[u32(q.y * w + q.x)];
        nd = bitcast<f32>(k - 1u);
    } else {
        var far_n = 1.0e9;
        for (var dy = -HOLE_RADIUS; dy <= HOLE_RADIUS; dy++) {
            for (var dx = -HOLE_RADIUS; dx <= HOLE_RADIUS; dx++) {
                let n = q + vec2<i32>(dx, dy);
                if n.x < 0 || n.y < 0 || n.x >= w || n.y >= h { continue; }
                let kn = key[u32(n.y * w + n.x)];
                if kn != 0u {
                    let nn = bitcast<f32>(kn - 1u);
                    if nn < far_n {
                        far_n = nn;
                        mv = field[u32(n.y * w + n.x)];
                        nd = nn;
                    }
                }
            }
        }
        if nd < 0.0 {
            nd = nearness(cur_depth[render_texel(id.xy)]);
        }
    }
    let cur_pos = centre - (1.0 - t) * mv;
    let prev_pos = centre + t * mv;
    let c_cur = bilinear(true, cur_pos);
    let c_prev = bilinear(false, prev_pos);
    let ok_cur = inside(cur_pos) && agrees(nearness(cur_depth[render_texel_at(cur_pos)]), nd);
    let ok_prev = inside(prev_pos) && agrees(nearness(prev_depth[render_texel_at(prev_pos)]), nd);
    var out = mix(c_prev, c_cur, t);
    if ok_cur && !ok_prev {
        out = c_cur;
    } else if ok_prev && !ok_cur {
        out = c_prev;
    }
    dst[u32(q.y * w + q.x)] = vec4<f32>(out, 1.0);
}
