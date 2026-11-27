use std::collections::HashMap;

use serde_json::{Value, json};

use super::ModelError;
use super::sidecar::{parameter_name, sidecar_json};
use super::types::*;

const FLOAT: u32 = 5126;
const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const DIFFUSE_PARAMETER: u32 = 0xC8E8_C282;

/// Which LODs go into the file. Every LOD is listed in the sidecar either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LodSelection {
    /// The first (most detailed) LOD of each drawable.
    #[default]
    Primary,
    /// Every LOD; lower ones are named `..._lodN`.
    All,
}

pub struct ExportOptions<'a> {
    pub lods: LodSelection,
    /// Skeleton to bind skinned parts to when the drawable carries none (character parts take the
    /// shared humanoid skeleton from a fragment).
    pub skeleton: Option<&'a Skeleton>,
    /// Maps a 64-bit texture id to the URI of an exported image. Only `diffusetex` bindings are used
    /// as base colour; a material without one stays untextured.
    pub texture_uri: Option<&'a dyn Fn(u64) -> Option<String>>,
}

impl Default for ExportOptions<'_> {
    fn default() -> Self {
        Self {
            lods: LodSelection::Primary,
            skeleton: None,
            texture_uri: None,
        }
    }
}

/// One `.glb` and the JSON sidecar that goes beside it.
#[derive(Debug, Clone)]
pub struct ExportedModel {
    /// File stem to use for both files.
    pub stem: String,
    pub glb: Vec<u8>,
    pub sidecar: String,
}

/// Export a decoded resource. A drawable dictionary gives one model per entry; a drawable or a
/// fragment gives one model holding all of its drawables.
pub fn export(
    set: &ModelSet,
    options: &ExportOptions<'_>,
) -> Result<Vec<ExportedModel>, ModelError> {
    let base = set.name.clone().unwrap_or_else(|| "model".to_string());
    let groups: Vec<(String, Vec<&Drawable>)> = match set.kind {
        ResourceKind::Dictionary => set
            .drawables
            .iter()
            .enumerate()
            .map(|(i, d)| (stem_of(d, &base, i), vec![d]))
            .collect(),
        _ => vec![(base.clone(), set.drawables.iter().collect())],
    };
    groups
        .into_iter()
        .map(|(stem, drawables)| {
            let glb = build_glb(&drawables, options)?;
            let sidecar = sidecar_json(set, &drawables, options.skeleton);
            Ok(ExportedModel {
                stem,
                glb,
                sidecar: serde_json::to_string_pretty(&sidecar).expect("sidecar is valid JSON"),
            })
        })
        .collect()
}

