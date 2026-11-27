//! The GPU tree builder: the passes of the CPU builder as WGSL compute kernels, compiled to SPIR-V by naga and
//! run through the `gpu` crate. Each pass is its own dispatch, so no kernel depends on another invocation's
//! writes within a dispatch. Everything that sets a byte (Morton keys, the sort order, the tree, the boxes and
//! half rounding) is integer work or exact float min, max, add-then-halve and power-of-two scaling, so the
//! bytes equal the CPU builder's.

use gpu::Gpu;

use crate::{BuildError, NodeBuilder, NodeOutput, NodeParams, Result};

const PRELUDE: &str = "
const INVALID: u32 = 0xFFFFFFFFu;
const LEAF: u32 = 0x80000000u;

fn fb(x: u32) -> f32 { return bitcast<f32>(x); }
fn ub(x: f32) -> u32 { return bitcast<u32>(x); }
fn ord(x: u32) -> u32 {
    if ((x & 0x80000000u) != 0u) { return ~x; }
    return x | 0x80000000u;
}
fn unord(o: u32) -> u32 {
    if ((o & 0x80000000u) != 0u) { return o & 0x7FFFFFFFu; }
    return ~o;
}
fn gindex(g: vec3<u32>) -> u32 { return g.x + g.y * 4194240u; }
";

const BOUNDS: &str = "
@group(0) @binding(0) var<storage, read_write> boxes: array<u32>;
@group(0) @binding(1) var<storage, read_write> bounds: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> params: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[0]) { return; }
    let b = i * 6u;
    if (fb(boxes[b]) > fb(boxes[b + 3u])) { return; }
    for (var a = 0u; a < 3u; a = a + 1u) {
        let c = (fb(boxes[b + a]) + fb(boxes[b + 3u + a])) * 0.5;
        atomicMin(&bounds[a], ord(ub(c)));
        atomicMax(&bounds[3u + a], ord(ub(c)));
    }
}
";

const KEYS: &str = "
@group(0) @binding(0) var<storage, read_write> boxes: array<u32>;
@group(0) @binding(1) var<storage, read_write> bounds: array<u32>;
@group(0) @binding(2) var<storage, read_write> keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> idx: array<u32>;
@group(0) @binding(4) var<storage, read_write> params: array<u32>;

fn scale_for(extent: f32) -> f32 {
    let bits = ub(extent);
    let exponent = i32((bits >> 23u) & 255u);
    if (exponent == 0 || exponent == 255) { return 1.0; }
    var ceiling = exponent - 127;
    if ((bits & 0x7FFFFFu) != 0u) { ceiling = ceiling + 1; }
    let shift = clamp(10 - ceiling, -126, 127);
    return bitcast<f32>(u32(shift + 127) << 23u);
}

fn spread(v: u32) -> u32 {
    var x = v;
    x = (x | (x << 16u)) & 0x030000FFu;
    x = (x | (x << 8u)) & 0x0300F00Fu;
    x = (x | (x << 4u)) & 0x030C30C3u;
    x = (x | (x << 2u)) & 0x09249249u;
    return x;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[2]) { return; }
    if (i >= params[0]) {
        keys[i] = INVALID;
        idx[i] = INVALID;
        return;
    }
    idx[i] = i;
    let b = i * 6u;
    if (fb(boxes[b]) > fb(boxes[b + 3u])) {
        keys[i] = 0x3FFFFFFFu;
        return;
    }
    var extent = 0.0;
    for (var a = 0u; a < 3u; a = a + 1u) {
        let low = fb(unord(bounds[a]));
        let high = fb(unord(bounds[3u + a]));
        extent = max(extent, high - low);
    }
    let scale = scale_for(extent);
    var key = 0u;
    for (var a = 0u; a < 3u; a = a + 1u) {
        let c = (fb(boxes[b + a]) + fb(boxes[b + 3u + a])) * 0.5;
        let cell = u32(clamp((c - fb(unord(bounds[a]))) * scale, 0.0, 1023.0));
        key = key | (spread(cell) << (2u - a));
    }
    keys[i] = key;
}
";

