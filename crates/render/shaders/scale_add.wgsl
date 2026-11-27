struct Params {
    scale: u32,
    add: u32,
}

@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
var<immediate> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&output)) {
        output[id.x] = input[id.x] * params.scale + params.add;
    }
}
