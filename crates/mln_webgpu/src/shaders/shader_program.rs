// wgpu vs. Dawn: shader module creation runs inside a validation error scope
// so that a WGSL compile error yields "no module" (the C++ null check); the
// C++ backend gets null only on catastrophic failure and Dawn reports the
// compile error asynchronously. On a browser's WebGPU the scope is skipped
// (see `validated`).

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::color_mode::{BlendFunction, ColorMode};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::DrawModeType;
use mln_gfx::shader::Shader;
use mln_gfx::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    DepthMaskType, StencilFunctionType, StencilOpType,
};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_util::identity::SimpleIdentity;
use mln_vec_map::VecMap;

use crate::context::Context;
use crate::shaders::shader_source::{AttributeInfo, TextureInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingType {
    UniformBuffer,
    ReadOnlyStorageBuffer,
    StorageBuffer,
    Sampler,
    Texture,
    /// For `RGBA32Float` textures that require textureLoad instead of textureSample
    UnfilterableTexture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BindingInfo {
    pub(crate) group: u32,
    pub(crate) binding: u32,
    pub(crate) type_: BindingType,
    pub(crate) visibility: wgpu::ShaderStages,
}

/// Runs `create` inside a validation error scope and returns the error the
/// scope caught, which stands in for the C++ null result. On a browser's
/// WebGPU the scope is skipped: popping it waits for the browser, which in
/// the middle of a frame expires the canvas texture. Errors there reach the
/// device's uncaptured-error callback instead.
fn validated<T>(device: &wgpu::Device, create: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    if device.adapter_info().backend == wgpu::Backend::BrowserWebGpu {
        return Ok(create());
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = create();
    match pollster::block_on(scope.pop()) {
        None => Ok(value),
        Some(error) => Err(error),
    }
}

/// `source` for wgpu: natively, the module naga parses it to. Parsing the
/// WGSL is most of what creating a shader costs, and every renderer creates
/// the same shaders, so the process parses each source once. A source that
/// doesn't parse goes as WGSL, for wgpu to report its error.
#[cfg(not(target_family = "wasm"))]
fn shader_source(source: &str) -> wgpu::ShaderSource<'_> {
    use parking_lot::Mutex;

    static PARSED: Mutex<Option<HashMap<String, wgpu::naga::Module>>> = Mutex::new(None);
    let cached = PARSED.lock().get_or_insert_default().get(source).cloned();
    let module = if let Some(module) = cached {
        module
    } else {
        let Ok(module) = wgpu::naga::front::wgsl::parse_str(source) else {
            return wgpu::ShaderSource::Wgsl(Cow::Borrowed(source));
        };
        PARSED.lock().get_or_insert_default().insert(source.to_owned(), module.clone());
        module
    };
    wgpu::ShaderSource::Naga(Cow::Owned(module))
}

#[cfg(target_family = "wasm")]
fn shader_source(source: &str) -> wgpu::ShaderSource<'_> {
    wgpu::ShaderSource::Wgsl(Cow::Borrowed(source))
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn trim_string(value: &str) -> String {
    value.trim_matches(|c: char| c.is_ascii() && is_space(c as u8)).to_owned()
}

fn webgpu_blend_operation(equation: ColorBlendEquationType) -> wgpu::BlendOperation {
    match equation {
        ColorBlendEquationType::Add => wgpu::BlendOperation::Add,
        ColorBlendEquationType::Subtract => wgpu::BlendOperation::Subtract,
        ColorBlendEquationType::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
    }
}

fn webgpu_blend_factor(factor: ColorBlendFactorType) -> wgpu::BlendFactor {
    use ColorBlendFactorType as F;
    match factor {
        F::Zero => wgpu::BlendFactor::Zero,
        F::One => wgpu::BlendFactor::One,
        F::SrcColor => wgpu::BlendFactor::Src,
        F::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        F::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        F::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        F::DstAlpha => wgpu::BlendFactor::DstAlpha,
        F::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        F::DstColor => wgpu::BlendFactor::Dst,
        F::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        F::SrcAlphaSaturate => wgpu::BlendFactor::SrcAlphaSaturated,
        F::ConstantColor | F::ConstantAlpha => wgpu::BlendFactor::Constant,
        F::OneMinusConstantColor | F::OneMinusConstantAlpha => wgpu::BlendFactor::OneMinusConstant,
    }
}

fn to_primitive_topology(draw_mode_type: DrawModeType) -> wgpu::PrimitiveTopology {
    match draw_mode_type {
        DrawModeType::Points => wgpu::PrimitiveTopology::PointList,
        DrawModeType::Lines | DrawModeType::LineLoop => wgpu::PrimitiveTopology::LineList,
        DrawModeType::LineStrip => wgpu::PrimitiveTopology::LineStrip,
        DrawModeType::TriangleStrip => wgpu::PrimitiveTopology::TriangleStrip,
        // WebGPU does not support triangle fan directly; approximate with triangle list
        DrawModeType::TriangleFan | DrawModeType::Triangles => wgpu::PrimitiveTopology::TriangleList,
    }
}

fn depth_compare_function(func: DepthFunctionType) -> wgpu::CompareFunction {
    use DepthFunctionType as D;
    match func {
        D::Never => wgpu::CompareFunction::Never,
        D::Less => wgpu::CompareFunction::Less,
        D::Equal => wgpu::CompareFunction::Equal,
        D::LessEqual => wgpu::CompareFunction::LessEqual,
        D::Greater => wgpu::CompareFunction::Greater,
        D::NotEqual => wgpu::CompareFunction::NotEqual,
        D::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        D::Always => wgpu::CompareFunction::Always,
    }
}

fn stencil_compare_function(func: StencilFunctionType) -> wgpu::CompareFunction {
    use StencilFunctionType as S;
    match func {
        S::Never => wgpu::CompareFunction::Never,
        S::Less => wgpu::CompareFunction::Less,
        S::Equal => wgpu::CompareFunction::Equal,
        S::LessEqual => wgpu::CompareFunction::LessEqual,
        S::Greater => wgpu::CompareFunction::Greater,
        S::NotEqual => wgpu::CompareFunction::NotEqual,
        S::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        S::Always => wgpu::CompareFunction::Always,
    }
}

fn to_stencil_op(op: StencilOpType) -> wgpu::StencilOperation {
    use StencilOpType as O;
    match op {
        O::Zero => wgpu::StencilOperation::Zero,
        O::Keep => wgpu::StencilOperation::Keep,
        O::Replace => wgpu::StencilOperation::Replace,
        O::Increment => wgpu::StencilOperation::IncrementClamp,
        O::Decrement => wgpu::StencilOperation::DecrementClamp,
        O::Invert => wgpu::StencilOperation::Invert,
        O::IncrementWrap => wgpu::StencilOperation::IncrementWrap,
        O::DecrementWrap => wgpu::StencilOperation::DecrementWrap,
    }
}

/// A vertex buffer layout (`WGPUVertexBufferLayout`) with owned attributes.
#[derive(Clone, Debug)]
pub(crate) struct VertexLayout {
    pub(crate) array_stride: u64,
    pub(crate) step_mode: wgpu::VertexStepMode,
    pub(crate) attributes: Vec<wgpu::VertexAttribute>,
}

pub(crate) struct ShaderProgram {
    shader_program_id: SimpleIdentity,
    shader_name: String,
    context: Rc<Context>,
    vertex_shader_module: Option<wgpu::ShaderModule>,
    fragment_shader_module: Option<wgpu::ShaderModule>,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    bind_group_order: Vec<u32>,
    /// The group that holds only `globalIndex` (see
    /// [`get_index_group`](Self::get_index_group)).
    index_group: Option<u32>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    render_pipeline_cache: RefCell<HashMap<u64, wgpu::RenderPipeline>>,
    vertex_attributes: VertexAttributeArray,
    instance_attributes: VertexAttributeArray,
    texture_bindings: [Option<usize>; MAX_TEXTURE_COUNT_PER_SHADER],
    binding_infos: Vec<BindingInfo>,
    bindings_per_group: Vec<Vec<BindingInfo>>,
    has_vertex_entry_point: bool,
    has_fragment_entry_point: bool,
    logged_missing_entry_point: Cell<bool>,
}

impl ShaderProgram {
    /// Static name member (`is_shader_v`)
    pub(crate) const NAME: &'static str = "WebGPUShader";

    pub(crate) fn new(
        context: &Context,
        vertex_source: &str,
        fragment_source: &str,
        attrs: &[AttributeInfo],
        texts: &[TextureInfo],
        name: &str,
    ) -> Self {
        let mut program = Self::from_sources(context, vertex_source, fragment_source);
        if name != "ShaderProgram" {
            program.shader_name = name.to_owned();
        }
        for attr in attrs {
            program.init_attribute(attr);
        }
        for tex in texts {
            program.init_texture(tex);
        }
        program
    }

    /// A program named `ContextShader`, of `vertex_source` and
    /// `fragment_source`.
    ///
    /// A `globalIndex` the sources declare in group 0 moves to a group of its
    /// own (see [`get_index_group`](Self::get_index_group)).
    pub(crate) fn from_sources(context: &Context, vertex_source: &str, fragment_source: &str) -> Self {
        let index_group = index_group_of(&[vertex_source, fragment_source]);
        let (vertex_source, fragment_source) = match index_group {
            Some(group) => (move_global_index(vertex_source, group), move_global_index(fragment_source, group)),
            None => (Cow::Borrowed(vertex_source), Cow::Borrowed(fragment_source)),
        };
        let (vertex_source, fragment_source) = (vertex_source.as_ref(), fragment_source.as_ref());
        let mut program = ShaderProgram {
            shader_program_id: SimpleIdentity::new(),
            shader_name: "ContextShader".to_owned(),
            context: context.rc(),
            vertex_shader_module: None,
            fragment_shader_module: None,
            bind_group_layouts: Vec::new(),
            bind_group_order: Vec::new(),
            index_group,
            pipeline_layout: None,
            render_pipeline_cache: RefCell::new(HashMap::new()),
            vertex_attributes: VertexAttributeArray::new(),
            instance_attributes: VertexAttributeArray::new(),
            texture_bindings: [None; MAX_TEXTURE_COUNT_PER_SHADER],
            binding_infos: Vec::new(),
            bindings_per_group: Vec::new(),
            has_vertex_entry_point: true,
            has_fragment_entry_point: true,
            logged_missing_entry_point: Cell::new(false),
        };
        let device = context.get_backend().device();

        program.has_vertex_entry_point = !trim_string(vertex_source).is_empty();
        program.has_fragment_entry_point = !trim_string(fragment_source).is_empty();

        let create_module = |label: &str, source: &str| -> Option<wgpu::ShaderModule> {
            let module = validated(device, || {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: shader_source(source),
                })
            });
            module.map_err(|error| eprintln!("{error}")).ok()
        };

        // The shader group already prepends the common prelude and runs the
        // preprocessor.
        if program.has_vertex_entry_point {
            program.vertex_shader_module = create_module("Vertex Shader Module", vertex_source);
            if program.vertex_shader_module.is_none() {
                eprintln!("Failed to create vertex shader module for {}", program.shader_name);
                eprintln!("Vertex shader source length: {}", vertex_source.len());
            }
        }

        if program.has_fragment_entry_point {
            program.fragment_shader_module = create_module("Fragment Shader Module", fragment_source);
            if program.fragment_shader_module.is_none() {
                eprintln!("Failed to create fragment shader module for {}", program.shader_name);
                eprintln!("Fragment shader source length: {}", fragment_source.len());
            }
        }

        program.create_pipeline_layout(vertex_source, fragment_source);
        program
    }

    /// The pipeline for the vertex layouts and modes, reused from earlier
    /// calls with the same `reuse_hash`. C++ also takes a renderable, which
    /// it doesn't use.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get_render_pipeline(
        &self,
        vertex_layouts: &[VertexLayout],
        color_mode: &ColorMode,
        depth_mode: &DepthMode,
        stencil_mode: &StencilMode,
        draw_mode_type: DrawModeType,
        reuse_hash: Option<u64>,
        cull_mode: CullFaceMode,
    ) -> Option<wgpu::RenderPipeline> {
        // Check cache first
        if let Some(hash) = reuse_hash
            && let Some(pipeline) = self.render_pipeline_cache.borrow().get(&hash)
        {
            return Some(pipeline.clone());
        }
        // Create new pipeline
        let pipeline =
            self.create_pipeline(vertex_layouts, color_mode, depth_mode, stencil_mode, draw_mode_type, cull_mode);
        // Cache the pipeline if we have a reuse hash
        if let (Some(hash), Some(p)) = (reuse_hash, &pipeline) {
            self.render_pipeline_cache.borrow_mut().insert(hash, p.clone());
        }
        pipeline
    }

    pub(crate) fn get_binding_infos_for_group(&self, group: u32) -> &[BindingInfo] {
        for (i, &g) in self.bind_group_order.iter().enumerate() {
            if g == group {
                return &self.bindings_per_group[i];
            }
        }
        &[]
    }

    pub(crate) fn get_bind_group_layout(&self, group: u32) -> Option<&wgpu::BindGroupLayout> {
        self.bind_group_order.iter().position(|&g| g == group).map(|i| &self.bind_group_layouts[i])
    }

    pub(crate) fn get_bind_group_order(&self) -> &[u32] {
        &self.bind_group_order
    }

    /// The group that holds only `globalIndex`, the index of the draw's
    /// entries in the layer's UBO vectors, at binding `ID_GLOBAL_UBO_INDEX`.
    /// Its layout is the context's index group layout, and a draw binds it
    /// with the dynamic offset of its index (`Context::index_bind_group`),
    /// so its other groups bind only what it shares with the layer's other
    /// draws.
    pub(crate) fn get_index_group(&self) -> Option<u32> {
        self.index_group
    }

    pub(crate) fn get_shader_name(&self) -> &str {
        &self.shader_name
    }

    /// Whether both WGSL modules compiled.
    #[cfg(test)]
    pub(crate) fn has_shader_modules(&self) -> bool {
        self.vertex_shader_module.is_some() && self.fragment_shader_module.is_some()
    }

    pub(crate) fn init_attribute(&mut self, info: &AttributeInfo) {
        let index = info.index as i32;
        // Indexes must be unique, if there's a conflict check the `attributes`
        // array in the shader
        #[cfg(debug_assertions)]
        {
            self.vertex_attributes.visit_attributes(|a| debug_assert_ne!(a.get_index(), index));
            self.instance_attributes.visit_attributes(|a| debug_assert_ne!(a.get_index(), index));
        }
        self.vertex_attributes.set(info.id, index, info.data_type, 1);
    }

    pub(crate) fn init_instance_attribute(&mut self, info: &AttributeInfo) {
        // Index is the block index of the instance attribute
        let index = info.index as i32;
        #[cfg(debug_assertions)]
        self.vertex_attributes.visit_attributes(|a| debug_assert_ne!(a.get_index(), index));
        self.instance_attributes.set(info.id, index, info.data_type, 1);
    }

    pub(crate) fn init_texture(&mut self, info: &TextureInfo) {
        debug_assert!(info.id < self.texture_bindings.len());
        if info.id >= self.texture_bindings.len() {
            return;
        }
        self.texture_bindings[info.id] = Some(info.index);
    }

    fn create_pipeline_layout(&mut self, vertex_source: &str, fragment_source: &str) {
        let device = self.context.get_backend().device().clone();
        if !vertex_source.is_empty() || !fragment_source.is_empty() {
            self.binding_infos.clear();
            if !vertex_source.is_empty() {
                self.analyze_shader_bindings(vertex_source, wgpu::ShaderStages::VERTEX);
            }
            if !fragment_source.is_empty() {
                self.analyze_shader_bindings(fragment_source, wgpu::ShaderStages::FRAGMENT);
            }
        }

        self.rebuild_bind_group_layouts(&device);

        let layouts: Vec<Option<&wgpu::BindGroupLayout>> = self.bind_group_layouts.iter().map(Some).collect();
        self.pipeline_layout = Some(device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        }));
    }

    /// Scans `@group(N) @binding(M) var<...> name: type` declarations, as the
    /// C++ text scanner does.
    fn analyze_shader_bindings(&mut self, source: &str, stage: wgpu::ShaderStages) {
        let b = source.as_bytes();
        let find = |pat: &str, from: usize| source.get(from..).and_then(|s| s.find(pat)).map(|p| p + from);
        let find_char = |c: char, from: usize| source.get(from..).and_then(|s| s.find(c)).map(|p| p + from);
        let mut search_pos = 0usize;
        while let Some(group_token) = find("@group", search_pos) {
            let Some(group_paren) = find_char('(', group_token) else { break };
            let mut group_start = group_paren + 1;
            while group_start < b.len() && is_space(b[group_start]) {
                group_start += 1;
            }
            let mut group_end = group_start;
            while group_end < b.len() && b[group_end].is_ascii_digit() {
                group_end += 1;
            }
            if group_end == group_start {
                search_pos = group_end;
                continue;
            }
            let group: u32 = source[group_start..group_end].parse().unwrap_or(0);

            let Some(binding_token) = find("@binding", group_end) else { break };
            let Some(binding_paren) = find_char('(', binding_token) else { break };
            let mut binding_start = binding_paren + 1;
            while binding_start < b.len() && is_space(b[binding_start]) {
                binding_start += 1;
            }
            let mut binding_end = binding_start;
            while binding_end < b.len() && b[binding_end].is_ascii_digit() {
                binding_end += 1;
            }
            if binding_end == binding_start {
                search_pos = binding_end;
                continue;
            }
            let binding: u32 = source[binding_start..binding_end].parse().unwrap_or(0);

            let Some(var_pos) = find("var", binding_end) else { break };
            if var_pos > binding_end && b[var_pos - 1].is_ascii_alphabetic() {
                search_pos = var_pos + 3;
                continue;
            }
            let mut after_var = var_pos + 3;
            while after_var < b.len() && is_space(b[after_var]) {
                after_var += 1;
            }

            let mut type_ = BindingType::UniformBuffer;
            if after_var < b.len() && b[after_var] == b'<' {
                let Some(qualifier_end) = find_char('>', after_var) else { break };
                let qualifier = trim_string(&source[after_var + 1..qualifier_end]);
                if qualifier.contains("storage") {
                    type_ = if qualifier.contains("read_write") {
                        BindingType::StorageBuffer
                    } else {
                        BindingType::ReadOnlyStorageBuffer
                    };
                } else {
                    type_ = BindingType::UniformBuffer;
                }
                after_var = qualifier_end + 1;
            }

            let Some(colon_pos) = find_char(':', after_var) else { break };
            // Extract the variable name
            let var_name = trim_string(&source[after_var..colon_pos]);
            let type_start = colon_pos + 1;
            let mut type_end = type_start;
            while type_end < b.len() {
                let c = b[type_end];
                if c == b';' || c == b'\n' || c == b'\r' {
                    break;
                }
                type_end += 1;
            }
            let type_name = trim_string(&source[type_start..type_end]);

            if type_ == BindingType::UniformBuffer {
                if type_name.contains("sampler") {
                    type_ = BindingType::Sampler;
                } else if type_name.contains("texture") {
                    // textureLoad only (no textureSample) means unfilterable
                    let uses_texture_load = source.contains(&format!("textureLoad({var_name}"));
                    let uses_texture_sample = source.contains(&format!("textureSample({var_name}"));
                    type_ = if uses_texture_load && !uses_texture_sample {
                        BindingType::UnfilterableTexture
                    } else {
                        BindingType::Texture
                    };
                }
            }

            if let Some(existing) =
                self.binding_infos.iter_mut().find(|info| info.group == group && info.binding == binding)
            {
                existing.visibility |= stage;
                if existing.type_ == BindingType::UniformBuffer && type_ != BindingType::UniformBuffer {
                    existing.type_ = type_;
                }
            } else {
                self.binding_infos.push(BindingInfo { group, binding, type_, visibility: stage });
            }

            search_pos = type_end;
        }
    }

    fn rebuild_bind_group_layouts(&mut self, device: &wgpu::Device) {
        self.bind_group_layouts.clear();
        self.bind_group_order.clear();
        self.bindings_per_group.clear();

        if self.binding_infos.is_empty() {
            self.binding_infos.push(BindingInfo {
                group: 0,
                binding: 0,
                type_: BindingType::UniformBuffer,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            });
        }

        let mut grouped: VecMap<u32, Vec<BindingInfo>> = VecMap::new();
        for info in &self.binding_infos {
            grouped.entry(info.group).or_default().push(*info);
        }

        for (group, mut infos) in grouped {
            infos.sort_by_key(|info| info.binding);
            if Some(group) == self.index_group {
                self.bind_group_order.push(group);
                self.bindings_per_group.push(infos);
                self.bind_group_layouts.push(self.context.index_bind_group_layout());
                continue;
            }
            let layout_entries: Vec<wgpu::BindGroupLayoutEntry> = infos
                .iter()
                .map(|info| wgpu::BindGroupLayoutEntry {
                    binding: info.binding,
                    visibility: info.visibility,
                    ty: match info.type_ {
                        BindingType::UniformBuffer => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        BindingType::ReadOnlyStorageBuffer => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        BindingType::StorageBuffer => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        BindingType::Sampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        BindingType::Texture => wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        BindingType::UnfilterableTexture => wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    },
                    count: None,
                })
                .collect();
            let label = format!("Shader Bind Group {group}");
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(&label),
                entries: &layout_entries,
            });
            self.bind_group_order.push(group);
            self.bindings_per_group.push(infos);
            self.bind_group_layouts.push(layout);
        }
    }

    fn create_pipeline(
        &self,
        vertex_layouts: &[VertexLayout],
        color_mode: &ColorMode,
        depth_mode: &DepthMode,
        stencil_mode: &StencilMode,
        draw_mode_type: DrawModeType,
        cull_mode: CullFaceMode,
    ) -> Option<wgpu::RenderPipeline> {
        let device = self.context.get_backend().device();
        if !self.has_vertex_entry_point || !self.has_fragment_entry_point {
            if !self.logged_missing_entry_point.get() {
                eprintln!(
                    "WebGPU: shader '{}' is missing WGSL entry points; skipping pipeline creation",
                    self.shader_name
                );
                self.logged_missing_entry_point.set(true);
            }
            return None;
        }
        let (Some(vertex_module), Some(fragment_module), Some(pipeline_layout)) =
            (self.vertex_shader_module.as_ref(), self.fragment_shader_module.as_ref(), self.pipeline_layout.as_ref())
        else {
            eprintln!(
                "createPipeline missing requirement: vertexModule={}, fragmentModule={}, pipelineLayout={}",
                self.vertex_shader_module.is_some(),
                self.fragment_shader_module.is_some(),
                self.pipeline_layout.is_some()
            );
            return None;
        };

        let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = vertex_layouts
            .iter()
            .map(|l| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: l.array_stride,
                    step_mode: l.step_mode,
                    attributes: &l.attributes,
                })
            })
            .collect();

        // Set up fragment state with color mode blending. Quirk ported: the
        // blend state is always set; `Replace` keeps the SrcAlpha /
        // OneMinusSrcAlpha defaults.
        let mut alpha_blend = wgpu::BlendComponent {
            operation: webgpu_blend_operation(ColorBlendEquationType::Add),
            src_factor: webgpu_blend_factor(ColorBlendFactorType::SrcAlpha),
            dst_factor: webgpu_blend_factor(ColorBlendFactorType::OneMinusSrcAlpha),
        };
        let mut color_blend = alpha_blend;
        // Apply color mode blending if not replace
        if color_mode.blend_function != BlendFunction::Replace {
            let f = &color_mode.blend_function;
            color_blend = wgpu::BlendComponent {
                operation: webgpu_blend_operation(f.equation()),
                src_factor: webgpu_blend_factor(f.src_factor()),
                dst_factor: webgpu_blend_factor(f.dst_factor()),
            };
            alpha_blend = color_blend;
        }

        let color_format = self.context.get_backend().get_color_format().unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
        let mut write_mask = wgpu::ColorWrites::empty();
        if color_mode.mask.r {
            write_mask |= wgpu::ColorWrites::RED;
        }
        if color_mode.mask.g {
            write_mask |= wgpu::ColorWrites::GREEN;
        }
        if color_mode.mask.b {
            write_mask |= wgpu::ColorWrites::BLUE;
        }
        if color_mode.mask.a {
            write_mask |= wgpu::ColorWrites::ALPHA;
        }
        let targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState { color: color_blend, alpha: alpha_blend }),
            write_mask,
        })];

        // Set up primitive state
        let primitive = wgpu::PrimitiveState {
            topology: to_primitive_topology(draw_mode_type),
            strip_index_format: if matches!(draw_mode_type, DrawModeType::LineStrip | DrawModeType::TriangleStrip) {
                Some(wgpu::IndexFormat::Uint16)
            } else {
                None
            },
            front_face: if cull_mode.winding == CullFaceWindingType::CounterClockwise {
                wgpu::FrontFace::Ccw
            } else {
                wgpu::FrontFace::Cw
            },
            cull_mode: if !cull_mode.enabled {
                None
            } else if cull_mode.side == CullFaceSideType::Front {
                Some(wgpu::Face::Front)
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        };

        // Set up depth stencil state
        let depth_stencil = self.context.get_backend().get_depth_stencil_format().map(|depth_format| {
            let keep_face = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Keep,
            };
            let mut stencil = wgpu::StencilState { front: keep_face, back: keep_face, read_mask: 0, write_mask: 0 };

            let stencil_enabled = !(stencil_mode.test == Test::Always
                && stencil_mode.mask == 0
                && stencil_mode.fail == StencilOpType::Keep
                && stencil_mode.depth_fail == StencilOpType::Keep
                && stencil_mode.pass == StencilOpType::Keep);
            if stencil_enabled {
                let function = stencil_mode.test.func();
                let read_mask = stencil_mode.test.masked().unwrap_or(0xFF);
                let face = wgpu::StencilFaceState {
                    compare: stencil_compare_function(function),
                    fail_op: to_stencil_op(stencil_mode.fail),
                    depth_fail_op: to_stencil_op(stencil_mode.depth_fail),
                    pass_op: to_stencil_op(stencil_mode.pass),
                };
                stencil = wgpu::StencilState { front: face, back: face, read_mask, write_mask: stencil_mode.mask };
            }

            if self.shader_name == "ClippingMaskProgram" {
                stencil.front.pass_op = wgpu::StencilOperation::Replace;
                if stencil.write_mask == 0 {
                    stencil.write_mask = 0xFF;
                }
                if stencil.read_mask == 0 {
                    stencil.read_mask = 0xFF;
                }
                stencil.front.compare = wgpu::CompareFunction::Always;
                stencil.back = stencil.front;
            }

            wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(depth_mode.mask == DepthMaskType::ReadWrite),
                depth_compare: Some(depth_compare_function(depth_mode.func)),
                stencil,
                bias: wgpu::DepthBiasState::default(),
            }
        });

        // Create render pipeline (inside an error scope: a failure is the C++
        // null pipeline)
        let pipeline = validated(device, || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&self.shader_name),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: vertex_module,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                primitive,
                depth_stencil,
                multisample: wgpu::MultisampleState { count: 1, mask: 0xFFFF_FFFF, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: fragment_module,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        pipeline.map_err(|error| eprintln!("{} pipeline creation failed: {error}", self.shader_name)).ok()
    }
}