const SORT: &str = "
@group(0) @binding(0) var<storage, read_write> keys: array<u32>;
@group(0) @binding(1) var<storage, read_write> idx: array<u32>;
@group(0) @binding(2) var<storage, read_write> params: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[2]) { return; }
    let k = params[3];
    let j = params[4];
    let l = i ^ j;
    if (l <= i) { return; }
    let ascending = (i & k) == 0u;
    let ka = keys[i];
    let kb = keys[l];
    let ia = idx[i];
    let ib = idx[l];
    let greater = (ka > kb) || (ka == kb && ia > ib);
    if (greater == ascending) {
        keys[i] = kb;
        keys[l] = ka;
        idx[i] = ib;
        idx[l] = ia;
    }
}
";

const TREE: &str = "
@group(0) @binding(0) var<storage, read_write> keys: array<u32>;
@group(0) @binding(1) var<storage, read_write> kids: array<u32>;
@group(0) @binding(2) var<storage, read_write> parent: array<u32>;
@group(0) @binding(3) var<storage, read_write> params: array<u32>;

fn delta(i: i32, j: i32) -> i32 {
    if (j < 0 || j >= i32(params[0])) { return -1; }
    let a = keys[u32(i)];
    let b = keys[u32(j)];
    if (a == b) { return 32 + i32(countLeadingZeros(u32(i) ^ u32(j))); }
    return i32(countLeadingZeros(a ^ b));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let u = gindex(gid);
    if (u >= params[1]) { return; }
    let i = i32(u);
    var direction = -1;
    if (delta(i, i + 1) - delta(i, i - 1) > 0) { direction = 1; }
    let minimum = delta(i, i - direction);
    var limit = 2;
    while (delta(i, i + limit * direction) > minimum) { limit = limit * 2; }
    var span = 0;
    var stride = limit / 2;
    while (stride >= 1) {
        if (delta(i, i + (span + stride) * direction) > minimum) { span = span + stride; }
        stride = stride / 2;
    }
    let j = i + span * direction;
    let prefix = delta(i, j);
    var split = 0;
    var t = span;
    loop {
        t = (t + 1) >> 1u;
        if (delta(i, i + (split + t) * direction) > prefix) { split = split + t; }
        if (t <= 1) { break; }
    }
    let gamma = i + split * direction + min(direction, 0);
    var left = u32(gamma);
    if (min(i, j) == gamma) { left = left | LEAF; }
    var right = u32(gamma + 1);
    if (max(i, j) == gamma + 1) { right = right | LEAF; }
    kids[2u * u] = left;
    kids[2u * u + 1u] = right;
    if ((left & LEAF) == 0u) { parent[left] = u; }
    if ((right & LEAF) == 0u) { parent[right] = u; }
}
";

const DEPTH: &str = "
@group(0) @binding(0) var<storage, read_write> parent: array<u32>;
@group(0) @binding(1) var<storage, read_write> depth: array<u32>;
@group(0) @binding(2) var<storage, read_write> flag: array<u32>;
@group(0) @binding(3) var<storage, read_write> params: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[1]) { return; }
    var d = 0u;
    var at = i;
    while (parent[at] != INVALID) {
        at = parent[at];
        d = d + 1u;
    }
    depth[i] = d;
    flag[i] = select(0u, 1u, (d & 1u) == 0u);
}
";

const SCAN: &str = "
@group(0) @binding(0) var<storage, read_write> data: array<u32>;
@group(0) @binding(1) var<storage, read_write> sums: array<u32>;
@group(0) @binding(2) var<storage, read_write> params: array<u32>;

var<workgroup> tmp: array<u32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let i = wid.x * 256u + lid.x;
    var v = 0u;
    if (i < params[1]) { v = data[i]; }
    tmp[lid.x] = v;
    workgroupBarrier();
    for (var offset = 1u; offset < 256u; offset = offset << 1u) {
        var add = 0u;
        if (lid.x >= offset) { add = tmp[lid.x - offset]; }
        workgroupBarrier();
        tmp[lid.x] = tmp[lid.x] + add;
        workgroupBarrier();
    }
    if (i < params[1]) { data[i] = tmp[lid.x]; }
    if (lid.x == 255u) { sums[wid.x] = tmp[255u]; }
}
";

