use super::ModelError;
use super::pages::Pages;
use super::skeleton::read_skeleton;
use super::types::*;
use super::vertex::read_geometry;

const FRAGMENT_RECORD: usize = 0x110;
const MAX_MODELS: u16 = 256;
const MAX_PALETTE: u16 = 4096;
const MAX_MATERIALS: u16 = 512;
const MAX_GEOMETRIES: u16 = 1024;
const SHADER_MAGIC: u32 = 0x6D65_7461;

pub(crate) fn decode_set(pages: Pages<'_>, name: Option<&str>) -> Result<ModelSet, ModelError> {
    let kind = detect_kind(&pages)?;
    let mut warnings = Vec::new();
    let mut drawables = Vec::new();
    match kind {
        ResourceKind::Drawable => {
            drawables.push(read_drawable(
                &pages,
                0,
                name.map(str::to_owned),
                None,
                &mut warnings,
            )?);
        }
        ResourceKind::Dictionary => {
            let count = pages.u16(56, "dictionary count")? as usize;
            let children = pages.pointer(48, count * 8, "dictionary children")?;
            let hashes = pages.optional_pointer(32, "dictionary hashes")?;
            for i in 0..count {
                let child = pages.pointer(children + 8 * i, 192, "dictionary child")?;
                let hash = match hashes {
                    Some(at) => Some(pages.u32(at + 4 * i, "dictionary hash")?),
                    None => None,
                };
                drawables.push(read_drawable(&pages, child, None, hash, &mut warnings)?);
            }
        }
        ResourceKind::Fragment => {
            let main = pages.pointer(24, 192, "fragment drawable")?;
            let count = pages.u16(48, "fragment drawable count")? as usize;
            drawables.push(read_drawable(
                &pages,
                main,
                name.map(str::to_owned),
                None,
                &mut warnings,
            )?);
            for i in 1..count {
                let at = main + FRAGMENT_RECORD * i;
                let label = name.map(|n| format!("{n}#{i}"));
                match read_drawable(&pages, at, label, None, &mut warnings) {
                    Ok(d) => drawables.push(d),
                    Err(e) => warnings.push(format!("fragment child {i} skipped: {e}")),
                }
            }
        }
    }
    Ok(ModelSet {
        name: name.map(str::to_owned),
        kind,
        drawables,
        warnings,
    })
}

fn finite_bounds(p: &Pages<'_>, at: usize) -> bool {
    let (Ok(min), Ok(max)) = (p.f32s::<3>(at, "bounds"), p.f32s::<3>(at + 16, "bounds")) else {
        return false;
    };
    (0..3).all(|i| {
        min[i].is_finite()
            && max[i].is_finite()
            && min[i].abs() < 1e7
            && max[i].abs() < 1e7
            && min[i] <= max[i]
    })
}

fn detect_kind(p: &Pages<'_>) -> Result<ResourceKind, ModelError> {
    if p.system.len() < 192 || p.u64(0, "header")? != 0 {
        return Err(ModelError::NotAModel);
    }
    let ptr = |at: usize| p.u64(at, "header").map(Pages::is_system_pointer);
    let null_or_ptr = |at: usize| {
        p.u64(at, "header")
            .map(|v| v == 0 || Pages::is_system_pointer(v))
    };
    let u16s = |at: usize| p.u16(at, "header");
    if ptr(8)? && ptr(32)? && ptr(48)? {
        let n = u16s(56)?;
        if 0 < n && n <= 4096 && n == u16s(58)? && n == u16s(40)? && n == u16s(42)? {
            return Ok(ResourceKind::Dictionary);
        }
    }
    if ptr(8)?
        && ptr(24)?
        && ptr(168)?
        && null_or_ptr(16)?
        && null_or_ptr(32)?
        && null_or_ptr(40)?
        && (1..=4096).contains(&u16s(48)?)
        && u16s(50)? <= 4096
        && u16s(52)? <= u16s(54)?
        && u16s(54)? <= 4096
    {
        return Ok(ResourceKind::Fragment);
    }
    let radius = p.f32(44, "radius")?;
    if null_or_ptr(8)?
        && ptr(16)?
        && ptr(80)?
        && null_or_ptr(168)?
        && (0.0..1e6).contains(&radius)
        && finite_bounds(p, 48)
    {
        return Ok(ResourceKind::Drawable);
    }
    Err(ModelError::NotAModel)
}

