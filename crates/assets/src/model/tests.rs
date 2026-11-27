use serde_json::Value;

use super::testdata::*;
use super::*;

fn decode_resource(r: &Resource, name: Option<&str>) -> Result<ModelSet, ModelError> {
    decode_pages(&r.system, &r.graphics, name)
}

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

fn bone(
    name: &'static str,
    parent: Option<u16>,
    translation: [f32; 3],
    world: [f32; 3],
) -> BoneSpec {
    BoneSpec {
        name,
        hash: joaat(name),
        parent,
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation,
        world,
    }
}

fn two_bones() -> Vec<BoneSpec> {
    vec![
        bone("SKEL_ROOT", None, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
        bone("SKEL_Pelvis", Some(0), [0.5, 0.0, 1.0], [0.5, 0.0, 2.0]),
    ]
}

#[test]
fn joaat_matches_measured_hashes() {
    assert_eq!(joaat("SKEL_ROOT"), 0x6686_685C);
    assert_eq!(joaat("SKEL_Pelvis"), 0x8133_DE19);
    assert_eq!(joaat("SKEL_Head"), 0x34C5_3E77);
    assert_eq!(joaat("a"), 0xCA2E_9442);
}

#[test]
fn parameter_names_match_their_hashes() {
    for name in [
        "diffusetex",
        "speculartex",
        "bumptex",
        "speculartex2",
        "materialid",
        "tinttexture",
        "lookuptexture",
    ] {
        assert_eq!(parameter_name(joaat(name)), Some(name), "{name}");
    }
}

#[test]
fn quantized_positions_decode_for_every_axis_word() {
    for axis in 0..3 {
        let mut g = GeometrySpec::quad();
        g.form = PositionForm::Quantized(axis);
        g.positions = vec![
            [1.0, 2.0, 3.0],
            [-4.0, 5.5, 0.25],
            [10.0, -3.0, 7.0],
            [0.0, 0.0, 0.0],
        ];
        let set = decode_resource(
            &drawable_resource(&DrawableSpec::simple(g.clone())),
            Some("quad"),
        )
        .unwrap();
        let decoded = &set.drawables[0].lods[0].models[0].geometries[0];
        assert_eq!(decoded.positions.len(), 4);
        for (got, want) in decoded.positions.iter().zip(&g.positions) {
            for k in 0..3 {
                assert!(
                    close(got[k], want[k], 1e-3),
                    "axis {axis}: {got:?} vs {want:?}"
                );
            }
        }
    }
}

#[test]
fn float_positions_and_attributes_decode() {
    let mut g = GeometrySpec::quad();
    g.form = PositionForm::Float;
    g.normals = Some(vec![[0.0, 0.0, 1.0]; 4]);
    g.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    g.colors = Some(vec![
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [1, 2, 3, 4],
    ]);
    let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g.clone())), None).unwrap();
    let d = &set.drawables[0].lods[0].models[0].geometries[0];
    assert_eq!(d.positions, g.positions);
    assert_eq!(d.normals.len(), 4);
    assert_eq!(d.uv_sets, vec![g.uvs.clone().unwrap()]);
    assert_eq!(d.colors, g.colors.clone().unwrap());
    assert_eq!(d.indices, vec![0, 1, 2, 0, 2, 3]);
}

#[test]
fn quantized_block_decodes_octahedral_normals_and_snorm_uvs() {
    let mut g = GeometrySpec::quad();
    g.normals = Some(vec![
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.577, 0.577, 0.577],
    ]);
    g.uvs = Some(vec![[0.0, 0.0], [1.0, 0.5], [2.0, -2.0], [-4.0, 4.0]]);
    let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g.clone())), None).unwrap();
    let d = &set.drawables[0].lods[0].models[0].geometries[0];
    for (got, want) in d.normals.iter().zip(g.normals.as_ref().unwrap()) {
        let dot: f32 = (0..3).map(|k| got[k] * want[k]).sum();
        assert!(dot > 0.99, "{got:?} vs {want:?}");
    }
    for (got, want) in d.uv_sets[0].iter().zip(g.uvs.as_ref().unwrap()) {
        assert!(
            close(got[0], want[0], 2e-3) && close(got[1], want[1], 2e-3),
            "{got:?} vs {want:?}"
        );
    }
}

#[test]
fn indices_cross_cluster_boundaries_and_short_references() {
    let n = 300usize;
    let positions: Vec<[f32; 3]> = (0..n)
        .map(|i| [i as f32, (i * i % 17) as f32, 0.0])
        .collect();
    let triangles: Vec<[u32; 3]> = (0..n as u32 - 2).map(|i| [i, i + 1, i + 2]).collect();
    for short in [false, true] {
        let mut g = GeometrySpec::quad();
        g.positions = positions.clone();
        g.triangles = triangles.clone();
        g.short_refs = short;
        let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g)), None).unwrap();
        let d = &set.drawables[0].lods[0].models[0].geometries[0];
        let flat: Vec<u32> = triangles.iter().flatten().copied().collect();
        assert_eq!(d.indices, flat);
    }
}

