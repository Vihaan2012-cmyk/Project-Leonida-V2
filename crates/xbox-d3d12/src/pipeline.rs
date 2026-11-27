//! Pipeline state objects built from the console shader bytecode through the translator.
//!
//! [`PipelineEnv`] holds what every pipeline is built from: the device, one pipeline layout, the persistent
//! cache, the shader source and the place shader code is uploaded to. Its `create_*` methods turn the pieces of
//! a `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, a compute description (the PC shape of slot 62 or the console shape
//! of slot 67) or a mesh description into a [`Pipeline`] that the replay binds.

pub mod cache;
pub mod shaders;
pub mod state;

use std::fmt;
use std::sync::Arc;

use ash::vk;
use sha2::{Digest, Sha256};
use shader_container::Stage;
use shader_translate::images::{self, NUMBERS, VIEWS};

use crate::execute::VkContext;
use crate::root::{RootArguments, RootError, RootSignature, StageLayout, UserData, UserStage};

pub use cache::PipelineCache;
pub use shaders::{DirectTranslation, Link, ShaderSource, TranslatedStage};
pub use state::{
    BlendState, DepthStencilState, InputElement, RasterizerState, RenderTargetBlend, StencilFace,
    TopologyType, dxgi_to_vk, format_bytes,
};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("the shader bytecode is not usable: {0}")]
    Bytecode(String),
    #[error("a {found} shader where a {wanted} shader is needed")]
    WrongStage { wanted: String, found: String },
    #[error("the shader could not be translated: {0}")]
    Translate(String),
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("the pipeline description is invalid: {0}")]
    Invalid(&'static str),
    #[error("this GPU lacks {0}")]
    NoFeature(&'static str),
    #[error("DXGI format {0} is not one a pipeline can use here")]
    Format(u32),
    #[error("{0}")]
    CodeBase(&'static str),
    #[error("root signature: {0}")]
    Root(#[from] RootError),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("pipeline cache: {0}")]
    Cache(String),
    #[error("shader code upload failed: {0}")]
    Upload(String),
}

/// Where shader programs live while pipelines use them: the arena, at console VAs.
pub trait CodeStore: Send + Sync {
    /// Copies `bytes` to the arena and returns the console VA of their first byte.
    fn upload(&self, bytes: &[u8]) -> Result<u64, PipelineError>;
}

/// Raw shader bytes whose debug form is a hash, so descriptions stay printable.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bytecode(pub Vec<u8>);

impl fmt::Debug for Bytecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hash = Sha256::digest(&self.0);
        write!(
            f,
            "Bytecode({} bytes, {:02x}{:02x}{:02x}{:02x})",
            self.0.len(),
            hash[0],
            hash[1],
            hash[2],
            hash[3]
        )
    }
}

impl Bytecode {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, or the same from a pipeline state stream.
#[derive(Debug, Clone)]
pub struct GraphicsPsoDesc {
    pub root: Arc<RootSignature>,
    pub vs: Bytecode,
    pub ps: Bytecode,
    pub gs: Bytecode,
    /// A mesh shader replaces the vertex and geometry shaders.
    pub ms: Bytecode,
    pub blend: BlendState,
    pub sample_mask: u32,
    pub raster: RasterizerState,
    pub depth_stencil: DepthStencilState,
    pub input_layout: Vec<InputElement>,
    /// 0 disabled, 1 0xFFFF, 2 0xFFFFFFFF.
    pub strip_cut: u32,
    pub topology: TopologyType,
    /// `DXGI_FORMAT` per render target; 0 for an unused one.
    pub render_targets: Vec<u32>,
    pub depth_format: u32,
    pub sample_count: u32,
}

impl GraphicsPsoDesc {
    pub fn new(root: Arc<RootSignature>) -> GraphicsPsoDesc {
        GraphicsPsoDesc {
            root,
            vs: Bytecode::default(),
            ps: Bytecode::default(),
            gs: Bytecode::default(),
            ms: Bytecode::default(),
            blend: BlendState::default(),
            sample_mask: u32::MAX,
            raster: RasterizerState::default(),
            depth_stencil: DepthStencilState::default(),
            input_layout: Vec::new(),
            strip_cut: 0,
            topology: 3,
            render_targets: Vec::new(),
            depth_format: 0,
            sample_count: 1,
        }
    }

    /// Equal for descriptions that build the same pipeline.
    pub fn key(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.root.fingerprint());
        hash.update(format!("{:?}", GraphicsKey(self)).as_bytes());
        hash.finalize().into()
    }
}

struct GraphicsKey<'a>(&'a GraphicsPsoDesc);