const ADD: &str = "
@group(0) @binding(0) var<storage, read_write> data: array<u32>;
@group(0) @binding(1) var<storage, read_write> offsets: array<u32>;
@group(0) @binding(2) var<storage, read_write> params: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[1]) { return; }
    data[i] = data[i] + offsets[i / 256u];
}
";

const LEVEL: &str = "
@group(0) @binding(0) var<storage, read_write> boxes: array<u32>;
@group(0) @binding(1) var<storage, read_write> idx: array<u32>;
@group(0) @binding(2) var<storage, read_write> kids: array<u32>;
@group(0) @binding(3) var<storage, read_write> depth: array<u32>;
@group(0) @binding(4) var<storage, read_write> params: array<u32>;

fn child_base(c: u32) -> u32 {
    if ((c & LEAF) != 0u) { return idx[c & ~LEAF] * 6u; }
    return (params[0] + c) * 6u;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[1]) { return; }
    if (depth[i] != params[5]) { return; }
    let l = child_base(kids[2u * i]);
    let r = child_base(kids[2u * i + 1u]);
    let at = (params[0] + i) * 6u;
    for (var a = 0u; a < 3u; a = a + 1u) {
        boxes[at + a] = ub(min(fb(boxes[l + a]), fb(boxes[r + a])));
        boxes[at + 3u + a] = ub(max(fb(boxes[l + 3u + a]), fb(boxes[r + 3u + a])));
    }
}
";

const NODES: &str = "
@group(0) @binding(0) var<storage, read_write> boxes: array<u32>;
@group(0) @binding(1) var<storage, read_write> idx: array<u32>;
@group(0) @binding(2) var<storage, read_write> kids: array<u32>;
@group(0) @binding(3) var<storage, read_write> depth: array<u32>;
@group(0) @binding(4) var<storage, read_write> incl: array<u32>;
@group(0) @binding(5) var<storage, read_write> nodes: array<u32>;
@group(0) @binding(6) var<storage, read_write> parents: array<u32>;
@group(0) @binding(7) var<storage, read_write> params: array<u32>;

fn half_round(bits: u32, up: bool) -> u32 {
    let sgn = (bits >> 16u) & 0x8000u;
    let mag = bits & 0x7FFFFFFFu;
    let away = up != (sgn != 0u);
    if (mag > 0x7F800000u) { return 0x7E00u; }
    if (mag == 0x7F800000u) { return sgn | 0x7C00u; }
    if (mag >= 0x47800000u) {
        if (away) { return sgn | 0x7C00u; }
        return sgn | 0x7BFFu;
    }
    if (mag < 0x38800000u) {
        if (mag != 0u && away) { return sgn | 0x0400u; }
        return 0u;
    }
    var h = (mag >> 13u) - (112u << 10u);
    if (away && (mag & 0x1FFFu) != 0u) { h = h + 1u; }
    return sgn | h;
}

