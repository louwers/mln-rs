//! WGSL programs and their pipelines. Shader modules and pipelines are
//! created inside a validation error scope, so that a WGSL compile error or a
//! rejected pipeline is an `Err`. On a browser's WebGPU the scope is skipped
//! (see [`validated`]).

use std::borrow::Cow;

use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{FixedState, GpuError, ShaderKey, VertexBufferLayout, VertexStep};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::WgslSource;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use mln_util::hash::HashMap;
use mln_vec_map::VecMap;

use crate::shaders::declaration::Declaration;
use crate::shaders::prelude::PRELUDE;
use crate::shaders::wgsl_preprocessor::{is_space, preprocess_wgsl};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingType {
    UniformBuffer,
    ReadOnlyStorageBuffer,
    Sampler,
    /// The sampler of an unfilterable texture.
    NonFilteringSampler,
    Texture,
    /// A texture whose format can't be filtered, or that the shader only
    /// reads with `textureLoad`.
    UnfilterableTexture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BindingInfo {
    pub(crate) group: u32,
    pub(crate) binding: u32,
    pub(crate) type_: BindingType,
    pub(crate) visibility: wgpu::ShaderStages,
    /// The bytes of a buffer that the shaders read: of one that ends in an
    /// array of any length, up to its first element. 0 for a texture or a
    /// sampler.
    pub(crate) min_size: u64,
}

/// Runs `create` inside a validation error scope and returns the error the
/// scope caught. On a browser's WebGPU the scope is skipped: popping it waits for the browser, which in
/// the middle of a frame expires the canvas texture. Errors there reach the
/// device's uncaptured-error callback instead.
pub(crate) fn validated<T>(device: &wgpu::Device, create: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    // The browser's WebGPU is wgpu's only backend in the browser build
    // (Cargo.toml). Asking the device for its backend would copy the
    // adapter's names on every call.
    if cfg!(target_family = "wasm") {
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
    match parsed(source) {
        Some(module) => wgpu::ShaderSource::Naga(Cow::Owned(module)),
        None => wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
    }
}

#[cfg(target_family = "wasm")]
fn shader_source(source: &str) -> wgpu::ShaderSource<'_> {
    wgpu::ShaderSource::Wgsl(Cow::Borrowed(source))
}

/// The module naga parses `source` to, parsed once a process; `None` for a
/// source that doesn't parse.
#[cfg(not(target_family = "wasm"))]
fn parsed(source: &str) -> Option<wgpu::naga::Module> {
    use parking_lot::Mutex;

    static PARSED: Mutex<Option<HashMap<String, wgpu::naga::Module>>> = Mutex::new(None);
    if let Some(module) = PARSED.lock().get_or_insert_default().get(source) {
        return Some(module.clone());
    }
    let module = wgpu::naga::front::wgsl::parse_str(source).ok()?;
    PARSED.lock().get_or_insert_default().insert(source.to_owned(), module.clone());
    Some(module)
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

/// The topology of `draw_mode_type`; `None` for the loops and fans WebGPU
/// lacks.
fn to_primitive_topology(draw_mode_type: DrawModeType) -> Option<wgpu::PrimitiveTopology> {
    Some(match draw_mode_type {
        DrawModeType::Points => wgpu::PrimitiveTopology::PointList,
        DrawModeType::Lines => wgpu::PrimitiveTopology::LineList,
        DrawModeType::LineStrip => wgpu::PrimitiveTopology::LineStrip,
        DrawModeType::TriangleStrip => wgpu::PrimitiveTopology::TriangleStrip,
        DrawModeType::Triangles => wgpu::PrimitiveTopology::TriangleList,
        DrawModeType::LineLoop | DrawModeType::TriangleFan => return None,
    })
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

/// The vertex format that a shader input of `type_` reads, if WebGPU has
/// one.
pub(crate) fn vertex_format(type_: AttributeDataType) -> Option<wgpu::VertexFormat> {
    use AttributeDataType as A;
    use wgpu::VertexFormat as F;
    Some(match type_ {
        A::Byte => F::Sint8,
        A::Byte2 => F::Sint8x2,
        A::Byte4 => F::Sint8x4,
        A::UByte => F::Uint8,
        A::UByte2 => F::Uint8x2,
        A::UByte4 => F::Uint8x4,
        A::UByte4Normalized => F::Unorm8x4,
        A::Short => F::Sint16,
        A::Short2 => F::Sint16x2,
        A::Short4 => F::Sint16x4,
        A::UShort => F::Uint16,
        A::UShort2 => F::Uint16x2,
        A::UShort4 => F::Uint16x4,
        A::Int => F::Sint32,
        A::Int2 => F::Sint32x2,
        A::Int3 => F::Sint32x3,
        A::Int4 => F::Sint32x4,
        A::UInt => F::Uint32,
        A::UInt2 => F::Uint32x2,
        A::UInt3 => F::Uint32x3,
        A::UInt4 => F::Uint32x4,
        A::Float => F::Float32,
        A::Float2 => F::Float32x2,
        A::Float3 => F::Float32x3,
        A::Float4 => F::Float32x4,
        _ => return None,
    })
}

/// A WGSL program: its vertex and fragment modules, and its bind group
/// layouts as its declaration has them.
pub(crate) struct ShaderProgram {
    shader_name: String,
    device: wgpu::Device,
    vertex_shader_module: wgpu::ShaderModule,
    fragment_shader_module: wgpu::ShaderModule,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    bind_group_order: Vec<u32>,
    /// The group that holds only `globalIndex` (see
    /// [`index_group`](Self::index_group)).
    index_group: Option<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    program: &'static Program,
    bindings_per_group: Vec<Vec<BindingInfo>>,
    /// The value of the `DEVICE_PIXEL_RATIO` override, for each stage that
    /// declares it.
    vertex_constants: Vec<(&'static str, f64)>,
    fragment_constants: Vec<(&'static str, f64)>,
}

/// The pipeline-overridable constant that holds the program's pixel ratio.
/// WGSL reads no define's value, so a stage that needs the ratio declares
/// it as `override DEVICE_PIXEL_RATIO: f32`.
const PIXEL_RATIO_OVERRIDE: &str = "DEVICE_PIXEL_RATIO";

/// The constants a stage of `source` overrides, at `pixel_ratio`. A
/// pipeline may set only the constants its stage declares.
fn constants(source: &str, pixel_ratio: f32) -> Vec<(&'static str, f64)> {
    if source.contains(&format!("override {PIXEL_RATIO_OVERRIDE}:")) {
        vec![(PIXEL_RATIO_OVERRIDE, f64::from(pixel_ratio))]
    } else {
        Vec::new()
    }
}

impl ShaderProgram {
    /// The program `program`, from `vertex_source` and `fragment_source`,
    /// its WGSL preprocessed already, with the bindings of `declaration`
    /// and `index_layout` for its index group (see
    /// [`index_group`](Self::index_group)).
    ///
    /// Its pipelines set the stages' `DEVICE_PIXEL_RATIO` override to
    /// `pixel_ratio`.
    ///
    /// Fails if either source is empty or doesn't compile.
    pub(crate) fn new(
        device: &wgpu::Device,
        index_layout: &wgpu::BindGroupLayout,
        vertex_source: &str,
        fragment_source: &str,
        program: &'static Program,
        declaration: Declaration,
        pixel_ratio: f32,
    ) -> Result<Self, GpuError> {
        let name = program.name;
        let create_module = |stage: &str, source: &str| -> Result<wgpu::ShaderModule, GpuError> {
            if trim_string(source).is_empty() {
                return Err(GpuError::Invalid(format!("shader {name} has no {stage} stage")));
            }
            let label = format!("{name} {stage}");
            validated(device, || {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&label),
                    source: shader_source(source),
                })
            })
            .map_err(|error| GpuError::Invalid(format!("shader {name}'s {stage} stage: {error}")))
        };
        let vertex_shader_module = create_module("vertex", vertex_source)?;
        let fragment_shader_module = create_module("fragment", fragment_source)?;

        let index_group = declaration.index_group;
        let groups = BindGroups::new(device, declaration.bindings, index_group, index_layout);
        // A group that holds no binding, below one that does, has no layout.
        let mut layouts: Vec<Option<&wgpu::BindGroupLayout>> = Vec::new();
        for (&group, layout) in groups.order.iter().zip(&groups.layouts) {
            layouts.resize(group as usize, None);
            layouts.push(Some(layout));
        }
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });

        Ok(ShaderProgram {
            shader_name: name.to_owned(),
            device: device.clone(),
            vertex_shader_module,
            fragment_shader_module,
            bind_group_layouts: groups.layouts,
            bind_group_order: groups.order,
            index_group,
            pipeline_layout,
            program,
            bindings_per_group: groups.bindings,
            vertex_constants: constants(vertex_source, pixel_ratio),
            fragment_constants: constants(fragment_source, pixel_ratio),
        })
    }

    /// How many storage buffers the vertex stage reads.
    #[cfg(test)]
    pub(crate) fn vertex_storage_buffers(&self) -> u32 {
        let storage = |info: &&BindingInfo| {
            info.type_ == BindingType::ReadOnlyStorageBuffer && info.visibility.contains(wgpu::ShaderStages::VERTEX)
        };
        self.bindings_per_group.iter().flatten().filter(storage).count() as u32
    }

    /// The bindings of `group`, by binding.
    pub(crate) fn binding_infos(&self, group: u32) -> &[BindingInfo] {
        self.bind_group_order.iter().position(|&g| g == group).map_or(&[], |i| &self.bindings_per_group[i])
    }

    pub(crate) fn bind_group_layout(&self, group: u32) -> Option<&wgpu::BindGroupLayout> {
        self.bind_group_order.iter().position(|&g| g == group).map(|i| &self.bind_group_layouts[i])
    }

    /// The groups that hold bindings, in order: each binds at the pipeline
    /// layout's slot of its number.
    pub(crate) fn bind_group_order(&self) -> &[u32] {
        &self.bind_group_order
    }

    /// The group that holds only `globalIndex`, the index of the draw's
    /// entries in the layer's UBO vectors, at binding [`GLOBAL_INDEX_ID`](mln_gfx::shaders::bindings::GLOBAL_INDEX_ID).
    /// Its layout is the index table's, and a draw binds it with the dynamic
    /// offset of its index, so its other groups bind only what it shares
    /// with the layer's other draws.
    pub(crate) fn index_group(&self) -> Option<u32> {
        self.index_group
    }

    pub(crate) fn name(&self) -> &str {
        &self.shader_name
    }

    /// The shader location of the program's attribute at `location`, if it
    /// steps by `step`: its location.
    fn location(&self, step: VertexStep, location: u32) -> Option<u32> {
        match self.program.attribute_at(location as usize) {
            Some((at, _)) if at == step => Some(location),
            _ => None,
        }
    }

    /// The pipeline of this program with `state`, reading vertex buffers of
    /// `layouts`, drawing into a `color` attachment and a `depth_stencil`
    /// one, either of which may be absent.
    /// Fails with [`GpuError::Unsupported`] for a primitive WebGPU lacks: a
    /// line loop or a triangle fan.
    pub(crate) fn render_pipeline(
        &self,
        layouts: &[VertexBufferLayout],
        state: &FixedState,
        color_format: Option<wgpu::TextureFormat>,
        depth_stencil_format: Option<wgpu::TextureFormat>,
    ) -> Result<wgpu::RenderPipeline, GpuError> {
        let attributes = layouts
            .iter()
            .map(|layout| {
                layout
                    .attributes
                    .iter()
                    .map(|attribute| {
                        let format = vertex_format(attribute.format).ok_or_else(|| {
                            GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format))
                        })?;
                        let shader_location = self.location(layout.step, attribute.location).ok_or_else(|| {
                            GpuError::Invalid(format!("{} has no attribute at {}", self.name(), attribute.location))
                        })?;
                        Ok(wgpu::VertexAttribute { format, offset: u64::from(attribute.offset), shader_location })
                    })
                    .collect::<Result<Vec<_>, GpuError>>()
            })
            .collect::<Result<Vec<_>, GpuError>>()?;
        let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = layouts
            .iter()
            .zip(&attributes)
            .map(|(layout, attributes)| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: u64::from(layout.stride),
                    step_mode: match layout.step {
                        VertexStep::Vertex => wgpu::VertexStepMode::Vertex,
                        VertexStep::Instance => wgpu::VertexStepMode::Instance,
                    },
                    attributes,
                })
            })
            .collect();

        // `Replace` writes the fragment's color as it is.
        let blend = (state.blend != BlendFunction::Replace).then(|| {
            let component = wgpu::BlendComponent {
                operation: webgpu_blend_operation(state.blend.equation()),
                src_factor: webgpu_blend_factor(state.blend.src_factor()),
                dst_factor: webgpu_blend_factor(state.blend.dst_factor()),
            };
            wgpu::BlendState { color: component, alpha: component }
        });
        let mut write_mask = wgpu::ColorWrites::empty();
        write_mask.set(wgpu::ColorWrites::RED, state.color_mask.r);
        write_mask.set(wgpu::ColorWrites::GREEN, state.color_mask.g);
        write_mask.set(wgpu::ColorWrites::BLUE, state.color_mask.b);
        write_mask.set(wgpu::ColorWrites::ALPHA, state.color_mask.a);
        let targets = [color_format.map(|format| wgpu::ColorTargetState { format, blend, write_mask })];

        let topology = to_primitive_topology(state.primitive)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} primitives", state.primitive)))?;
        let primitive = wgpu::PrimitiveState {
            topology,
            strip_index_format: matches!(state.primitive, DrawModeType::LineStrip | DrawModeType::TriangleStrip)
                .then_some(wgpu::IndexFormat::Uint16),
            front_face: if state.cull.winding == CullFaceWindingType::CounterClockwise {
                wgpu::FrontFace::Ccw
            } else {
                wgpu::FrontFace::Cw
            },
            cull_mode: if !state.cull.enabled {
                None
            } else if state.cull.side == CullFaceSideType::Front {
                Some(wgpu::Face::Front)
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        };

        let depth_stencil = depth_stencil_format.map(|format| {
            let stencil = if state.stencil.is_disabled() {
                wgpu::StencilState::default()
            } else {
                let face = wgpu::StencilFaceState {
                    compare: stencil_compare_function(state.stencil.test.func()),
                    fail_op: to_stencil_op(state.stencil.fail),
                    depth_fail_op: to_stencil_op(state.stencil.depth_fail),
                    pass_op: to_stencil_op(state.stencil.pass),
                };
                wgpu::StencilState {
                    front: face,
                    back: face,
                    read_mask: state.stencil.test.masked().unwrap_or(0xFF),
                    write_mask: state.stencil.write_mask,
                }
            };
            wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(state.depth.write),
                depth_compare: Some(depth_compare_function(state.depth.compare)),
                stencil,
                bias: wgpu::DepthBiasState::default(),
            }
        });

        let device = &self.device;
        validated(device, || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&self.shader_name),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.vertex_shader_module,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &self.vertex_constants,
                        ..Default::default()
                    },
                    buffers: &buffers,
                },
                primitive,
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.fragment_shader_module,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &self.fragment_constants,
                        ..Default::default()
                    },
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        })
        .map_err(|error| GpuError::Invalid(format!("{} pipeline: {error}", self.shader_name)))
    }
}