impl fmt::Debug for GraphicsKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.0;
        write!(
            f,
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:#x}|{:?}|{:?}|{}|{}|{:?}|{}|{}",
            d.vs,
            d.ps,
            d.gs,
            d.ms,
            d.blend,
            d.raster,
            d.sample_mask,
            d.depth_stencil,
            d.input_layout,
            d.strip_cut,
            d.topology,
            d.render_targets,
            d.depth_format,
            d.sample_count
        )
    }
}

/// A compute pipeline's bytecode: a DXBC container or FSSM blob (slot 62 and the game), or the four parts the
/// console's slot 67 passes.
#[derive(Debug, Clone)]
pub struct ComputePsoDesc {
    /// `None` when the shader carries its own signature; such a pipeline uses an empty one.
    pub root: Option<Arc<RootSignature>>,
    pub cs: Bytecode,
}

/// The console-only description at device slot 67: the root signature, then pointer and size pairs for an
/// FSPX payload's kinds 0, 1, 6 and 7.
#[derive(Debug, Clone, Default)]
pub struct Slot67Parts {
    pub dispatch_state: Vec<u8>,
    pub kind1: Vec<u8>,
    pub program: Vec<u8>,
    pub records: Vec<u8>,
}

impl ComputePsoDesc {
    /// Builds the one FSPX payload slot 67's four pairs describe. Kind 0 is required: the program alone does
    /// not say its group size or wave size.
    pub fn from_slot67(
        root: Option<Arc<RootSignature>>,
        parts: &Slot67Parts,
    ) -> Result<ComputePsoDesc, PipelineError> {
        if parts.program.is_empty() {
            return Err(PipelineError::Invalid("slot 67 without a program"));
        }
        if parts.dispatch_state.is_empty() {
            return Err(PipelineError::Unsupported(
                "slot 67 without dispatch state (a bare program has no group size)",
            ));
        }
        let kinds: Vec<(u32, &[u8])> = [
            (0, &parts.dispatch_state),
            (1, &parts.kind1),
            (6, &parts.program),
            (7, &parts.records),
        ]
        .into_iter()
        .filter(|(_, bytes)| !bytes.is_empty())
        .map(|(kind, bytes)| (kind, bytes.as_slice()))
        .collect();
        let directory = 16 + 16 * kinds.len();
        let mut body = Vec::new();
        let mut table = Vec::new();
        for (kind, bytes) in &kinds {
            for word in [
                *kind,
                (directory + body.len()) as u32,
                bytes.len() as u32,
                0,
            ] {
                table.extend_from_slice(&word.to_le_bytes());
            }
            body.extend_from_slice(bytes);
        }
        let mut payload = Vec::new();
        for word in [
            u32::from_le_bytes(*b"FSPX"),
            1,
            kinds.len() as u32,
            (directory + body.len()) as u32,
        ] {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.extend_from_slice(&table);
        payload.extend_from_slice(&body);
        Ok(ComputePsoDesc {
            root,
            cs: Bytecode(payload),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    Graphics,
    Mesh,
    Compute,
}

/// One stage of a built pipeline.
#[derive(Debug, Clone)]
pub struct StageInfo {
    pub user: UserStage,
    pub vk_stage: vk::ShaderStageFlags,
    /// The console VA of the stage's first instruction, where its program was uploaded.
    pub code_va: u64,
    pub translated: Arc<TranslatedStage>,
}

/// How one vertex buffer slot steps, from the input layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexSlot {
    pub slot: u32,
    pub per_instance: bool,
}

pub struct Pipeline {
    device: ash::Device,
    pub handle: vk::Pipeline,
    pub kind: PipelineKind,
    pub root: Arc<RootSignature>,
    pub stages: Vec<StageInfo>,
    pub layouts: Vec<StageLayout>,
    pub vertex_slots: Vec<VertexSlot>,
    pub topology: TopologyType,
    pub render_targets: Vec<vk::Format>,
    pub depth_format: vk::Format,
    pub sample_count: u32,
    pub has_geometry: bool,
    /// The thread-group size of a compute or mesh shader.
    pub threads: Option<[u32; 3]>,
    /// The console VA the pipeline's code base words carry: the program of the one stage that reads its own
    /// address.
    pub code_base: u64,
    pub key: [u8; 32],
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // SAFETY: the pipeline is destroyed once, by its last owner, after the GPU finished with it.
        unsafe { self.device.destroy_pipeline(self.handle, None) };
    }
}

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pipeline")
            .field("kind", &self.kind)
            .field("stages", &self.stages.len())
            .finish()
    }
}