fn read_drawable(
    p: &Pages<'_>,
    at: usize,
    name: Option<String>,
    name_hash: Option<u32>,
    warnings: &mut Vec<String>,
) -> Result<Drawable, ModelError> {
    if p.u64(at, "drawable")? != 0 {
        return Err(ModelError::Invalid(
            "drawable header does not start with zero".into(),
        ));
    }
    let radius = p.f32(at + 44, "drawable radius")?;
    if !(0.0..1e6).contains(&radius) || !finite_bounds(p, at + 48) {
        return Err(ModelError::Invalid("drawable bounds".into()));
    }
    let bounds = Bounds {
        min: p.f32s(at + 48, "drawable bounds")?,
        max: p.f32s(at + 64, "drawable bounds")?,
        radius,
    };
    let (materials, embedded_textures) = match p.optional_pointer(at + 16, "shader group")? {
        Some(group) => read_shader_group(p, group, warnings)?,
        None => (Vec::new(), 0),
    };
    let skeleton = match p.optional_pointer(at + 24, "skeleton")? {
        Some(header) => Some(read_skeleton(p, header)?),
        None => None,
    };
    let name = p.c_string(at + 168).or(name);

    let mut lods: Vec<Lod> = Vec::new();
    let mut seen = Vec::new();
    for level in 0..4 {
        let Some(list) = p.optional_pointer(at + 80 + 8 * level, "LOD list")? else {
            continue;
        };
        if seen.contains(&list) {
            continue;
        }
        seen.push(list);
        let distance = p.f32(at + 0x70 + 4 * level, "LOD distance")?;
        let models = read_models(p, list, materials.len(), warnings)?;
        lods.push(Lod {
            level,
            distance,
            models,
        });
    }
    if lods.is_empty() {
        return Err(ModelError::Invalid("drawable has no LOD".into()));
    }
    Ok(Drawable {
        name,
        name_hash,
        bounds,
        lods,
        materials,
        skeleton,
        embedded_textures,
    })
}

fn read_models(
    p: &Pages<'_>,
    list: usize,
    material_count: usize,
    warnings: &mut Vec<String>,
) -> Result<Vec<Model>, ModelError> {
    let count = p.u16(list + 8, "LOD model count")?;
    if count == 0 || count > MAX_MODELS || count != p.u16(list + 10, "LOD model capacity")? {
        return Err(ModelError::Invalid(format!("LOD model count {count}")));
    }
    let array = p.pointer(list, count as usize * 8, "model list")?;
    let mut models = Vec::with_capacity(count as usize);
    for mi in 0..count as usize {
        let model = p.pointer(array + 8 * mi, 56, "model")?;
        let geometries = p.u16(model + 16, "geometry count")?;
        if geometries == 0
            || geometries > MAX_GEOMETRIES
            || geometries != p.u16(model + 18, "geometry count")?
            || geometries != p.u16(model + 54, "geometry count")?
        {
            return Err(ModelError::Invalid(format!(
                "model geometry count {geometries}"
            )));
        }
        let bones = p.u16(model + 20, "bone count")?;
        if bones > MAX_PALETTE {
            return Err(ModelError::Invalid(format!("bone palette of {bones}")));
        }
        let mut bone_palette = Vec::with_capacity(bones as usize);
        if bones > 0 {
            let at = p.pointer(model + 0x28, bones as usize * 4, "bone palette")?;
            for i in 0..bones as usize {
                bone_palette.push(p.u32(at + 4 * i, "bone palette")?);
            }
        }
        let mapping = p.optional_pointer(model + 0x20, "shader mapping")?;
        let geometry_list = p.pointer(model + 8, geometries as usize * 8, "geometry list")?;
        let mut out = Vec::with_capacity(geometries as usize);
        for gi in 0..geometries as usize {
            let geometry = p.pointer(geometry_list + 8 * gi, 160, "geometry")?;
            let mut g = read_geometry(p, geometry, &bone_palette, warnings)?;
            g.material = match mapping {
                Some(m) => p.u16(m + 2 * gi, "shader mapping")? as usize,
                None => 0,
            };
            if g.material >= material_count.max(1) {
                warnings.push(format!(
                    "model {mi} geometry {gi} names shader {} but the group has {material_count}",
                    g.material
                ));
            }
            out.push(g);
        }
        models.push(Model {
            bone_palette,
            geometries: out,
        });
    }
    Ok(models)
}

