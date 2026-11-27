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