/// A program's bind group layouts, in the order of its pipeline layout's
/// slots, with the group and the bindings of each.
struct BindGroups {
    layouts: Vec<wgpu::BindGroupLayout>,
    order: Vec<u32>,
    bindings: Vec<Vec<BindingInfo>>,
}

impl BindGroups {
    /// The groups of `binding_infos`, with `index_layout` for `index_group`.
    fn new(
        device: &wgpu::Device,
        binding_infos: Vec<BindingInfo>,
        index_group: Option<u32>,
        index_layout: &wgpu::BindGroupLayout,
    ) -> BindGroups {
        let mut grouped: VecMap<u32, Vec<BindingInfo>> = VecMap::new();
        for info in binding_infos {
            grouped.entry(info.group).or_default().push(info);
        }
        if let Some(group) = index_group {
            grouped.entry(group).or_default();
        }

        let mut groups = BindGroups { layouts: Vec::new(), order: Vec::new(), bindings: Vec::new() };
        for (group, mut infos) in grouped {
            infos.sort_by_key(|info| info.binding);
            let layout = if Some(group) == index_group {
                index_layout.clone()
            } else {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = infos
                    .iter()
                    .map(|info| wgpu::BindGroupLayoutEntry {
                        binding: info.binding,
                        visibility: info.visibility,
                        ty: info.type_.layout_type(),
                        count: None,
                    })
                    .collect();
                let label = format!("Shader Bind Group {group}");
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(&label),
                    entries: &entries,
                })
            };
            groups.order.push(group);
            groups.bindings.push(infos);
            groups.layouts.push(layout);
        }
        groups
    }
}

