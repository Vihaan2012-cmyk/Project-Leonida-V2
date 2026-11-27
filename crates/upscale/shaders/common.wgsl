fn rcp_lo(a: f32) -> f32 { return bitcast<f32>(0x7ef07ebbu - bitcast<u32>(a)); }
fn rsq_lo(a: f32) -> f32 { return bitcast<f32>(0x5f347d74u - (bitcast<u32>(a) >> 1u)); }
fn rcp_med(a: f32) -> f32 {
    let b = bitcast<f32>(0x7ef19fffu - bitcast<u32>(a));
    return b * (-b * a + 2.0);
}
fn sat(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }
fn to_perceptual(c: vec3<f32>) -> vec3<f32> { return sqrt(max(c, vec3<f32>(0.0))); }
fn from_perceptual(c: vec3<f32>) -> vec3<f32> { return c * c; }
fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
