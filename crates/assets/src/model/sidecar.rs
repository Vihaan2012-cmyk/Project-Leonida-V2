use serde_json::{Value, json};

use super::types::*;

const PARAMETER_NAMES: [(&str, u32); 7] = [
    ("diffusetex", 0xC8E8_C282),
    ("speculartex", 0xC6F4_6E79),
    ("bumptex", 0x1E77_BF4C),
    ("speculartex2", 0x739E_3D0D),
    ("materialid", 0x8ADB_8480),
    ("tinttexture", 0x3FB7_964F),
    ("lookuptexture", 0xC075_F9B6),
];

/// Name of a texture parameter hash, for the ones measured.
pub fn parameter_name(hash: u32) -> Option<&'static str> {
    PARAMETER_NAMES
        .iter()
        .find(|(_, h)| *h == hash)
        .map(|(n, _)| *n)
}

fn hex(v: u32) -> String {
    format!("0x{v:08x}")
}

pub(crate) fn sidecar_json(
    set: &ModelSet,
    drawables: &[&Drawable],
    external: Option<&Skeleton>,
) -> Value {
    let kind = match set.kind {
        ResourceKind::Drawable => "drawable",
        ResourceKind::Dictionary => "dictionary",
        ResourceKind::Fragment => "fragment",
    };
    let mut notes = vec![
        "texture links are read from an inferred record layout: match id64 against a texture pool"
            .to_string(),
        "raw_parameters are the parameter table words, not decoded".to_string(),
    ];
    notes.extend(set.warnings.iter().cloned());
    json!({
        "format": "leonida-model/1",
        "name": set.name,
        "kind": kind,
        "up_axis": "Y (converted from the game's Z-up)",
        "drawables": drawables.iter().map(|d| drawable_json(d, external)).collect::<Vec<_>>(),
        "notes": notes,
    })
}

fn drawable_json(d: &Drawable, external: Option<&Skeleton>) -> Value {
    let skeleton = d.skeleton.as_ref().or(external);
    json!({
        "name": d.name,
        "name_hash": d.name_hash.map(hex),
        "bounds": {"min": d.bounds.min, "max": d.bounds.max, "radius": d.bounds.radius},
        "embedded_textures": d.embedded_textures,
        "skeleton": skeleton.map(|s| json!({
            "bones": s.bones.len(),
            "own": d.skeleton.is_some(),
            "joints": s.bones.iter().map(|b| json!({
                "name": b.name,
                "hash": hex(b.hash),
                "parent": b.parent,
            })).collect::<Vec<_>>(),
        })),
        "lods": d.lods.iter().map(lod_json).collect::<Vec<_>>(),
        "materials": d.materials.iter().enumerate().map(|(i, m)| material_json(i, m)).collect::<Vec<_>>(),
    })
}

fn lod_json(l: &Lod) -> Value {
    json!({
        "level": l.level,
        "distance": l.distance,
        "models": l.models.iter().enumerate().map(|(i, m)| json!({
            "index": i,
            "bone_palette": m.bone_palette.iter().map(|h| hex(*h)).collect::<Vec<_>>(),
            "geometries": m.geometries.iter().enumerate().map(|(gi, g)| json!({
                "index": gi,
                "material": g.material,
                "vertices": g.positions.len(),
                "triangles": g.triangle_count(),
                "uv_sets": g.uv_sets.len(),
                "has_normals": !g.normals.is_empty(),
                "has_colors": !g.colors.is_empty(),
                "skin": g.skin.as_ref().map(|s| s.layout.name()),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "vertices": l.models.iter().flat_map(|m| &m.geometries).map(|g| g.positions.len()).sum::<usize>(),
        "triangles": l.models.iter().flat_map(|m| &m.geometries).map(Geometry::triangle_count).sum::<usize>(),
    })
}

fn material_json(index: usize, m: &Material) -> Value {
    json!({
        "index": index,
        "shader_hash": hex(m.shader_hash),
        "render_bucket": m.render_bucket & 0x7F,
        "render_bucket_flag": m.render_bucket >> 7,
        "render_bucket_mask": format!("0x{:08x}", m.render_bucket_mask),
        "textures": m.textures.iter().map(|t| json!({
            "parameter": hex(t.param_hash),
            "parameter_name": parameter_name(t.param_hash),
            "name_hash": hex(t.name_hash),
            "id64": format!("0x{:016x}", t.id64),
        })).collect::<Vec<_>>(),
        "raw_parameters": m.raw_parameters.iter().map(|w| [hex(w[0]), hex(w[1])]).collect::<Vec<_>>(),
    })
}