impl Pipeline {
    /// The push-constant block of this pipeline's stages: the user data, then the heap base and the code base.
    pub fn push_words(
        &self,
        user: &UserData,
        heap_base: u64,
    ) -> [u32; shader_translate::PUSH_WORDS as usize] {
        let mut words = [0u32; shader_translate::PUSH_WORDS as usize];
        words[..32].copy_from_slice(&user.sgprs);
        let heap = shader_translate::PUSH_HEAP as usize;
        words[heap] = heap_base as u32;
        words[heap + 1] = (heap_base >> 32) as u32;
        let code = shader_translate::PUSH_CODE as usize;
        words[code] = self.code_base as u32;
        words[code + 1] = (self.code_base >> 32) as u32;
        words
    }

    pub fn bind_point(&self) -> vk::PipelineBindPoint {
        match self.kind {
            PipelineKind::Compute => vk::PipelineBindPoint::COMPUTE,
            _ => vk::PipelineBindPoint::GRAPHICS,
        }
    }

    pub fn stage_layouts(&self) -> Vec<&StageLayout> {
        self.layouts.iter().collect()
    }
}

/// Descriptor set 1's layout as the translator's image contract lays it out, with `count` entries in each array.
/// The device layer's own bindless set must have these bindings.
pub fn bindless_bindings(count: u32) -> Vec<vk::DescriptorSetLayoutBinding<'static>> {
    let all = vk::ShaderStageFlags::ALL;
    let one = |binding: u32, kind: vk::DescriptorType, count: u32| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(binding)
            .descriptor_type(kind)
            .descriptor_count(count)
            .stage_flags(all)
    };
    let mut list = vec![
        one(images::TEXTURE_TABLE, vk::DescriptorType::STORAGE_BUFFER, 1),
        one(images::SAMPLER_TABLE, vk::DescriptorType::STORAGE_BUFFER, 1),
        one(images::SAMPLERS, vk::DescriptorType::SAMPLER, count),
        one(images::COMPARE_SAMPLERS, vk::DescriptorType::SAMPLER, count),
    ];
    for view in VIEWS {
        for number in NUMBERS {
            list.push(one(
                images::binding(view, number),
                vk::DescriptorType::SAMPLED_IMAGE,
                count,
            ));
            list.push(one(
                images::storage_binding(view, number),
                vk::DescriptorType::STORAGE_IMAGE,
                count,
            ));
        }
        for wide in [false, true] {
            list.push(one(
                images::atomic_binding(view, wide),
                vk::DescriptorType::STORAGE_IMAGE,
                count,
            ));
        }
    }
    list
}

