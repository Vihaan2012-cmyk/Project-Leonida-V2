// Writes a float buffer into a half-float storage image. P: 0 width, 1 height.
@group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> P: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[0] || id.y >= P[1] { return; }
    textureStore(dst, vec2<i32>(id.xy), src[id.y * P[0] + id.x]);
}