#[test]
fn winding_follows_the_stored_normals() {
    let mut g = GeometrySpec::quad();
    g.form = PositionForm::Float;
    g.positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    g.normals = Some(vec![[0.0, 0.0, -1.0]; 4]);
    let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g)), None).unwrap();
    let d = &set.drawables[0].lods[0].models[0].geometries[0];
    assert_eq!(d.indices, vec![0, 2, 1, 0, 3, 2]);
}

fn skinned_spec(
    skin: SkinForm,
    influences: Vec<Vec<(u16, u8)>>,
    palette: Vec<u32>,
) -> DrawableSpec {
    let mut g = GeometrySpec::quad();
    g.skin = skin;
    g.influences = influences;
    let mut d = DrawableSpec::simple(g);
    d.lods[0].1[0].palette = palette;
    d
}

#[test]
fn skin_layouts_decode_sorted_by_weight() {
    let influences = vec![
        vec![(0, 255)],
        vec![(1, 100), (0, 155)],
        vec![(2, 255)],
        vec![
            (0, 10),
            (1, 20),
            (2, 30),
            (0, 40),
            (1, 50),
            (2, 60),
            (0, 20),
            (1, 25),
        ],
    ];
    let palette = vec![0x11, 0x22, 0x33];
    for skin in [SkinForm::Bytes(2), SkinForm::Packed(2), SkinForm::Words(2)] {
        let set = decode_resource(
            &drawable_resource(&skinned_spec(skin, influences.clone(), palette.clone())),
            None,
        )
        .unwrap();
        let g = &set.drawables[0].lods[0].models[0].geometries[0];
        let s = g.skin.as_ref().expect("skin");
        assert_eq!(s.joints[0][0], 0);
        assert_eq!(s.weights[0], [255, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&s.weights[1][..2], &[155, 100]);
        assert_eq!(&s.joints[1][..2], &[0, 1]);
        assert_eq!(s.weights[3], [60, 50, 40, 30, 25, 20, 20, 10]);
        assert!(s.weights.iter().all(|w| w.windows(2).all(|p| p[0] >= p[1])));
    }
}

#[test]
fn rigid_and_constant_skin_bind_one_bone() {
    let rigid = skinned_spec(
        SkinForm::Rigid8,
        vec![
            vec![(1, 255)],
            vec![(0, 255)],
            vec![(2, 255)],
            vec![(1, 255)],
        ],
        vec![1, 2, 3],
    );
    let set = decode_resource(&drawable_resource(&rigid), None).unwrap();
    let s = set.drawables[0].lods[0].models[0].geometries[0]
        .skin
        .clone()
        .unwrap();
    assert_eq!(s.layout, SkinLayout::Rigid);
    assert_eq!(
        s.joints.iter().map(|j| j[0]).collect::<Vec<_>>(),
        vec![1, 0, 2, 1]
    );

    let constant = skinned_spec(SkinForm::Constant(2), Vec::new(), vec![1, 2, 3]);
    let set = decode_resource(&drawable_resource(&constant), None).unwrap();
    let s = set.drawables[0].lods[0].models[0].geometries[0]
        .skin
        .clone()
        .unwrap();
    assert_eq!(s.layout, SkinLayout::Constant);
    assert!(s.joints.iter().all(|j| j[0] == 2) && s.weights.iter().all(|w| w[0] == 255));
}

#[test]
fn bone_index_past_the_palette_is_an_error() {
    let spec = skinned_spec(SkinForm::Bytes(1), vec![vec![(5, 255)]; 4], vec![1, 2]);
    let err = decode_resource(&drawable_resource(&spec), None).unwrap_err();
    assert!(matches!(err, ModelError::Invalid(_)), "{err}");
}

#[test]
fn materials_lods_and_names_decode() {
    let mut spec = DrawableSpec::simple(GeometrySpec::quad());
    spec.name = Some("prop_chair");
    spec.lods.push((
        60.0,
        vec![ModelSpec {
            direct_bones: 0,
            geometries: vec![GeometrySpec::quad()],
            palette: vec![],
        }],
    ));
    spec.materials = vec![
        MaterialSpec {
            shader: 0xAAAA_0001,
            bucket: 0,
            textures: vec![],
        },
        MaterialSpec {
            shader: 0xAAAA_0002,
            bucket: 0x83,
            textures: vec![
                TextureSpec {
                    name_hash: 7,
                    param: joaat("diffusetex"),
                    id64: 0x0123_4567_89AB_CDEF,
                },
                TextureSpec {
                    name_hash: 8,
                    param: joaat("bumptex"),
                    id64: 0x1111_2222_3333_4444,
                },
            ],
        },
    ];
    spec.lods[0].1[0].geometries[0].shader = 1;
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let d = &set.drawables[0];
    assert_eq!(set.kind, ResourceKind::Drawable);
    assert_eq!(d.name.as_deref(), Some("prop_chair"));
    assert_eq!(
        d.lods
            .iter()
            .map(|l| (l.level, l.distance))
            .collect::<Vec<_>>(),
        vec![(0, 15.0), (1, 60.0)]
    );
    assert_eq!(d.lods[0].models[0].geometries[0].material, 1);
    assert_eq!(d.materials[1].shader_hash, 0xAAAA_0002);
    assert_eq!(d.materials[1].render_bucket, 0x83);
    assert_eq!(d.materials[1].textures.len(), 2);
    assert_eq!(d.materials[1].textures[0].id64, 0x0123_4567_89AB_CDEF);
    assert_eq!(
        d.materials[1].textures[0].param_hash,
        Some(joaat("diffusetex"))
    );
    assert_eq!(d.materials[1].textures[1].slot, 1);
    assert_eq!(d.materials[1].textures[1].name_hash, 8);
    assert_eq!(d.materials[1].raw_parameters.len(), 3);
}

#[test]
fn dictionary_and_fragment_resources_decode() {
    let child = |hash: u32, name: &'static str| {
        let mut d = DrawableSpec::simple(GeometrySpec::quad());
        d.name = Some(name);
        (hash, d)
    };
    let dict = dictionary_resource(&[child(0x1111, "one"), child(0x2222, "two")]);
    let set = decode_resource(&dict, Some("props")).unwrap();
    assert_eq!(set.kind, ResourceKind::Dictionary);
    assert_eq!(set.drawables.len(), 2);
    assert_eq!(set.drawables[1].name_hash, Some(0x2222));

    let mut main = DrawableSpec::simple(GeometrySpec::quad());
    main.skeleton = two_bones();
    let frag = fragment_resource(&[main, DrawableSpec::simple(GeometrySpec::quad())]);
    let set = decode_resource(&frag, Some("car")).unwrap();
    assert_eq!(set.kind, ResourceKind::Fragment);
    assert_eq!(set.drawables.len(), 2);
    assert_eq!(set.drawables[0].skeleton.as_ref().unwrap().bones.len(), 2);
    assert!(set.drawables[1].skeleton.is_none());
}