fn stem_of(d: &Drawable, base: &str, index: usize) -> String {
    let raw = match (&d.name, d.name_hash) {
        (Some(n), _) => n.clone(),
        (None, Some(h)) => format!("{base}_{h:08x}"),
        (None, None) => format!("{base}_{index}"),
    };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

type Matrix = [f32; 16];

const IDENTITY: Matrix = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
/// Inverse of the Z-up to Y-up change `(x, y, z) -> (x, z, -y)`, as a column-major matrix.
const Y_UP_TO_Z_UP: Matrix = [
    1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1.,
];
const Z_UP_TO_Y_UP_ROTATION: [f32; 4] = [
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f32::consts::FRAC_1_SQRT_2,
];

fn y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn mat_mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn normalized_quat(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 1e-6 && len.is_finite() {
        [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        target: Option<u32>,
        component: u32,
        count: usize,
        kind: &str,
    ) -> usize {
        let view = self.view(bytes, target);
        self.accessors.push(
            json!({"bufferView": view, "componentType": component, "count": count, "type": kind}),
        );
        self.accessors.len() - 1
    }

    fn set_bounds(&mut self, accessor: usize, min: &[f32], max: &[f32]) {
        self.accessors[accessor]["min"] = json!(min);
        self.accessors[accessor]["max"] = json!(max);
    }

    fn set_normalized(&mut self, accessor: usize) {
        self.accessors[accessor]["normalized"] = json!(true);
    }

    fn floats(&mut self, data: &[f32], count: usize, kind: &str, target: Option<u32>) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.accessor(&bytes, target, FLOAT, count, kind)
    }
}

struct Joints {
    /// Node index of each skin joint.
    nodes: Vec<usize>,
    inverse_bind: Vec<Matrix>,
    by_hash: HashMap<u32, usize>,
    /// Helper joints for palette-less models whose indices run past the skeleton.
    direct: HashMap<usize, usize>,
    skeleton_len: usize,
    /// Nodes of joints the skeleton lacks; they hang off the scene root.
    helpers: Vec<usize>,
}

fn build_glb(drawables: &[&Drawable], options: &ExportOptions<'_>) -> Result<Vec<u8>, ModelError> {
    let skeleton = options
        .skeleton
        .or_else(|| drawables.iter().find_map(|d| d.skeleton.as_ref()));
    let mut b = Builder::default();
    let mut nodes: Vec<Value> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    let mut joints = Joints {
        nodes: Vec::new(),
        inverse_bind: Vec::new(),
        by_hash: HashMap::new(),
        direct: HashMap::new(),
        skeleton_len: skeleton.map_or(0, |s| s.bones.len()),
        helpers: Vec::new(),
    };

    if let Some(skel) = skeleton {
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); skel.bones.len()];
        for (i, bone) in skel.bones.iter().enumerate() {
            match bone.parent {
                Some(p) => children[p as usize].push(i),
                None => roots.push(i),
            }
        }
        for (i, bone) in skel.bones.iter().enumerate() {
            let (rotation, translation) = if bone.parent.is_none() {
                (
                    quat_mul(Z_UP_TO_Y_UP_ROTATION, normalized_quat(bone.rotation)),
                    y_up(bone.translation),
                )
            } else {
                (normalized_quat(bone.rotation), bone.translation)
            };
            let mut node = json!({
                "name": bone.name.clone().unwrap_or_else(|| format!("bone_0x{:08x}", bone.hash)),
                "rotation": rotation,
                "translation": translation,
                "scale": bone.scale,
            });
            if !children[i].is_empty() {
                node["children"] = json!(children[i]);
            }
            nodes.push(node);
            joints.nodes.push(i);
            joints
                .inverse_bind
                .push(mat_mul(&bone.inverse_bind, &Y_UP_TO_Z_UP));
            joints.by_hash.entry(bone.hash).or_insert(i);
        }
    }

    let mut materials: Vec<Value> = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut image_of: HashMap<String, usize> = HashMap::new();
    let mut mesh_defs: Vec<Value> = Vec::new();
    let mut mesh_nodes: Vec<usize> = Vec::new();
    let mut skinned_nodes: Vec<usize> = Vec::new();

    for (di, drawable) in drawables.iter().enumerate() {
        let label = drawable
            .name
            .clone()
            .or_else(|| drawable.name_hash.map(|h| format!("{h:08x}")))
            .unwrap_or_else(|| format!("drawable{di}"));
        let material_base = materials.len();
        for (mi, m) in drawable.materials.iter().enumerate() {
            let mut material = material_json(m, mi, &label);
            if let Some(resolve) = options.texture_uri
                && let Some(uri) = m
                    .textures
                    .iter()
                    .find(|t| t.param_hash == Some(DIFFUSE_PARAMETER))
                    .and_then(|t| resolve(t.id64))
            {
                let image = *image_of.entry(uri.clone()).or_insert_with(|| {
                    images.push(json!({"uri": uri}));
                    textures.push(json!({"source": images.len() - 1}));
                    textures.len() - 1
                });
                material["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": image});
            }
            materials.push(material);
        }
        let take = match options.lods {
            LodSelection::Primary => 1,
            LodSelection::All => drawable.lods.len(),
        };
        for lod in drawable.lods.iter().take(take) {
            for (mi, model) in lod.models.iter().enumerate() {
                let skinned = model.geometries.iter().any(|g| g.skin.is_some());
                let palette_map = skinned.then(|| palette_joints(model, &mut joints, &mut nodes));
                let mut primitives = Vec::new();
                for g in &model.geometries {
                    let material = (g.material < drawable.materials.len())
                        .then_some(material_base + g.material);
                    let map = g.skin.as_ref().and(palette_map.as_deref());
                    primitives.push(primitive_json(&mut b, g, material, map, skinned));
                }
                if primitives.is_empty() {
                    continue;
                }
                let name = if lod.level == drawable.lods[0].level {
                    format!("{label}_model{mi}")
                } else {
                    format!("{label}_lod{}_model{mi}", lod.level)
                };
                mesh_defs.push(json!({"name": name, "primitives": primitives}));
                let mut node = json!({"name": name, "mesh": mesh_defs.len() - 1});
                node["extras"] = json!({"lod": lod.level, "lod_distance": lod.distance});
                nodes.push(node);
                let index = nodes.len() - 1;
                mesh_nodes.push(index);
                if skinned {
                    skinned_nodes.push(index);
                }
            }
        }
    }

    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "leonida assets"},
        "scene": 0,
    });
    if !skinned_nodes.is_empty() && !joints.nodes.is_empty() {
        let ibm: Vec<f32> = joints.inverse_bind.iter().flatten().copied().collect();
        let accessor = b.floats(&ibm, joints.inverse_bind.len(), "MAT4", None);
        doc["skins"] = json!([{"joints": joints.nodes, "inverseBindMatrices": accessor}]);
        for &n in &skinned_nodes {
            nodes[n]["skin"] = json!(0);
        }
    }
    let mut scene_nodes: Vec<usize> = roots.clone();
    scene_nodes.extend(joints.helpers.iter().copied());
    scene_nodes.extend(mesh_nodes);
    doc["scenes"] = json!([{"name": "scene", "nodes": scene_nodes}]);
    doc["nodes"] = json!(nodes);
    doc["meshes"] = json!(mesh_defs);
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
    }
    doc["accessors"] = json!(b.accessors);
    doc["bufferViews"] = json!(b.views);
    doc["buffers"] = json!([{"byteLength": b.bin.len()}]);
    if b.bin.len() > u32::MAX as usize - 1024 {
        return Err(ModelError::Invalid(
            "model is larger than a glb can hold".into(),
        ));
    }
    Ok(pack_glb(&doc, &b.bin))
}