fn write_box(rec: u32, slot: u32, src: u32) {
    if (params[10] == 1u) {
        let h0 = half_round(boxes[src], false);
        let h1 = half_round(boxes[src + 1u], false);
        let h2 = half_round(boxes[src + 2u], false);
        let h3 = half_round(boxes[src + 3u], true);
        let h4 = half_round(boxes[src + 4u], true);
        let h5 = half_round(boxes[src + 5u], true);
        nodes[rec + 4u + 3u * slot] = h0 | (h1 << 16u);
        nodes[rec + 5u + 3u * slot] = h2 | (h3 << 16u);
        nodes[rec + 6u + 3u * slot] = h4 | (h5 << 16u);
    } else {
        for (var k = 0u; k < 6u; k = k + 1u) {
            nodes[rec + 4u + 6u * slot + k] = boxes[src + k];
        }
    }
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gindex(gid);
    if (i >= params[1]) { return; }
    if ((depth[i] & 1u) != 0u) { return; }
    var items: array<u32, 4>;
    var count = 0u;
    for (var side = 0u; side < 2u; side = side + 1u) {
        let c = kids[2u * i + side];
        if ((c & LEAF) == 0u) {
            items[count] = kids[2u * c];
            items[count + 1u] = kids[2u * c + 1u];
            count = count + 2u;
        } else {
            items[count] = c;
            count = count + 1u;
        }
    }
    let own_unit = params[9] + (incl[i] - 1u) * params[10];
    let own_pointer = (own_unit << 3u) | params[11];
    let rec = (incl[i] - 1u) * params[10] * 16u;
    let empty = params[0] * 6u + params[1] * 6u;
    if (i == 0u) { parents[own_unit - params[6]] = INVALID; }
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        if (slot < count) {
            let c = items[slot];
            var unit = 0u;
            var pointer = 0u;
            var src = 0u;
            if ((c & LEAF) != 0u) {
                let e = idx[c & ~LEAF];
                unit = params[6] + e * params[7];
                pointer = (unit << 3u) | params[8];
                src = e * 6u;
            } else {
                unit = params[9] + (incl[c] - 1u) * params[10];
                pointer = (unit << 3u) | params[11];
                src = (params[0] + c) * 6u;
            }
            nodes[rec + slot] = pointer;
            write_box(rec, slot, src);
            parents[unit - params[6]] = own_pointer;
        } else {
            nodes[rec + slot] = INVALID;
            write_box(rec, slot, empty);
        }
    }
}
";

fn compile(kernel: &str) -> Result<Vec<u32>> {
    let source = format!("{PRELUDE}{kernel}");
    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| BuildError::Gpu(format!("WGSL: {}", e.emit_to_string(&source))))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| BuildError::Gpu(format!("WGSL validation: {e:?}")))?;
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: naga::ShaderStage::Compute,
        entry_point: "main".into(),
    };
    let options = naga::back::spv::Options {
        lang_version: (1, 3),
        ..Default::default()
    };
    naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline))
        .map_err(|e| BuildError::Gpu(format!("SPIR-V: {e:?}")))
}

struct Kernels {
    bounds: Vec<u32>,
    keys: Vec<u32>,
    sort: Vec<u32>,
    tree: Vec<u32>,
    depth: Vec<u32>,
    scan: Vec<u32>,
    add: Vec<u32>,
    level: Vec<u32>,
    nodes: Vec<u32>,
}

const PARAM_WORDS: usize = 16;

/// Builds trees on the GPU: Morton keys, a bitonic sort, the radix tree, the even-depth collapse with a
/// scan, boxes bottom-up one depth level per dispatch, then the node records and parent table.
pub struct GpuBuilder {
    gpu: Gpu,
    kernels: Kernels,
}

fn groups(count: usize) -> [u32; 3] {
    let wg = count.div_ceil(64).max(1) as u32;
    if wg <= 65535 {
        [wg, 1, 1]
    } else {
        [65535, wg.div_ceil(65535), 1]
    }
}

impl Kernels {
    fn compile() -> Result<Kernels> {
        Ok(Kernels {
            bounds: compile(BOUNDS)?,
            keys: compile(KEYS)?,
            sort: compile(SORT)?,
            tree: compile(TREE)?,
            depth: compile(DEPTH)?,
            scan: compile(SCAN)?,
            add: compile(ADD)?,
            level: compile(LEVEL)?,
            nodes: compile(NODES)?,
        })
    }
}

/// Compiles every kernel to SPIR-V without a device, as a check of the WGSL.
pub fn compile_kernels() -> Result<()> {
    Kernels::compile().map(|_| ())
}

impl GpuBuilder {
    pub fn new() -> Result<GpuBuilder> {
        GpuBuilder::with(Gpu::new().map_err(|e| BuildError::Gpu(e.to_string()))?)
    }