#[test]
fn skeleton_decodes_with_hierarchy() {
    let mut spec = DrawableSpec::simple(GeometrySpec::quad());
    spec.skeleton = two_bones();
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let s = set.drawables[0].skeleton.as_ref().unwrap();
    assert_eq!(s.bones[0].name.as_deref(), Some("SKEL_ROOT"));
    assert_eq!(s.bones[1].parent, Some(0));
    assert_eq!(s.bones[0].parent, None);
    assert_eq!(s.find_hash(joaat("SKEL_Pelvis")), Some(1));
    assert_eq!(s.bones[1].inverse_bind[12..15], [-0.5, 0.0, -2.0]);
}

#[test]
fn parent_cycles_are_rejected() {
    let mut spec = DrawableSpec::simple(GeometrySpec::quad());
    spec.skeleton = vec![
        bone("a", Some(1), [0.0; 3], [0.0; 3]),
        bone("b", Some(0), [0.0; 3], [0.0; 3]),
    ];
    assert!(matches!(
        decode_resource(&drawable_resource(&spec), None),
        Err(ModelError::Invalid(_))
    ));
}

#[test]
fn garbage_and_truncation_never_panic() {
    assert!(matches!(
        decode_pages(&[], &[], None),
        Err(ModelError::NotAModel)
    ));
    assert!(matches!(
        decode_pages(&[0u8; 400], &[], None),
        Err(ModelError::NotAModel)
    ));
    let mut g = GeometrySpec::quad();
    g.normals = Some(vec![[0.0, 0.0, 1.0]; 4]);
    let r = drawable_resource(&DrawableSpec::simple(g));
    for cut in (0..r.system.len()).step_by(7) {
        let _ = decode_pages(&r.system[..cut], &r.graphics, None);
    }
    for cut in 0..r.graphics.len() {
        let _ = decode_pages(&r.system, &r.graphics[..cut], None);
    }
    let mut state = 0x1234_5678u32;
    for _ in 0..300 {
        let mut system = r.system.clone();
        let mut graphics = r.graphics.clone();
        for _ in 0..4 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let at = (state >> 8) as usize % (system.len() + graphics.len());
            if at < system.len() {
                system[at] ^= (state >> 3) as u8 | 1;
            } else {
                graphics[at - system.len()] ^= (state >> 3) as u8 | 1;
            }
        }
        let _ = decode_pages(&system, &graphics, None);
    }
}