/// What every pipeline of one device is built from.
pub struct PipelineEnv {
    pub ctx: Arc<VkContext>,
    /// Set 0 is empty and set 1 is the bindless image set; push constants cover every stage.
    pub layout: vk::PipelineLayout,
    empty_set: vk::DescriptorSetLayout,
    pub cache: PipelineCache,
    pub source: Arc<dyn ShaderSource>,
    pub code: Arc<dyn CodeStore>,
    /// Told about anything approximate or unmeasured a pipeline needed.
    pub note: Arc<dyn Fn(&str) + Send + Sync>,
}

impl PipelineEnv {
    /// `bindless` is the layout of the device's image descriptor set (see [`bindless_bindings`]).
    pub fn new(
        ctx: Arc<VkContext>,
        bindless: vk::DescriptorSetLayout,
        cache: PipelineCache,
        source: Arc<dyn ShaderSource>,
        code: Arc<dyn CodeStore>,
    ) -> Result<PipelineEnv, PipelineError> {
        // SAFETY: plain create calls on a live device; the layouts are destroyed in Drop.
        let (empty_set, layout) = unsafe {
            let empty_set = ctx.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default(),
                None,
            )?;
            let sets = [empty_set, bindless];
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::ALL)
                .size(shader_translate::PUSH_WORDS * 4)];
            let layout = ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sets)
                    .push_constant_ranges(&ranges),
                None,
            )?;
            (empty_set, layout)
        };
        Ok(PipelineEnv {
            ctx,
            layout,
            empty_set,
            cache,
            source,
            code,
            note: Arc::new(|_| {}),
        })
    }

    fn note(&self, text: &str) {
        (self.note)(text);
    }

    fn upload(&self, stage: &TranslatedStage) -> Result<u64, PipelineError> {
        Ok(self.code.upload(&stage.code)? + u64::from(stage.code_start))
    }

    fn stage_info(
        &self,
        translated: Arc<TranslatedStage>,
        user: UserStage,
        vk_stage: vk::ShaderStageFlags,
    ) -> Result<StageInfo, PipelineError> {
        Ok(StageInfo {
            user,
            vk_stage,
            code_va: self.upload(&translated)?,
            translated,
        })
    }

    fn module(&self, spirv: &[u32]) -> Result<vk::ShaderModule, PipelineError> {
        // SAFETY: `spirv` is a whole translated module.
        Ok(unsafe {
            self.ctx
                .device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None)?
        })
    }

    /// Chooses the one code base the block carries, or says two stages need different ones. The translator
    /// reads a single code base for every stage of a pipeline.
    fn code_base(&self, stages: &[StageInfo]) -> Result<u64, PipelineError> {
        let mut using = stages.iter().filter(|s| s.translated.uses_code_base);
        let Some(first) = using.next() else {
            return Ok(0);
        };
        if using.any(|s| s.translated.hash != first.translated.hash) {
            self.note("two stages of one pipeline read constants through their own address");
            return Err(PipelineError::CodeBase(
                "two stages read constants through their own address, and the push block has one code base",
            ));
        }
        Ok(first.code_va)
    }

    pub fn create_graphics(&self, desc: &GraphicsPsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        let features = &self.ctx.features;
        let mesh = !desc.ms.is_empty();
        if mesh && !(desc.vs.is_empty() && desc.gs.is_empty()) {
            return Err(PipelineError::Invalid(
                "a mesh shader beside a vertex or geometry shader",
            ));
        }
        if !mesh && desc.vs.is_empty() {
            return Err(PipelineError::Invalid("no vertex or mesh shader"));
        }
        if !features.pixel_shaders {
            return Err(PipelineError::NoFeature(
                "dynamic rendering and fragment barycentrics",
            ));
        }
        if mesh && !features.mesh_shaders {
            return Err(PipelineError::NoFeature("VK_EXT_mesh_shader"));
        }
        if !desc.gs.is_empty() && !features.geometry_shaders {
            return Err(PipelineError::NoFeature("geometry shaders"));
        }
        let has_geometry = !desc.gs.is_empty();
        let source = &self.source;
        let mut stages = Vec::new();
        let first = if mesh {
            let ms = source.translate(&desc.ms.0, Stage::Mesh, &Link::default())?;
            stages.push(self.stage_info(
                ms.clone(),
                UserStage::Mesh,
                vk::ShaderStageFlags::MESH_EXT,
            )?);
            ms
        } else {
            let link = Link {
                previous: None,
                first_half: has_geometry,
            };
            let vs = source.translate(&desc.vs.0, Stage::Vertex, &link)?;
            stages.push(self.stage_info(
                vs.clone(),
                UserStage::Vertex,
                vk::ShaderStageFlags::VERTEX,
            )?);
            vs
        };
        let mut previous = first.clone();
        if has_geometry {
            let gs = source.translate(&desc.gs.0, Stage::Geometry, &Link::default())?;
            stages.push(self.stage_info(
                gs.clone(),
                UserStage::Geometry,
                vk::ShaderStageFlags::GEOMETRY,
            )?);
            previous = gs;
        }
        if !desc.ps.is_empty() {
            let link = Link {
                previous: Some(previous),
                first_half: false,
            };
            let ps = source.translate(&desc.ps.0, Stage::Pixel, &link)?;
            stages.push(self.stage_info(ps, UserStage::Pixel, vk::ShaderStageFlags::FRAGMENT)?);
        }
        let layouts = stages
            .iter()
            .map(|s| desc.root.layout(s.user))
            .collect::<Result<Vec<_>, _>>()?;
        crate::root::pack(
            &layouts.iter().collect::<Vec<_>>(),
            &RootArguments::new(&desc.root),
            Default::default(),
        )?;
        let code_base = self.code_base(&stages)?;
        for stage in &stages {
            if !stage.user.base_measured() {
                self.note(&format!(
                    "{:?} user data starts at s{}, which comes from a census of shader reads and no signature",
                    stage.user,
                    stage.user.base()
                ));
            }
        }
        if desc.root.console_flags() != 0 {
            self.note("the root signature carries console flag bits whose meaning is not measured");
        }
        let formats = desc
            .render_targets
            .iter()
            .map(|&f| {
                if f == 0 {
                    Ok(vk::Format::UNDEFINED)
                } else {
                    dxgi_to_vk(f).ok_or(PipelineError::Format(f))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let depth_format = match desc.depth_format {
            0 => vk::Format::UNDEFINED,
            f => dxgi_to_vk(f).ok_or(PipelineError::Format(f))?,
        };
        let (attributes, bindings, vertex_slots) = if mesh {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            self.vertex_input(desc, &first)
        };
        let handle = self.build_graphics(
            desc,
            &stages,
            mesh,
            &formats,
            depth_format,
            &attributes,
            &bindings,
        )?;
        let threads = stages.iter().find_map(|s| {
            (s.user == UserStage::Mesh)
                .then_some(s.translated.threads)
                .flatten()
        });
        Ok(Arc::new(Pipeline {
            device: self.ctx.device.clone(),
            handle,
            kind: if mesh {
                PipelineKind::Mesh
            } else {
                PipelineKind::Graphics
            },
            root: desc.root.clone(),
            stages,
            layouts,
            vertex_slots,
            topology: desc.topology,
            render_targets: formats,
            depth_format,
            sample_count: desc.sample_count.max(1),
            has_geometry,
            threads,
            code_base,
            key: desc.key(),
        }))
    }

    /// Vertex attributes for the shader's fetched registers, found by semantic in the input layout.
    fn vertex_input(
        &self,
        desc: &GraphicsPsoDesc,
        vertex: &TranslatedStage,
    ) -> (
        Vec<vk::VertexInputAttributeDescription>,
        Vec<vk::VertexInputBindingDescription>,
        Vec<VertexSlot>,
    ) {
        let mut next_offset = std::collections::HashMap::<u32, u32>::new();
        let mut resolved = Vec::new();
        for element in &desc.input_layout {
            let offset = if element.offset == 0xFFFF_FFFF {
                *next_offset.get(&element.slot).unwrap_or(&0)
            } else {
                element.offset
            };
            let size = dxgi_to_vk(element.format).map_or(16, format_bytes);
            next_offset.insert(element.slot, offset + size);
            resolved.push((element, offset));
        }
        let mut attributes = Vec::new();
        let mut slots: Vec<VertexSlot> = Vec::new();
        let signature = vertex
            .interface
            .as_ref()
            .map(|i| i.inputs.as_slice())
            .unwrap_or_default();
        for input in &vertex.vertex_inputs {
            let found = signature
                .iter()
                .find(|e| e.register == u32::from(input.register))
                .and_then(|e| {
                    resolved.iter().find(|(layout, _)| {
                        layout.semantic.eq_ignore_ascii_case(&e.semantic)
                            && layout.semantic_index == e.semantic_index
                    })
                });
            let Some((element, offset)) = found else {
                self.note(&format!(
                    "vertex input register {} has no input layout element",
                    input.register
                ));
                continue;
            };
            let Some(format) = dxgi_to_vk(element.format) else {
                self.note(&format!(
                    "input layout format {} is not mapped",
                    element.format
                ));
                continue;
            };
            if element.class == 1 && element.step_rate > 1 {
                self.note("an instance step rate above 1 is treated as 1");
            }
            attributes.push(
                vk::VertexInputAttributeDescription::default()
                    .location(u32::from(input.register))
                    .binding(element.slot)
                    .format(format)
                    .offset(*offset),
            );
            if !slots.iter().any(|s| s.slot == element.slot) {
                slots.push(VertexSlot {
                    slot: element.slot,
                    per_instance: element.class == 1,
                });
            }
        }
        let bindings = slots
            .iter()
            .map(|s| {
                vk::VertexInputBindingDescription::default()
                    .binding(s.slot)
                    .stride(0)
                    .input_rate(if s.per_instance {
                        vk::VertexInputRate::INSTANCE
                    } else {
                        vk::VertexInputRate::VERTEX
                    })
            })
            .collect();
        (attributes, bindings, slots)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_graphics(
        &self,
        desc: &GraphicsPsoDesc,
        stages: &[StageInfo],
        mesh: bool,
        formats: &[vk::Format],
        depth_format: vk::Format,
        attributes: &[vk::VertexInputAttributeDescription],
        bindings: &[vk::VertexInputBindingDescription],
    ) -> Result<vk::Pipeline, PipelineError> {
        let features = &self.ctx.features;
        let mut modules = Vec::new();
        let result = (|| {
            let mut infos = Vec::new();
            for stage in stages {
                let module = self.module(&stage.translated.spirv)?;
                modules.push(module);
                infos.push(
                    vk::PipelineShaderStageCreateInfo::default()
                        .stage(stage.vk_stage)
                        .module(module)
                        .name(c"main"),
                );
            }
            let input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(bindings)
                .vertex_attribute_descriptions(attributes);
            let topology = state::default_topology(desc.topology, !desc.gs.is_empty());
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(topology)
                .primitive_restart_enable(false);
            let viewport = vk::PipelineViewportStateCreateInfo::default();
            let r = &desc.raster;
            let line = r.fill == 2 && features.fill_mode_non_solid;
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .depth_clamp_enable(!r.depth_clip && features.depth_clamp)
                .polygon_mode(if line {
                    vk::PolygonMode::LINE
                } else {
                    vk::PolygonMode::FILL
                })
                .cull_mode(match r.cull {
                    2 => vk::CullModeFlags::FRONT,
                    3 => vk::CullModeFlags::BACK,
                    _ => vk::CullModeFlags::NONE,
                })
                .front_face(if r.front_ccw {
                    vk::FrontFace::COUNTER_CLOCKWISE
                } else {
                    vk::FrontFace::CLOCKWISE
                })
                .depth_bias_enable(r.depth_bias != 0 || r.slope_scaled_bias != 0.0)
                .depth_bias_constant_factor(r.depth_bias as f32)
                .depth_bias_clamp(r.depth_bias_clamp)
                .depth_bias_slope_factor(r.slope_scaled_bias)
                .line_width(1.0);
            let samples = sample_flags(desc.sample_count);
            let mask = [desc.sample_mask];
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(samples)
                .sample_mask(&mask)
                .alpha_to_coverage_enable(desc.blend.alpha_to_coverage);
            let ds = &desc.depth_stencil;
            let face = |f: &StencilFace| {
                vk::StencilOpState::default()
                    .fail_op(state::vk_stencil_op(f.fail))
                    .depth_fail_op(state::vk_stencil_op(f.depth_fail))
                    .pass_op(state::vk_stencil_op(f.pass))
                    .compare_op(state::vk_compare(f.func))
                    .compare_mask(u32::from(ds.stencil_read_mask))
                    .write_mask(u32::from(ds.stencil_write_mask))
            };
            let depth_state = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(ds.depth_enable)
                .depth_write_enable(ds.depth_write)
                .depth_compare_op(state::vk_compare(ds.depth_func))
                .stencil_test_enable(ds.stencil_enable)
                .front(face(&ds.front))
                .back(face(&ds.back));
            let blends: Vec<_> = formats
                .iter()
                .enumerate()
                .map(|(i, format)| {
                    let t = &desc.blend.targets[if desc.blend.independent { i.min(7) } else { 0 }];
                    vk::PipelineColorBlendAttachmentState::default()
                        .blend_enable(t.blend_enable)
                        .src_color_blend_factor(state::vk_blend_factor(t.src))
                        .dst_color_blend_factor(state::vk_blend_factor(t.dst))
                        .color_blend_op(state::vk_blend_op(t.op))
                        .src_alpha_blend_factor(state::vk_blend_factor(t.src_alpha))
                        .dst_alpha_blend_factor(state::vk_blend_factor(t.dst_alpha))
                        .alpha_blend_op(state::vk_blend_op(t.op_alpha))
                        .color_write_mask(if *format == vk::Format::UNDEFINED {
                            vk::ColorComponentFlags::empty()
                        } else {
                            vk::ColorComponentFlags::from_raw(u32::from(t.write_mask))
                        })
                })
                .collect();
            let t0 = &desc.blend.targets[0];
            let logic = t0.logic_enable && features.logic_op;
            if t0.logic_enable && !features.logic_op {
                self.note("a logic-op blend on a GPU without logic ops is drawn without it");
            }
            let blend = vk::PipelineColorBlendStateCreateInfo::default()
                .logic_op_enable(logic)
                .logic_op(state::vk_logic_op(t0.logic_op))
                .attachments(&blends);
            let mut dynamic = vec![
                vk::DynamicState::VIEWPORT_WITH_COUNT,
                vk::DynamicState::SCISSOR_WITH_COUNT,
                vk::DynamicState::BLEND_CONSTANTS,
                vk::DynamicState::STENCIL_REFERENCE,
            ];
            if !mesh {
                dynamic.push(vk::DynamicState::PRIMITIVE_TOPOLOGY);
                if !bindings.is_empty() {
                    dynamic.push(vk::DynamicState::VERTEX_INPUT_BINDING_STRIDE);
                }
            }
            let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic);
            let mut rendering = vk::PipelineRenderingCreateInfo::default()
                .color_attachment_formats(formats)
                .depth_attachment_format(if state::is_depth_format(depth_format) {
                    depth_format
                } else {
                    vk::Format::UNDEFINED
                })
                .stencil_attachment_format(if state::has_stencil(depth_format) {
                    depth_format
                } else {
                    vk::Format::UNDEFINED
                });
            let mut create = vk::GraphicsPipelineCreateInfo::default()
                .stages(&infos)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
                .depth_stencil_state(&depth_state)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(self.layout)
                .push_next(&mut rendering);
            if !mesh {
                create = create
                    .vertex_input_state(&input)
                    .input_assembly_state(&assembly);
            }
            // SAFETY: every borrowed state lives until the call returns.
            unsafe {
                self.ctx
                    .device
                    .create_graphics_pipelines(self.cache.handle(), &[create], None)
                    .map(|p| p[0])
                    .map_err(|(_, e)| PipelineError::Vulkan(e))
            }
        })();
        for module in modules {
            // SAFETY: the pipeline holds what it needs of a module after creation.
            unsafe { self.ctx.device.destroy_shader_module(module, None) };
        }
        result
    }

    pub fn create_compute(&self, desc: &ComputePsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        let translated = self
            .source
            .translate(&desc.cs.0, Stage::Compute, &Link::default())?;
        let root = desc.root.clone().unwrap_or_else(|| {
            Arc::new(RootSignature {
                version: crate::root::RootVersion::V1_1,
                parameters: Vec::new(),
                static_samplers: Vec::new(),
                flags: 0,
            })
        });
        let layout = root.layout(UserStage::Compute)?;
        let stage = self.stage_info(
            translated.clone(),
            UserStage::Compute,
            vk::ShaderStageFlags::COMPUTE,
        )?;
        if layout.end != translated.user_sgprs {
            self.note(&format!(
                "the root signature's user data ends at s{} and the shader's header counts {} user SGPRs",
                layout.end, translated.user_sgprs
            ));
        }
        let module = self.module(&translated.spirv)?;
        let features = &self.ctx.features;
        let mut required = vk::PipelineShaderStageRequiredSubgroupSizeCreateInfo::default()
            .required_subgroup_size(32);
        let mut info = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"main");
        if features.subgroup_size_control
            && (features.min_subgroup_size..=features.max_subgroup_size).contains(&32)
        {
            info = info.push_next(&mut required);
            if features.full_subgroups {
                info = info.flags(vk::PipelineShaderStageCreateFlags::REQUIRE_FULL_SUBGROUPS);
            }
        } else if self.ctx.properties.limits.max_compute_shared_memory_size > 0 {
            self.note("this GPU cannot be told to run compute shaders 32 wide");
        }
        let create = vk::ComputePipelineCreateInfo::default()
            .stage(info)
            .layout(self.layout);
        // SAFETY: the create info borrows live data for the call.
        let built = unsafe {
            self.ctx
                .device
                .create_compute_pipelines(self.cache.handle(), &[create], None)
                .map(|p| p[0])
                .map_err(|(_, e)| PipelineError::Vulkan(e))
        };
        // SAFETY: the pipeline keeps what it needs of the module.
        unsafe { self.ctx.device.destroy_shader_module(module, None) };
        let handle = built?;
        let mut hash = Sha256::new();
        hash.update(root.fingerprint());
        hash.update(translated.hash);
        let code_base = if translated.uses_code_base {
            stage.code_va
        } else {
            0
        };
        Ok(Arc::new(Pipeline {
            device: self.ctx.device.clone(),
            handle,
            kind: PipelineKind::Compute,
            root,
            threads: translated.threads,
            stages: vec![stage],
            layouts: vec![layout],
            vertex_slots: Vec::new(),
            topology: 0,
            render_targets: Vec::new(),
            depth_format: vk::Format::UNDEFINED,
            sample_count: 1,
            has_geometry: false,
            code_base,
            key: hash.finalize().into(),
        }))
    }

    /// The blob `GetCachedBlob` returns for a pipeline.
    pub fn cached_blob(&self, pipeline: &Pipeline) -> Vec<u8> {
        self.cache.cached_blob(&pipeline.key)
    }
}

impl Drop for PipelineEnv {
    fn drop(&mut self) {
        // SAFETY: nothing builds pipelines from the layouts once the environment is gone.
        unsafe {
            self.ctx.device.destroy_pipeline_layout(self.layout, None);
            self.ctx
                .device
                .destroy_descriptor_set_layout(self.empty_set, None);
        }
    }
}

fn sample_flags(count: u32) -> vk::SampleCountFlags {
    match count {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}