    pub fn with(gpu: Gpu) -> Result<GpuBuilder> {
        Ok(GpuBuilder {
            gpu,
            kernels: Kernels::compile()?,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.gpu.info.name
    }

    fn run(&self, spirv: &[u32], buffers: &mut [&mut [u32]], groups: [u32; 3]) -> Result<()> {
        self.gpu
            .dispatch(spirv, buffers, groups, None)
            .map_err(|e| BuildError::Gpu(e.to_string()))
    }
}

impl NodeBuilder for GpuBuilder {
    fn build(&mut self, boxes: &[[f32; 6]], p: &NodeParams) -> Result<NodeOutput> {
        let n = boxes.len();
        let m = n - 1;
        let padded = n.next_power_of_two();
        if m.div_ceil(256) > 65535 {
            return Err(BuildError::Unsupported(
                "a GPU build of over 16 million elements",
            ));
        }
        let mut params = [0u32; PARAM_WORDS];
        params[0] = n as u32;
        params[1] = m as u32;
        params[2] = padded as u32;
        params[6] = p.element_base;
        params[7] = p.element_stride;
        params[8] = p.element_type;
        params[9] = p.node_base;
        params[10] = p.node_units;
        params[11] = p.node_type;
        params[12] = p.node_table_base;

        let mut all_boxes = vec![0u32; (n + m + 1) * 6];
        for (i, b) in boxes.iter().enumerate() {
            for (k, v) in b.iter().enumerate() {
                all_boxes[6 * i + k] = v.to_bits();
            }
        }
        for (k, v) in crate::EMPTY_BOX.iter().enumerate() {
            all_boxes[(n + m) * 6 + k] = v.to_bits();
        }
        let mut bounds = [u32::MAX, u32::MAX, u32::MAX, 0, 0, 0];
        self.run(
            &self.kernels.bounds,
            &mut [&mut all_boxes, &mut bounds, &mut params],
            groups(n),
        )?;
        let mut keys = vec![0u32; padded];
        let mut idx = vec![0u32; padded];
        self.run(
            &self.kernels.keys,
            &mut [
                &mut all_boxes,
                &mut bounds,
                &mut keys,
                &mut idx,
                &mut params,
            ],
            groups(padded),
        )?;
        let mut k = 2;
        while k <= padded {
            let mut j = k / 2;
            while j >= 1 {
                params[3] = k as u32;
                params[4] = j as u32;
                self.run(
                    &self.kernels.sort,
                    &mut [&mut keys, &mut idx, &mut params],
                    groups(padded),
                )?;
                j /= 2;
            }
            k *= 2;
        }

        let mut kids = vec![0u32; 2 * m];
        let mut parent = vec![u32::MAX; m];
        self.run(
            &self.kernels.tree,
            &mut [&mut keys, &mut kids, &mut parent, &mut params],
            groups(m),
        )?;
        let mut depth = vec![0u32; m];
        let mut incl = vec![0u32; m];
        self.run(
            &self.kernels.depth,
            &mut [&mut parent, &mut depth, &mut incl, &mut params],
            groups(m),
        )?;

        let blocks = m.div_ceil(256);
        let mut sums = vec![0u32; blocks];
        self.run(
            &self.kernels.scan,
            &mut [&mut incl, &mut sums, &mut params],
            [blocks as u32, 1, 1],
        )?;
        let mut running = 0u32;
        for sum in sums.iter_mut() {
            let block = *sum;
            *sum = running;
            running += block;
        }
        self.run(
            &self.kernels.add,
            &mut [&mut incl, &mut sums, &mut params],
            groups(m),
        )?;
        let node_count = incl[m - 1];

        let deepest = depth.iter().copied().max().unwrap_or(0);
        for level in (0..=deepest).rev() {
            params[5] = level;
            self.run(
                &self.kernels.level,
                &mut [&mut all_boxes, &mut idx, &mut kids, &mut depth, &mut params],
                groups(m),
            )?;
        }

        let mut nodes = vec![0u32; (node_count * p.node_units * 16) as usize];
        let mut parents = vec![0u32; (p.node_table_base + node_count * p.node_units) as usize];
        self.run(
            &self.kernels.nodes,
            &mut [
                &mut all_boxes,
                &mut idx,
                &mut kids,
                &mut depth,
                &mut incl,
                &mut nodes,
                &mut parents,
                &mut params,
            ],
            groups(m),
        )?;
        Ok(NodeOutput {
            node_count,
            nodes,
            parents,
        })
    }
}