#[test]
fn decode_splits_joined_bytes() {
    let r = drawable_resource(&DrawableSpec::simple(GeometrySpec::quad()));
    let mut joined = r.system.clone();
    joined.extend_from_slice(&r.graphics);
    let info = ResourceInfo {
        name: Some("x".into()),
        system_len: r.system.len(),
    };
    let set = decode(&joined, &info).unwrap();
    assert_eq!(set.name.as_deref(), Some("x"));
    let bad = ResourceInfo {
        name: None,
        system_len: joined.len() + 1,
    };
    assert!(decode(&joined, &bad).is_err());
}

// ---- glTF checks

pub(super) struct Glb {
    pub(super) doc: Value,
    pub(super) bin: Vec<u8>,
}

pub(super) fn parse_glb(bytes: &[u8]) -> Glb {
    assert_eq!(&bytes[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
        bytes.len()
    );
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    assert_eq!(&bytes[16..20], b"JSON");
    assert_eq!(json_len % 4, 0);
    let doc: Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
    let rest = &bytes[20 + json_len..];
    let bin = if rest.is_empty() {
        Vec::new()
    } else {
        let len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
        assert_eq!(&rest[4..8], b"BIN\0");
        assert_eq!(len % 4, 0);
        assert_eq!(rest.len(), 8 + len);
        rest[8..].to_vec()
    };
    Glb { doc, bin }
}

fn component_size(c: u64) -> usize {
    match c {
        5121 => 1,
        5123 => 2,
        5125 | 5126 => 4,
        other => panic!("component {other}"),
    }
}

fn element_count(kind: &str) -> usize {
    match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT4" => 16,
        other => panic!("type {other}"),
    }
}

impl Glb {
    fn accessor_bytes(&self, i: usize) -> (&[u8], &Value) {
        let a = &self.doc["accessors"][i];
        let view = &self.doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        let need = a["count"].as_u64().unwrap() as usize
            * component_size(a["componentType"].as_u64().unwrap())
            * element_count(a["type"].as_str().unwrap());
        assert!(need <= length, "accessor {i} needs {need} of {length}");
        assert!(offset + length <= self.bin.len());
        assert_eq!(offset % 4, 0, "view {i} alignment");
        (&self.bin[offset..offset + need], a)
    }

