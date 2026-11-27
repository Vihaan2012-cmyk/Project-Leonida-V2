//! One shader container's programs as the Vulkan shaders the runtime will use, with the options each needs.

use shader_container::{Fssm, Interface, SignatureElement, Stage};
use shader_ir::Function;
use shader_translate::{EsOptions, GsOptions, PixelOptions, VertexOptions};

/// A translated program and what its pipeline needs.
pub enum Built {
    Compute(Vec<u32>),
    Pixel(Vec<u32>, PixelOptions),
    Vertex(Vec<u32>, VertexOptions),
    Mesh(Vec<u32>),
    Geometry(Vec<u32>, GsOptions),
    FirstHalf(Vec<u32>, EsOptions),
}

/// Translates a container's main program for its stage.
pub fn build(
    stage: Option<Stage>,
    fssm: &Fssm,
    found: Option<&Interface>,
    function: &Function,
) -> Result<Built, String> {
    let text = |e: shader_translate::EmitError| e.to_string();
    Ok(match stage {
        Some(Stage::Pixel) => {
            let options = pixel_options(fssm, found).ok_or("no pixel setup in the header")?;
            Built::Pixel(
                shader_translate::pixel(function, &options).map_err(text)?,
                options,
            )
        }
        Some(Stage::Vertex) => {
            let options = vertex_options(fssm, found);
            Built::Vertex(
                shader_translate::vertex(function, &options).map_err(text)?,
                options,
            )
        }
        Some(Stage::Geometry) => {
            let options = geometry_options(fssm, found)?;
            Built::Geometry(
                shader_translate::gs(function, &options).map_err(text)?,
                options,
            )
        }
        Some(Stage::Mesh) => {
            let options = mesh_options(fssm, found)?;
            Built::Mesh(shader_translate::mesh(function, &options).map_err(text)?)
        }
        Some(Stage::Compute) => {
            let (rsrc2, group) = fssm
                .compute_setup()
                .ok_or("no compute setup in the header")?;
            let options = shader_translate::ComputeOptions::from_rsrc2(
                rsrc2,
                group,
                shader_translate::FloatMode::default(),
            );
            Built::Compute(shader_translate::compute(function, &options).map_err(text)?)
        }
        _ => return Err("a stage the game does not use".into()),
    })
}

/// Translates the first half of a merged vertex-and-geometry shader, a vertex shader's extra program.
pub fn build_first_half(
    fssm: &Fssm,
    found: Option<&Interface>,
    function: &Function,
) -> Result<Built, String> {
    let options = first_half_options(fssm, found);
    let spirv = shader_translate::es(function, &options).map_err(|e| e.to_string())?;
    Ok(Built::FirstHalf(spirv, options))
}

/// The fragment entry's inputs from the header and its colour targets from the output signature.
pub fn pixel_options(fssm: &Fssm, found: Option<&Interface>) -> Option<PixelOptions> {
    let setup = fssm.pixel_setup()?;
    let outputs = found
        .map(|f| f.outputs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|element| element.system_value == 64 && element.register < 8)
        .map(|element| shader_translate::PixelOutput {
            target: element.register as u8,
            number: number(element),
        })
        .collect();
    Some(PixelOptions {
        user_sgprs: setup.user_sgprs,
        inputs: setup.inputs,
        outputs,
        depth: shader_translate::DepthOutput::from_control(setup.depth_control),
        attribute_locations: Vec::new(),
        float: shader_translate::FloatMode::default(),
    })
}

fn number(element: &SignatureElement) -> shader_translate::images::Number {
    use shader_translate::images::Number;
    match element.component_type {
        1 => Number::Uint,
        2 => Number::Sint,
        _ => Number::Float,
    }
}

/// The vertex entry's fetched attributes: every input-signature element that is not a system value.
fn vertex_options(fssm: &Fssm, found: Option<&Interface>) -> VertexOptions {
    let inputs = found
        .map(|f| f.inputs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|element| element.system_value == 0 && element.register < 16)
        .map(|element| shader_translate::VertexInput {
            register: element.register as u8,
            number: number(element),
        })
        .collect();
    VertexOptions {
        user_sgprs: fssm.user_sgprs(),
        inputs,
        float: shader_translate::FloatMode::default(),
    }
}

fn signature_bytes(elements: &[SignatureElement]) -> u32 {
    elements
        .iter()
        .map(|element| element.register + 1)
        .max()
        .unwrap_or(0)
        * 16
}

/// The first half's fetched attributes as for a vertex shader, and one LDS item of 16 bytes per output register.
fn first_half_options(fssm: &Fssm, found: Option<&Interface>) -> EsOptions {
    let vertex = vertex_options(fssm, found);
    EsOptions {
        user_sgprs: vertex.user_sgprs,
        inputs: vertex.inputs,
        item_bytes: signature_bytes(found.map(|f| f.outputs.as_slice()).unwrap_or_default()),
        float: vertex.float,
    }
}

/// The geometry entry's primitives from the pipeline-state part and its vertex sizes from the signatures.
fn geometry_options(fssm: &Fssm, found: Option<&Interface>) -> Result<GsOptions, String> {
    use shader_translate::{GsInput, GsOutput};
    let found = found.ok_or("no geometry signatures")?;
    let info = found
        .pipeline
        .as_ref()
        .and_then(|p| p.geometry)
        .ok_or("no geometry pipeline state")?;
    let input = match info.input_primitive {
        1 => GsInput::Points,
        2 => GsInput::Lines,
        3 => GsInput::Triangles,
        6 => GsInput::LinesAdjacency,
        7 => GsInput::TrianglesAdjacency,
        _ => return Err("geometry shader input that is not points, lines or triangles".into()),
    };
    let output = match info.output_topology {
        1 => GsOutput::Points,
        3 => GsOutput::LineStrip,
        5 => GsOutput::TriangleStrip,
        _ => return Err("geometry shader output that is not points, lines or triangles".into()),
    };
    if info.stream_mask > 1 {
        return Err("geometry output to a stream past the first".into());
    }
    Ok(GsOptions {
        user_sgprs: fssm.user_sgprs(),
        input,
        item_bytes: signature_bytes(&found.inputs),
        output,
        max_vertices: u32::from(info.max_vertices),
        vertex_bytes: signature_bytes(&found.outputs),
        position: found
            .outputs
            .iter()
            .find(|element| element.system_value == 1)
            .map(|element| element.register as u8),
        float: shader_translate::FloatMode::default(),
    })
}

/// The mesh entry's group size, output counts and shared memory from the pipeline-state part.
fn mesh_options(
    fssm: &Fssm,
    found: Option<&Interface>,
) -> Result<shader_translate::MeshOptions, String> {
    let pipeline = found
        .and_then(|f| f.pipeline.as_ref())
        .ok_or("no mesh pipeline state")?;
    let (Some([x, y, z]), Some(mesh)) = (pipeline.threads, pipeline.mesh) else {
        return Err("no mesh pipeline state".into());
    };
    if mesh.topology != 2 {
        return Err("mesh shader drawing lines".into());
    }
    Ok(shader_translate::MeshOptions {
        user_sgprs: fssm.user_sgprs(),
        group_size: x * y * z,
        max_vertices: u32::from(mesh.max_vertices),
        max_primitives: u32::from(mesh.max_primitives),
        lds_bytes: mesh.shared_bytes,
        float: shader_translate::FloatMode::default(),
    })
}