impl Shader for ShaderProgram {
    fn type_name(&self) -> &'static str {
        Self::NAME
    }
}

impl ShaderProgramBase for ShaderProgram {
    fn get_id(&self) -> SimpleIdentity {
        self.shader_program_id
    }
    fn get_sampler_location(&self, id: usize) -> Option<usize> {
        self.texture_bindings.get(id).copied().flatten()
    }
    fn get_vertex_attributes(&self) -> &VertexAttributeArray {
        &self.vertex_attributes
    }
    fn get_instance_attributes(&self) -> &VertexAttributeArray {
        &self.instance_attributes
    }
}

/// Downcast a shader handle to the WebGPU program (C++ `static_cast`).
pub(crate) fn as_webgpu_shader(shader: &Rc<dyn ShaderProgramBase>) -> Option<&ShaderProgram> {
    shader.downcast_ref::<ShaderProgram>()
}

/// How the vendored sources declare `globalIndex`.
const GLOBAL_INDEX_DECLARATION: &str = "@group(0) @binding(1) var<uniform> globalIndex:";

/// The group after the last one `sources` use, if they declare `globalIndex`
/// in group 0.
fn index_group_of(sources: &[&str]) -> Option<u32> {
    if !sources.iter().any(|source| source.contains(GLOBAL_INDEX_DECLARATION)) {
        return None;
    }
    let last = sources
        .iter()
        .flat_map(|source| source.split("@group(").skip(1))
        .filter_map(|rest| rest.split(')').next()?.trim().parse::<u32>().ok())
        .max()?;
    Some(last + 1)
}

/// `source` with its `globalIndex` declaration in `group`.
fn move_global_index(source: &str, group: u32) -> Cow<'_, str> {
    if source.contains(GLOBAL_INDEX_DECLARATION) {
        let declaration = GLOBAL_INDEX_DECLARATION.replacen("@group(0)", &format!("@group({group})"), 1);
        Cow::Owned(source.replacen(GLOBAL_INDEX_DECLARATION, &declaration, 1))
    } else {
        Cow::Borrowed(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_index_moves_to_the_group_after_the_last() {
        let vertex = "@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;\n\
                      @group(0) @binding(2) var<storage, read> drawableVector: array<U>;";
        let fragment = "@group(1) @binding(0) var image_sampler: sampler;";
        assert_eq!(index_group_of(&[vertex, fragment]), Some(2));
        assert_eq!(index_group_of(&[fragment]), None);
        assert_eq!(
            move_global_index(vertex, 2),
            "@group(2) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;\n\
             @group(0) @binding(2) var<storage, read> drawableVector: array<U>;"
        );
        assert!(matches!(move_global_index(fragment, 2), Cow::Borrowed(_)));
    }
}