    pub(super) fn floats(&self, i: usize) -> Vec<f32> {
        let (b, a) = self.accessor_bytes(i);
        assert_eq!(a["componentType"], 5126);
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn uints(&self, i: usize) -> Vec<u32> {
        let (b, a) = self.accessor_bytes(i);
        match a["componentType"].as_u64().unwrap() {
            5123 => b
                .chunks_exact(2)
                .map(|c| u32::from(u16::from_le_bytes(c.try_into().unwrap())))
                .collect(),
            5125 => b
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect(),
            other => panic!("component {other}"),
        }
    }

    /// Structural validation: no empty arrays, bounds, indices, skins, node tree.
    pub(super) fn validate(&self) {
        for (key, v) in self.doc.as_object().unwrap() {
            if let Some(a) = v.as_array() {
                assert!(!a.is_empty(), "{key} is empty");
            }
        }
        let declared = self.doc["buffers"][0]["byteLength"].as_u64().unwrap() as usize;
        assert!(declared <= self.bin.len() && self.bin.len() - declared < 4);
        for i in 0..self.doc["accessors"].as_array().unwrap().len() {
            self.accessor_bytes(i);
        }
        for mesh in self.doc["meshes"].as_array().unwrap() {
            for prim in mesh["primitives"].as_array().unwrap() {
                let attrs = prim["attributes"].as_object().unwrap();
                let pos = attrs["POSITION"].as_u64().unwrap() as usize;
                let count = self.doc["accessors"][pos]["count"].as_u64().unwrap();
                let data = self.floats(pos);
                let a = &self.doc["accessors"][pos];
                for k in 0..3 {
                    let lo = data
                        .iter()
                        .skip(k)
                        .step_by(3)
                        .copied()
                        .fold(f32::INFINITY, f32::min);
                    let hi = data
                        .iter()
                        .skip(k)
                        .step_by(3)
                        .copied()
                        .fold(f32::NEG_INFINITY, f32::max);
                    assert_eq!(a["min"][k].as_f64().unwrap() as f32, lo);
                    assert_eq!(a["max"][k].as_f64().unwrap() as f32, hi);
                }
                for (name, v) in attrs {
                    assert_eq!(
                        self.doc["accessors"][v.as_u64().unwrap() as usize]["count"]
                            .as_u64()
                            .unwrap(),
                        count,
                        "{name}"
                    );
                }
                let idx = self.uints(prim["indices"].as_u64().unwrap() as usize);
                assert_eq!(idx.len() % 3, 0);
                assert!(idx.iter().all(|i| u64::from(*i) < count));
                if let Some(m) = prim.get("material") {
                    assert!(
                        (m.as_u64().unwrap() as usize)
                            < self.doc["materials"].as_array().unwrap().len()
                    );
                }
            }
        }
        let nodes = self.doc["nodes"].as_array().unwrap();
        let mut parent = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n["children"].as_array().into_iter().flatten() {
                let c = c.as_u64().unwrap() as usize;
                assert!(parent[c].is_none(), "node {c} has two parents");
                parent[c] = Some(i);
            }
        }
        for start in 0..nodes.len() {
            let (mut at, mut steps) = (start, 0);
            while let Some(p) = parent[at] {
                at = p;
                steps += 1;
                assert!(steps <= nodes.len(), "cycle");
            }
        }
        for s in self.doc["scenes"][0]["nodes"].as_array().unwrap() {
            assert!(
                parent[s.as_u64().unwrap() as usize].is_none(),
                "scene lists a child node"
            );
        }
        for skin in self.doc["skins"].as_array().into_iter().flatten() {
            let joints = skin["joints"].as_array().unwrap().len();
            let ibm =
                &self.doc["accessors"][skin["inverseBindMatrices"].as_u64().unwrap() as usize];
            assert_eq!(ibm["count"].as_u64().unwrap() as usize, joints);
            assert_eq!(ibm["type"], "MAT4");
        }
        for n in nodes {
            if n.get("skin").is_some() {
                for prim in self.doc["meshes"][n["mesh"].as_u64().unwrap() as usize]["primitives"]
                    .as_array()
                    .unwrap()
                {
                    let attrs = prim["attributes"].as_object().unwrap();
                    let skin = &self.doc["skins"][n["skin"].as_u64().unwrap() as usize];
                    let joint_count = skin["joints"].as_array().unwrap().len() as u32;
                    for set in 0..2 {
                        let (Some(j), Some(w)) = (
                            attrs.get(&format!("JOINTS_{set}")),
                            attrs.get(&format!("WEIGHTS_{set}")),
                        ) else {
                            continue;
                        };
                        assert!(
                            self.uints(j.as_u64().unwrap() as usize)
                                .iter()
                                .all(|v| *v < joint_count)
                        );
                        let _ = self.floats(w.as_u64().unwrap() as usize);
                    }
                    let first = self.floats(attrs["WEIGHTS_0"].as_u64().unwrap() as usize);
                    let second = attrs
                        .get("WEIGHTS_1")
                        .map(|w| self.floats(w.as_u64().unwrap() as usize));
                    for v in 0..first.len() / 4 {
                        let mut sum: f32 = first[4 * v..4 * v + 4].iter().sum();
                        if let Some(s) = &second {
                            sum += s[4 * v..4 * v + 4].iter().sum::<f32>();
                        }
                        assert!(close(sum, 1.0, 1e-5), "vertex {v} weights sum to {sum}");
                    }
                }
            } else if let Some(m) = n.get("mesh") {
                for prim in self.doc["meshes"][m.as_u64().unwrap() as usize]["primitives"]
                    .as_array()
                    .unwrap()
                {
                    assert!(prim["attributes"].get("JOINTS_0").is_none());
                }
            }
        }
    }
}

pub(super) type Mat = [f32; 16];

pub(super) fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn node_matrix(n: &Value) -> Mat {
    let get = |k: &str, d: &[f32]| -> Vec<f32> {
        n.get(k).map_or(d.to_vec(), |v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect()
        })
    };
    let (t, q, s) = (
        get("translation", &[0.0; 3]),
        get("rotation", &[0.0, 0.0, 0.0, 1.0]),
        get("scale", &[1.0; 3]),
    );
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut m = [0.0; 16];
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] = r[row][c] * s[c];
        }
    }
    m[12..15].copy_from_slice(&t);
    m[15] = 1.0;
    m
}

pub(super) fn world_matrices(glb: &Glb) -> Vec<Mat> {
    let nodes = glb.doc["nodes"].as_array().unwrap();
    let mut world = vec![None; nodes.len()];
    fn go(nodes: &[Value], world: &mut Vec<Option<Mat>>, at: usize, parent: Mat) {
        let m = mul(&parent, &node_matrix(&nodes[at]));
        world[at] = Some(m);
        for c in nodes[at]["children"].as_array().into_iter().flatten() {
            go(nodes, world, c.as_u64().unwrap() as usize, m);
        }
    }
    let identity: Mat = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    for s in glb.doc["scenes"][0]["nodes"].as_array().unwrap() {
        go(nodes, &mut world, s.as_u64().unwrap() as usize, identity);
    }
    world.into_iter().map(|m| m.unwrap_or(identity)).collect()
}

