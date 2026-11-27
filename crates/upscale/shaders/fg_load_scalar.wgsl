// Copies the first channel of a colour-format image (depth stored as a colour) into a float buffer.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<storage, read> P: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= P[0] || id.y >= P[1] { return; }
    dst[id.y * P[0] + id.x] = textureLoad(src, vec2<i32>(id.xy), 0).x;
}