impl BindingType {
    fn layout_type(self) -> wgpu::BindingType {
        let buffer = |ty| wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None };
        let texture = |filterable| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        match self {
            BindingType::UniformBuffer => buffer(wgpu::BufferBindingType::Uniform),
            BindingType::ReadOnlyStorageBuffer => buffer(wgpu::BufferBindingType::Storage { read_only: true }),
            BindingType::Sampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            BindingType::NonFilteringSampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            BindingType::Texture => texture(true),
            BindingType::UnfilterableTexture => texture(false),
        }
    }
}

/// Compiles the program of `key` from `source`: the common prelude, the
/// declarations of the key's variant ([`Declaration`]), the program's own
/// prelude and each stage, preprocessed with the key's defines.
pub(crate) fn compile(
    device: &wgpu::Device,
    index_layout: &wgpu::BindGroupLayout,
    key: &ShaderKey,
    source: &'static WgslSource,
) -> Result<ShaderProgram, GpuError> {
    let declaration = Declaration::new(key)?;
    // The common prelude goes through the preprocessor with the program, so
    // its `#ifdef`s see the defines.
    let shader_prelude = source.prelude.unwrap_or("");
    let declarations = &declaration.declarations;
    let vertex_input = &declaration.vertex_input;
    let vertex = format!("{PRELUDE}\n{declarations}{vertex_input}{shader_prelude}{}", source.vertex.unwrap_or(""));
    let fragment = format!("{PRELUDE}\n{declarations}{shader_prelude}{}", source.fragment.unwrap_or(""));
    // WGSL reads no define's value; the pixel ratio's is an override's.
    let defines: HashMap<String, bool> = key.define_values().map(|(name, _)| (name.to_owned(), true)).collect();
    let vertex_source = preprocess_wgsl(&vertex, &defines);
    let fragment_source = preprocess_wgsl(&fragment, &defines);
    let pixel_ratio = key
        .define_values()
        .find(|&(name, _)| name == PIXEL_RATIO_OVERRIDE)
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(1.0);

    ShaderProgram::new(device, index_layout, &vertex_source, &fragment_source, key.program(), declaration, pixel_ratio)
}