fn export_one(set: &ModelSet) -> (ExportedModel, Glb) {
    let mut out = export(set, &ExportOptions::default()).unwrap();
    let e = out.remove(0);
    let glb = parse_glb(&e.glb);
    glb.validate();
    (e, glb)
}

#[test]
fn static_model_exports_valid_gltf_in_y_up() {
    let mut g = GeometrySpec::quad();
    g.form = PositionForm::Float;
    g.normals = Some(vec![[0.0, 0.0, 1.0]; 4]);
    g.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    g.colors = Some(vec![[9, 8, 7, 255]; 4]);
    let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g)), Some("quad")).unwrap();
    let (e, glb) = export_one(&set);
    assert_eq!(e.stem, "quad");
    let prim = &glb.doc["meshes"][0]["primitives"][0];
    let attrs = &prim["attributes"];
    let pos = glb.floats(attrs["POSITION"].as_u64().unwrap() as usize);
    assert_eq!(&pos[3..6], &[1.0, 0.0, 0.0]);
    assert_eq!(&pos[6..9], &[1.0, 0.0, -2.0]);
    assert_eq!(&pos[9..12], &[0.0, 0.5, -2.0]);
    let normals = glb.floats(attrs["NORMAL"].as_u64().unwrap() as usize);
    assert_eq!(&normals[0..3], &[0.0, 1.0, 0.0]);
    assert!(attrs.get("TEXCOORD_0").is_some() && attrs.get("COLOR_0").is_some());
    assert!(glb.doc.get("skins").is_none());
    assert_eq!(glb.doc["materials"][0]["alphaMode"], Value::Null);
    assert_eq!(
        glb.doc["materials"][0]["extras"]["shader_hash"],
        "0xdeadbeef"
    );
}

fn skinned_character() -> ModelSet {
    let influences = vec![
        vec![(0, 255)],
        vec![(1, 128), (0, 127)],
        vec![(2, 255)],
        vec![
            (0, 10),
            (1, 20),
            (2, 30),
            (0, 40),
            (1, 50),
            (2, 60),
            (0, 20),
            (1, 25),
        ],
    ];
    let mut spec = skinned_spec(
        SkinForm::Bytes(2),
        influences,
        vec![joaat("SKEL_ROOT"), joaat("SKEL_Pelvis"), 0xCAFE_F00D],
    );
    spec.skeleton = two_bones();
    spec.name = Some("ped");
    decode_resource(&drawable_resource(&spec), None).unwrap()
}

