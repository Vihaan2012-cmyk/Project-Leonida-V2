// P: 0 width, 1 height, 2 render width, 3 render height, 4 phase (f32), 5 flags (1 reversed depth),
//    6 depth tolerance (f32), 7 near / far (f32), 8 mv scale x, 9 mv scale y
fn pf(i: u32) -> f32 { return bitcast<f32>(P[i]); }

fn nearness(d: f32) -> f32 {
    let k = pf(7u);
    let n = select(1.0 - d * (1.0 - k), d * (1.0 - k) + k, (P[5] & 1u) != 0u);
    return clamp(n, 0.0, 4.0);
}

fn render_texel(p: vec2<u32>) -> u32 {
    let rx = min(u32(floor((f32(p.x) + 0.5) * f32(P[2]) / f32(P[0]))), P[2] - 1u);
    let ry = min(u32(floor((f32(p.y) + 0.5) * f32(P[3]) / f32(P[1]))), P[3] - 1u);
    return ry * P[2] + rx;
}

fn render_texel_at(pos: vec2<f32>) -> u32 {
    let rx = u32(clamp(floor(pos.x * f32(P[2]) / f32(P[0])), 0.0, f32(P[2] - 1u)));
    let ry = u32(clamp(floor(pos.y * f32(P[3]) / f32(P[1])), 0.0, f32(P[3] - 1u)));
    return ry * P[2] + rx;
}
