struct Params {
    blocks: u32,
    lane_stride: u32,
    max_chain: u32,
    lanes_base: u32,
    out_base: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

var<immediate> p: Params;

@group(0) @binding(0) var<storage, read_write> lanes: array<u32>;
@group(0) @binding(1) var<storage, read_write> tables: array<u32>;
@group(0) @binding(2) var<storage, read_write> out: array<u32>;
@group(0) @binding(3) var<storage, read_write> status: array<atomic<u32>>;

fn lane(l: u32, i: u32) -> u32 {
    let at = l * p.lane_stride + i;
    return (lanes[p.lanes_base + (at >> 2u)] >> ((at & 3u) * 8u)) & 255u;
}

fn unzig(v: u32) -> i32 {
    if (v & 1u) != 0u {
        return -i32((v + 1u) >> 1u);
    }
    return i32(v >> 1u);
}

fn wrap(v: i32, m: i32) -> u32 {
    return u32(((v % m) + m) % m);
}

fn quadrant(sym: u32) -> u32 {
    return (tables[sym >> 2u] >> ((sym & 3u) * 8u)) & 255u;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let i = id.x + id.y * groups.x * 64u;
    if i >= p.blocks {
        return;
    }
    var sum = array<i32, 6>(0, 0, 0, 0, 0, 0);
    var root = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
    var j = i;
    var steps = 0u;
    loop {
        let v = lane(0u, j);
        var d = v >> 1u;
        if (v & 1u) != 0u {
            d = 256u - ((v + 1u) >> 1u);
        }
        for (var c = 0u; c < 6u; c++) {
            sum[c] += unzig(lane(2u + c, j));
        }
        if d == 0u {
            break;
        }
        if j >= d {
            j = j - d;
        } else {
            let entry = 256u + j - d;
            let lo = tables[64u + entry * 2u];
            let hi = tables[64u + entry * 2u + 1u];
            if ((hi >> 16u) & 1u) == 0u {
                atomicAdd(&status[0], 1u);
            } else {
                root = array<u32, 6>(lo & 255u, (lo >> 8u) & 255u, (lo >> 16u) & 255u, lo >> 24u, hi & 255u, (hi >> 8u) & 255u);
            }
            break;
        }
        steps++;
        if steps > p.max_chain {
            atomicAdd(&status[1], 1u);
            break;
        }
    }
    let e0 = wrap(i32(root[0]) + sum[0], 32);
    let e1 = wrap(i32(root[1]) + sum[1], 64);
    let e2 = wrap(i32(root[2]) + sum[2], 32);
    let e3 = wrap(i32(root[3]) + sum[3], 32);
    let e4 = wrap(i32(root[4]) + sum[4], 64);
    let e5 = wrap(i32(root[5]) + sum[5], 32);
    let c0 = (e0 << 11u) | (e1 << 5u) | e2;
    let c1 = (e3 << 11u) | (e4 << 5u) | e5;
    let a = quadrant(lane(8u, i));
    let b = quadrant(lane(9u, i));
    let c = quadrant(lane(10u, i));
    let d = quadrant(lane(11u, i));
    let b2 = (a & 15u) | ((b & 15u) << 4u);
    let b3 = (a >> 4u) | ((b >> 4u) << 4u);
    let b4 = (c & 15u) | ((d & 15u) << 4u);
    let b5 = (c >> 4u) | ((d >> 4u) << 4u);
    out[p.out_base + i * 2u] = c0 | (c1 << 16u);
    out[p.out_base + i * 2u + 1u] = b2 | (b3 << 8u) | (b4 << 16u) | (b5 << 24u);
}