#[test]
fn skinned_model_has_consistent_skin_and_rest_pose() {
    let set = skinned_character();
    let (_, glb) = export_one(&set);
    let skin = &glb.doc["skins"][0];
    let joints = skin["joints"].as_array().unwrap();
    assert_eq!(
        joints.len(),
        3,
        "two bones and one helper for the unknown hash"
    );
    let names: Vec<_> = joints
        .iter()
        .map(|j| {
            glb.doc["nodes"][j.as_u64().unwrap() as usize]["name"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(names, vec!["SKEL_ROOT", "SKEL_Pelvis", "bone_0xcafef00d"]);

    let ibm = glb.floats(skin["inverseBindMatrices"].as_u64().unwrap() as usize);
    let world = world_matrices(&glb);
    for (j, node) in joints.iter().enumerate() {
        let w = world[node.as_u64().unwrap() as usize];
        let m: Mat = ibm[16 * j..16 * j + 16].try_into().unwrap();
        let rest = mul(&w, &m);
        for (i, v) in rest.iter().enumerate() {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!(close(*v, want, 1e-5), "joint {j}: {rest:?}");
        }
    }

    let prim = &glb.doc["meshes"][0]["primitives"][0];
    let attrs = &prim["attributes"];
    assert!(attrs.get("JOINTS_1").is_some() && attrs.get("WEIGHTS_1").is_some());
    let j0 = glb.uints(attrs["JOINTS_0"].as_u64().unwrap() as usize);
    let w0 = glb.floats(attrs["WEIGHTS_0"].as_u64().unwrap() as usize);
    assert_eq!(&j0[4..6], &[1, 0]);
    assert!(close(w0[4], 128.0 / 255.0, 1e-5) && close(w0[5], 127.0 / 255.0, 1e-5));
    assert_eq!(j0[8], 2);
}

#[test]
fn external_skeleton_binds_character_parts() {
    let skeleton = Skeleton {
        bones: decode_resource(
            &{
                let mut s = DrawableSpec::simple(GeometrySpec::quad());
                s.skeleton = two_bones();
                drawable_resource(&s)
            },
            None,
        )
        .unwrap()
        .drawables[0]
            .skeleton
            .clone()
            .unwrap()
            .bones,
    };
    let mut spec = skinned_spec(
        SkinForm::Bytes(1),
        vec![
            vec![(0, 255)],
            vec![(1, 255)],
            vec![(1, 255)],
            vec![(0, 255)],
        ],
        vec![joaat("SKEL_ROOT"), joaat("SKEL_Pelvis")],
    );
    spec.name = Some("part");
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let options = ExportOptions {
        skeleton: Some(&skeleton),
        ..ExportOptions::default()
    };
    let e = export(&set, &options).unwrap().remove(0);
    let glb = parse_glb(&e.glb);
    glb.validate();
    assert_eq!(glb.doc["skins"][0]["joints"].as_array().unwrap().len(), 2);
    let sidecar: Value = serde_json::from_str(&e.sidecar).unwrap();
    assert_eq!(sidecar["drawables"][0]["skeleton"]["own"], false);
    assert_eq!(sidecar["drawables"][0]["skeleton"]["bones"], 2);
}

#[test]
fn skinned_rest_pose_reproduces_vertices() {
    let set = skinned_character();
    let (_, glb) = export_one(&set);
    let skin = &glb.doc["skins"][0];
    let joints: Vec<usize> = skin["joints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j.as_u64().unwrap() as usize)
        .collect();
    let ibm = glb.floats(skin["inverseBindMatrices"].as_u64().unwrap() as usize);
    let world = world_matrices(&glb);
    let attrs = &glb.doc["meshes"][0]["primitives"][0]["attributes"];
    let pos = glb.floats(attrs["POSITION"].as_u64().unwrap() as usize);
    let sets: Vec<(Vec<u32>, Vec<f32>)> = (0..2)
        .map(|s| {
            (
                glb.uints(attrs[format!("JOINTS_{s}")].as_u64().unwrap() as usize),
                glb.floats(attrs[format!("WEIGHTS_{s}")].as_u64().unwrap() as usize),
            )
        })
        .collect();
    for v in 0..4 {
        let mut out = [0.0f32; 3];
        for (j, w) in &sets {
            for k in 0..4 {
                let joint = j[4 * v + k] as usize;
                let m = mul(
                    &world[joints[joint]],
                    &ibm[16 * joint..16 * joint + 16].try_into().unwrap(),
                );
                for r in 0..3 {
                    let t = m[r] * pos[3 * v]
                        + m[4 + r] * pos[3 * v + 1]
                        + m[8 + r] * pos[3 * v + 2]
                        + m[12 + r];
                    out[r] += w[4 * v + k] * t;
                }
            }
        }
        for r in 0..3 {
            assert!(
                close(out[r], pos[3 * v + r], 1e-4),
                "vertex {v}: {out:?} vs {:?}",
                &pos[3 * v..3 * v + 3]
            );
        }
    }
}

#[test]
fn lod_selection_and_dictionary_exports() {
    let mut a = DrawableSpec::simple(GeometrySpec::quad());
    a.name = Some("alpha");
    a.lods.push((
        50.0,
        vec![ModelSpec {
            direct_bones: 0,
            geometries: vec![GeometrySpec::quad()],
            palette: vec![],
        }],
    ));
    let b = DrawableSpec::simple(GeometrySpec::quad());
    let set = decode_resource(&dictionary_resource(&[(0xAA, a), (0xBB, b)]), Some("dict")).unwrap();

    let primary = export(&set, &ExportOptions::default()).unwrap();
    assert_eq!(primary.len(), 2);
    assert_eq!(primary[0].stem, "alpha");
    assert_eq!(primary[1].stem, "dict_000000bb");
    let glb = parse_glb(&primary[0].glb);
    glb.validate();
    assert_eq!(glb.doc["meshes"].as_array().unwrap().len(), 1);

    let all = export(
        &set,
        &ExportOptions {
            lods: LodSelection::All,
            ..ExportOptions::default()
        },
    )
    .unwrap();
    let glb = parse_glb(&all[0].glb);
    glb.validate();
    assert_eq!(glb.doc["meshes"].as_array().unwrap().len(), 2);
    assert_eq!(glb.doc["nodes"][1]["name"], "alpha_lod1_model0");

    let sidecar: Value = serde_json::from_str(&all[0].sidecar).unwrap();
    let lods = sidecar["drawables"][0]["lods"].as_array().unwrap();
    assert_eq!(lods.len(), 2);
    assert_eq!(lods[1]["distance"], 50.0);
    assert_eq!(lods[0]["triangles"], 2);
    assert_eq!(lods[0]["models"][0]["geometries"][0]["vertices"], 4);
}

#[test]
fn materials_carry_alpha_modes_textures_and_sidecar_links() {
    let mut spec = DrawableSpec::simple(GeometrySpec::quad());
    spec.materials = vec![
        MaterialSpec {
            shader: 1,
            bucket: 1,
            textures: vec![],
        },
        MaterialSpec {
            shader: 2,
            bucket: 3,
            textures: vec![
                TextureSpec {
                    name_hash: 1,
                    param: joaat("bumptex"),
                    id64: 0xB,
                },
                TextureSpec {
                    name_hash: 2,
                    param: joaat("diffusetex"),
                    id64: 0xD,
                },
            ],
        },
        MaterialSpec {
            shader: 3,
            bucket: 0,
            textures: vec![TextureSpec {
                name_hash: 3,
                param: joaat("speculartex"),
                id64: 0xE,
            }],
        },
    ];
    spec.name = Some("m");
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let resolve = |id: u64| (id == 0xD || id == 0xE).then(|| format!("textures/{id:016x}.dds"));
    let options = ExportOptions {
        texture_uri: Some(&resolve),
        ..ExportOptions::default()
    };
    let e = export(&set, &options).unwrap().remove(0);
    let glb = parse_glb(&e.glb);
    glb.validate();
    let m = &glb.doc["materials"];
    assert_eq!(m[0]["alphaMode"], "BLEND");
    assert_eq!(m[1]["alphaMode"], "MASK");
    assert_eq!(m[1]["pbrMetallicRoughness"]["baseColorTexture"]["index"], 0);
    assert!(
        m[2]["pbrMetallicRoughness"]
            .get("baseColorTexture")
            .is_none(),
        "no fallback to a specular map"
    );
    assert_eq!(glb.doc["images"][0]["uri"], "textures/000000000000000d.dds");
    assert_eq!(glb.doc["images"].as_array().unwrap().len(), 1);

    let sidecar: Value = serde_json::from_str(&e.sidecar).unwrap();
    let mats = &sidecar["drawables"][0]["materials"];
    assert_eq!(mats[1]["shader_hash"], "0x00000002");
    assert_eq!(mats[1]["textures"][1]["parameter_name"], "diffusetex");
    assert_eq!(mats[1]["textures"][1]["id64"], "0x000000000000000d");
    assert_eq!(mats[0]["render_bucket"], 1);
}

#[test]
fn large_meshes_use_wide_indices() {
    let n = 70_000usize;
    let mut g = GeometrySpec::quad();
    g.form = PositionForm::Float;
    g.positions = (0..n)
        .map(|i| [(i % 300) as f32, (i / 300) as f32, 0.0])
        .collect();
    g.triangles = (0..2000u32)
        .map(|i| [i * 30, i * 30 + 1, i * 30 + 2])
        .collect();
    g.triangles.push([69_997, 69_998, 69_999]);
    let set = decode_resource(&drawable_resource(&DrawableSpec::simple(g)), None).unwrap();
    let (_, glb) = export_one(&set);
    let idx = glb.doc["meshes"][0]["primitives"][0]["indices"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(glb.doc["accessors"][idx]["componentType"], 5125);
    assert_eq!(glb.uints(idx).last(), Some(&69_999));
}

#[test]
fn palette_less_models_index_the_skeleton_directly() {
    let mut spec = skinned_spec(
        SkinForm::Bytes(1),
        vec![
            vec![(1, 255)],
            vec![(0, 255)],
            vec![(1, 128), (0, 127)],
            vec![(0, 255)],
        ],
        Vec::new(),
    );
    spec.lods[0].1[0].direct_bones = 2;
    spec.skeleton = two_bones();
    spec.name = Some("car");
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let model = &set.drawables[0].lods[0].models[0];
    assert!(model.bone_palette.is_empty());
    assert_eq!(model.bone_count, 2);
    let (_, glb) = export_one(&set);
    let joints = glb.doc["skins"][0]["joints"].as_array().unwrap();
    assert_eq!(joints.len(), 2, "indices map straight onto the skeleton");
    let attrs = &glb.doc["meshes"][0]["primitives"][0]["attributes"];
    let j0 = glb.uints(attrs["JOINTS_0"].as_u64().unwrap() as usize);
    assert_eq!(&j0[0..4], &[1, 0, 0, 0]);

    spec.skeleton = Vec::new();
    let set = decode_resource(&drawable_resource(&spec), None).unwrap();
    let (_, glb) = export_one(&set);
    let names: Vec<_> = glb.doc["skins"][0]["joints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| {
            glb.doc["nodes"][j.as_u64().unwrap() as usize]["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(names, vec!["bone_0", "bone_1"]);
}