fn read_shader_group(
    p: &Pages<'_>,
    group: usize,
    warnings: &mut Vec<String>,
) -> Result<(Vec<Material>, usize), ModelError> {
    p.slice(group, 64, "shader group")?;
    let embedded = match p.optional_pointer(group + 8, "embedded texture dictionary")? {
        Some(dict) => p.u16(dict + 56, "embedded texture count")? as usize,
        None => 0,
    };
    let count = p.u16(group + 24, "material count")?;
    if count > MAX_MATERIALS || count != p.u16(group + 26, "material capacity")? {
        return Err(ModelError::Invalid(format!("material count {count}")));
    }
    let mut materials = Vec::with_capacity(count as usize);
    if count > 0 {
        let array = p.pointer(group + 16, count as usize * 8, "material list")?;
        for i in 0..count as usize {
            let shader = p.pointer(array + 8 * i, 64, "shader")?;
            materials.push(read_material(p, shader, i, warnings)?);
        }
    }
    Ok((materials, embedded))
}

/// Shader record: +0 effect hash, +4 `meta` tag, +0x10 texture reference array, +0x20 parameter info
/// (u8 texture count at +1, u8 parameter count at +4, then eight bytes per parameter), +0x39 render
/// bucket, +0x3C bucket mask. A texture reference's +0x30 points at 16 bytes: name hash, parameter
/// hash, 64-bit asset id, behind a 16-byte header whose first word is 6. Which of those words is the
/// parameter hash is inferred from "parameter hash immediately before the asset key".
fn read_material(
    p: &Pages<'_>,
    shader: usize,
    index: usize,
    warnings: &mut Vec<String>,
) -> Result<Material, ModelError> {
    let mut material = Material {
        shader_hash: p.u32(shader, "shader hash")?,
        render_bucket: p.u8(shader + 0x39, "render bucket")?,
        render_bucket_mask: p.u32(shader + 0x3C, "render bucket mask")?,
        ..Material::default()
    };
    if p.u32(shader + 4, "shader tag")? != SHADER_MAGIC {
        warnings.push(format!(
            "material {index} has no meta tag; textures not read"
        ));
        return Ok(material);
    }
    let Some(info) = p.optional_pointer(shader + 32, "parameter info")? else {
        return Ok(material);
    };
    let textures = p.u8(info + 1, "texture count")? as usize;
    let params = p.u8(info + 4, "parameter count")? as usize;
    p.slice(info, 8 + params * 8, "parameter table")?;
    for i in 0..params {
        material.raw_parameters.push([
            p.u32(info + 8 + 8 * i, "parameter")?,
            p.u32(info + 12 + 8 * i, "parameter")?,
        ]);
    }
    if textures == 0 {
        return Ok(material);
    }
    let array = p.pointer(shader + 16, textures * 8, "texture references")?;
    for t in 0..textures {
        let Some(reference) = p.optional_pointer(array + 8 * t, "texture reference")? else {
            continue;
        };
        p.slice(reference, 80, "texture reference")?;
        let Some(asset) = p.optional_pointer(reference + 48, "texture asset")? else {
            continue;
        };
        if asset < 16 || p.u32(asset - 16, "texture asset tag")? != 6 {
            warnings.push(format!(
                "material {index} texture {t}: asset header is not 6"
            ));
            continue;
        }
        material.textures.push(MaterialTexture {
            name_hash: p.u32(asset, "texture name hash")?,
            param_hash: p.u32(asset + 4, "texture parameter hash")?,
            id64: p.u64(asset + 8, "texture id")?,
        });
    }
    Ok(material)
}
