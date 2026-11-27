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