/// Skin joint index of each palette entry, adding a helper joint for a hash the skeleton lacks.
fn palette_joints(model: &Model, joints: &mut Joints, nodes: &mut Vec<Value>) -> Vec<u16> {
    if model.bone_palette.is_empty() {
        return (0..model.bone_count as usize)
            .map(|i| {
                if i < joints.skeleton_len {
                    return i as u16;
                }
                let joint = *joints.direct.entry(i).or_insert_with(|| {
                    nodes.push(json!({"name": format!("bone_{i}")}));
                    joints.nodes.push(nodes.len() - 1);
                    joints.helpers.push(nodes.len() - 1);
                    joints.inverse_bind.push(IDENTITY);
                    joints.nodes.len() - 1
                });
                joint as u16
            })
            .collect();
    }
    model
        .bone_palette
        .iter()
        .map(|hash| {
            let joint = *joints.by_hash.entry(*hash).or_insert_with(|| {
                nodes.push(json!({"name": format!("bone_0x{hash:08x}")}));
                joints.nodes.push(nodes.len() - 1);
                joints.helpers.push(nodes.len() - 1);
                joints.inverse_bind.push(IDENTITY);
                joints.nodes.len() - 1
            });
            joint as u16
        })
        .collect()
}

fn material_json(m: &Material, index: usize, label: &str) -> Value {
    let bucket = m.render_bucket & 0x7F;
    let mut v = json!({
        "name": format!("{label}_shader{index}_{:08x}", m.shader_hash),
        "pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.8},
        "doubleSided": true,
    });
    match bucket {
        1 | 2 | 6 => v["alphaMode"] = json!("BLEND"),
        3 => {
            v["alphaMode"] = json!("MASK");
            v["alphaCutoff"] = json!(0.5);
        }
        _ => {}
    }
    let textures: Vec<Value> = m
        .textures
        .iter()
        .map(|t| {
            json!({
                "slot": t.slot,
                "parameter": t.param_hash.map(|h| format!("0x{h:08x}")),
                "parameter_name": t.param_hash.and_then(parameter_name),
                "id64": format!("0x{:016x}", t.id64),
            })
        })
        .collect();
    v["extras"] = json!({
        "shader_hash": format!("0x{:08x}", m.shader_hash),
        "render_bucket": m.render_bucket,
        "render_bucket_mask": m.render_bucket_mask,
        "textures": textures,
    });
    v
}

