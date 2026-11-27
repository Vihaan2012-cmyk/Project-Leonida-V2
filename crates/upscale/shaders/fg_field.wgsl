@group(0) @binding(0) var<storage, read> depth: array<f32>;
@group(0) @binding(1) var<storage, read> motion: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> key: array<u32>;
@group(0) @binding(3) var<storage, read_write> field: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read> P: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[0] || id.y >= P[1] { return; }
    let r = render_texel(id.xy);
    let to_display = vec2<f32>(f32(P[0]) / f32(P[2]), f32(P[1]) / f32(P[3]));
    let mv = motion[r] * vec2<f32>(pf(8u), pf(9u)) * to_display;
    let q = vec2<i32>(floor(vec2<f32>(f32(id.x), f32(id.y)) + 0.5 + (1.0 - pf(4u)) * mv));
    if q.x < 0 || q.y < 0 || q.x >= i32(P[0]) || q.y >= i32(P[1]) { return; }
    let i = u32(q.y) * P[0] + u32(q.x);
    if key[i] == bitcast<u32>(nearness(depth[r])) + 1u {
        field[i] = mv;
    }
}
