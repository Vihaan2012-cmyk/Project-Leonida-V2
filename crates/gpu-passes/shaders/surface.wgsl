struct Params {
    dir: u32,
    tiled_base: u32,
    linear_base: u32,
    w: u32,
    h: u32,
    d: u32,
    ox: u32,
    oy: u32,
    tiles_x: u32,
    tiles_y: u32,
    shape: u32,
    count: u32,
    pat: vec4<u32>,
}

var<immediate> p: Params;

@group(0) @binding(0) var<storage, read_write> tiled: array<u32>;
@group(0) @binding(1) var<storage, read_write> linear: array<u32>;

struct Coord {
    x: u32,
    y: u32,
    z: u32,
    valid: bool,
}

fn select_of(bit: u32) -> u32 {
    return (p.pat[bit / 5u] >> (6u * (bit % 5u))) & 63u;
}

fn forward(x: u32, y: u32, z: u32) -> u32 {
    let k = p.shape & 15u;
    let tb = (p.shape >> 4u) & 31u;
    let twl = (p.shape >> 9u) & 15u;
    let thl = (p.shape >> 13u) & 15u;
    let tdl = (p.shape >> 17u) & 15u;
    let xx = x + p.ox;
    let yy = y + p.oy;
    let tile = ((z >> tdl) * p.tiles_y + (yy >> thl)) * p.tiles_x + (xx >> twl);
    var off = tile << tb;
    for (var b = k; b < tb; b++) {
        let s = select_of(b);
        let idx = s & 15u;
        let kind = s >> 4u;
        var c = z;
        if kind == 1u {
            c = xx;
        } else if kind == 2u {
            c = yy;
        }
        off |= ((c >> idx) & 1u) << b;
    }
    return off;
}

fn inverse(q: u32) -> Coord {
    let k = p.shape & 15u;
    let tb = (p.shape >> 4u) & 31u;
    let twl = (p.shape >> 9u) & 15u;
    let thl = (p.shape >> 13u) & 15u;
    let tdl = (p.shape >> 17u) & 15u;
    var xin = 0u;
    var yin = 0u;
    var zin = 0u;
    for (var b = k; b < tb; b++) {
        let s = select_of(b);
        let bit = (q >> b) & 1u;
        let idx = s & 15u;
        let kind = s >> 4u;
        if kind == 1u {
            xin |= bit << idx;
        } else if kind == 2u {
            yin |= bit << idx;
        } else {
            zin |= bit << idx;
        }
    }
    let tile = q >> tb;
    let per_slab = p.tiles_x * p.tiles_y;
    let tx = tile % p.tiles_x;
    let ty = (tile / p.tiles_x) % p.tiles_y;
    let tz = tile / per_slab;
    let xx = (tx << twl) | xin;
    let yy = (ty << thl) | yin;
    let z = (tz << tdl) | zin;
    var c: Coord;
    c.valid = xx >= p.ox && yy >= p.oy && xx - p.ox < p.w && yy - p.oy < p.h && z < p.d;
    c.x = xx - p.ox;
    c.y = yy - p.oy;
    c.z = z;
    return c;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let idx = id.x + id.y * groups.x * 64u;
    if idx >= p.count {
        return;
    }
    let k = p.shape & 15u;
    let eb = 1u << k;
    let first = idx * 4u;
    if p.dir == 0u {
        let total = p.w * p.h * p.d * eb;
        var word = 0u;
        if eb >= 4u {
            let el = first >> k;
            let off = forward(el % p.w, (el / p.w) % p.h, el / (p.w * p.h)) + (first & (eb - 1u));
            word = tiled[p.tiled_base + (off >> 2u)];
        } else {
            for (var j = 0u; j < 4u; j++) {
                let lb = first + j;
                if lb < total {
                    let el = lb >> k;
                    let off = forward(el % p.w, (el / p.w) % p.h, el / (p.w * p.h)) + (lb & (eb - 1u));
                    word |= ((tiled[p.tiled_base + (off >> 2u)] >> ((off & 3u) * 8u)) & 255u) << (8u * j);
                }
            }
        }
        linear[p.linear_base + idx] = word;
    } else {
        if eb >= 4u {
            let c = inverse(first);
            if c.valid {
                let lb = ((c.z * p.h + c.y) * p.w + c.x) * eb + (first & (eb - 1u));
                tiled[p.tiled_base + idx] = linear[p.linear_base + (lb >> 2u)];
            }
        } else {
            var word = 0u;
            var any_valid = false;
            for (var j = 0u; j < 4u; j++) {
                let q = first + j;
                let c = inverse(q);
                if c.valid {
                    let lb = ((c.z * p.h + c.y) * p.w + c.x) * eb + (q & (eb - 1u));
                    word |= ((linear[p.linear_base + (lb >> 2u)] >> ((lb & 3u) * 8u)) & 255u) << (8u * j);
                    any_valid = true;
                }
            }
            if any_valid {
                tiled[p.tiled_base + idx] = word;
            }
        }
    }
}