fn primitive_json(
    b: &mut Builder,
    g: &Geometry,
    material: Option<usize>,
    skin_map: Option<&[u16]>,
    skinned_mesh: bool,
) -> Value {
    let n = g.positions.len();
    let mut pos = Vec::with_capacity(n * 3);
    let mut min = vec![f32::INFINITY; 3];
    let mut max = vec![f32::NEG_INFINITY; 3];
    for p in &g.positions {
        let q = y_up(*p);
        for k in 0..3 {
            min[k] = min[k].min(q[k]);
            max[k] = max[k].max(q[k]);
        }
        pos.extend_from_slice(&q);
    }
    let mut attributes = serde_json::Map::new();
    let position = b.floats(&pos, n, "VEC3", Some(ARRAY_BUFFER));
    b.set_bounds(position, &min, &max);
    attributes.insert("POSITION".into(), json!(position));
    if g.normals.len() == n {
        let data: Vec<f32> = g.normals.iter().flat_map(|v| y_up(*v)).collect();
        attributes.insert(
            "NORMAL".into(),
            json!(b.floats(&data, n, "VEC3", Some(ARRAY_BUFFER))),
        );
    }
    for (set, uvs) in g.uv_sets.iter().enumerate().filter(|(_, u)| u.len() == n) {
        let data: Vec<f32> = uvs.iter().flatten().copied().collect();
        attributes.insert(
            format!("TEXCOORD_{set}"),
            json!(b.floats(&data, n, "VEC2", Some(ARRAY_BUFFER))),
        );
    }
    if g.colors.len() == n {
        let bytes: Vec<u8> = g.colors.iter().flatten().copied().collect();
        let colors = b.accessor(&bytes, Some(ARRAY_BUFFER), UNSIGNED_BYTE, n, "VEC4");
        b.set_normalized(colors);
        attributes.insert("COLOR_0".into(), json!(colors));
    }
    if skinned_mesh {
        let (joints, weights) = skin_arrays(g, skin_map);
        let second = weights.iter().any(|w| w[4..].iter().any(|v| *v > 0.0));
        for set in 0..if second { 2 } else { 1 } {
            let jb: Vec<u8> = joints
                .iter()
                .flat_map(|j| j[4 * set..4 * set + 4].iter().flat_map(|v| v.to_le_bytes()))
                .collect();
            attributes.insert(
                format!("JOINTS_{set}"),
                json!(b.accessor(&jb, Some(ARRAY_BUFFER), UNSIGNED_SHORT, n, "VEC4")),
            );
            let wf: Vec<f32> = weights
                .iter()
                .flat_map(|w| w[4 * set..4 * set + 4].iter().copied())
                .collect();
            attributes.insert(
                format!("WEIGHTS_{set}"),
                json!(b.floats(&wf, n, "VEC4", Some(ARRAY_BUFFER))),
            );
        }
    }
    let indices = if n <= 65535 {
        let bytes: Vec<u8> = g
            .indices
            .iter()
            .flat_map(|i| (*i as u16).to_le_bytes())
            .collect();
        b.accessor(
            &bytes,
            Some(ELEMENT_ARRAY_BUFFER),
            UNSIGNED_SHORT,
            g.indices.len(),
            "SCALAR",
        )
    } else {
        let bytes: Vec<u8> = g.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        b.accessor(
            &bytes,
            Some(ELEMENT_ARRAY_BUFFER),
            UNSIGNED_INT,
            g.indices.len(),
            "SCALAR",
        )
    };
    let mut prim = json!({"attributes": attributes, "indices": indices, "mode": 4});
    if let Some(m) = material {
        prim["material"] = json!(m);
    }
    prim
}

/// Per vertex, eight joints and normalised weights; a vertex without weight is bound to joint 0.
fn skin_arrays(g: &Geometry, map: Option<&[u16]>) -> (Vec<[u16; 8]>, Vec<[f32; 8]>) {
    let n = g.positions.len();
    let mut joints = vec![[0u16; 8]; n];
    let mut weights = vec![[0f32; 8]; n];
    for v in 0..n {
        let (Some(skin), Some(map)) = (&g.skin, map) else {
            weights[v][0] = 1.0;
            continue;
        };
        let total: u32 = skin.weights[v].iter().map(|w| u32::from(*w)).sum();
        if total == 0 {
            weights[v][0] = 1.0;
            continue;
        }
        for k in 0..8 {
            if skin.weights[v][k] != 0 {
                joints[v][k] = map[skin.joints[v][k] as usize];
                weights[v][k] = f32::from(skin.weights[v][k]) / total as f32;
            }
        }
    }
    (joints, weights)
}

fn pack_glb(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(doc).expect("glTF JSON");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    out
}
