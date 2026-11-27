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

