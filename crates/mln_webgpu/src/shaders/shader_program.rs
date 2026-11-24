//! WGSL programs and their pipelines. Shader modules and pipelines are
//! created inside a validation error scope, so that a WGSL compile error or a
//! rejected pipeline is an `Err`. On a browser's WebGPU the scope is skipped
//! (see [`validated`]).

use std::borrow::Cow;

use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{FixedState, GpuError, ShaderKey, VertexBufferLayout, VertexStep};
use mln_gfx::shaders::program::{Binding, BufferKind, Program};
use mln_gfx::shaders::source::wgsl::{self, Wgsl};
use mln_gfx::shaders::source::{Stage, identifiers};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use mln_util::hash::{HashMap, HashSet};
use mln_vec_map::VecMap;

use crate::shaders::wgsl_preprocessor::{is_space, preprocess_wgsl};
use crate::shaders::wgsl_sources::{BUFFER_GROUP, PRELUDE, TEXTURE_GROUP, binding_type, declaration, location};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingType {
    UniformBuffer,
    ReadOnlyStorageBuffer,
    Sampler,
    /// The sampler of an unfilterable texture.
    NonFilteringSampler,
    Texture,
    /// A texture that its program declares unfilterable.
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

/// The bytes of each buffer that `sources` read, by group and binding: of a
/// buffer that ends in an array of any length, up to its first element.
#[cfg(not(target_family = "wasm"))]
fn buffer_sizes(sources: &[&str]) -> Vec<((u32, u32), u64)> {
    let mut sizes: Vec<((u32, u32), u64)> = Vec::new();
    for module in sources.iter().filter_map(|source| parsed(source)) {
        let mut layouter = wgpu::naga::proc::Layouter::default();
        if layouter.update(module.to_ctx()).is_err() {
            continue;
        }
        for (_, global) in module.global_variables.iter() {
            let Some(binding) = &global.binding else { continue };
            if !matches!(global.space, wgpu::naga::AddressSpace::Uniform | wgpu::naga::AddressSpace::Storage { .. }) {
                continue;
            }
            let key = (binding.group, binding.binding);
            let size = u64::from(layouter[global.ty].size);
            match sizes.iter_mut().find(|(at, _)| *at == key) {
                Some((_, known)) => *known = (*known).max(size),
                None => sizes.push((key, size)),
            }
        }
    }
    sizes
}

/// None in a browser, where naga doesn't parse the sources; the browser's
/// WebGPU keeps reads inside the bound ranges itself.
#[cfg(target_family = "wasm")]
fn buffer_sizes(_: &[&str]) -> Vec<((u32, u32), u64)> {
    Vec::new()
}

fn trim_string(value: &str) -> String {
    value.trim_matches(|c: char| c.is_ascii() && is_space(c as u8)).to_owned()
}

fn webgpu_blend_operation(equation: ColorBlendEquationType) -> wgpu::BlendOperation {
    match equation {
        ColorBlendEquationType::Add => wgpu::BlendOperation::Add,
        ColorBlendEquationType::Subtract => wgpu::BlendOperation::Subtract,
        ColorBlendEquationType::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
        ColorBlendEquationType::Min => wgpu::BlendOperation::Min,
        ColorBlendEquationType::Max => wgpu::BlendOperation::Max,
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
/// layouts of the bindings its stages declare.
pub(crate) struct ShaderProgram {
    shader_name: String,
    device: wgpu::Device,
    vertex_shader_module: wgpu::ShaderModule,
    fragment_shader_module: wgpu::ShaderModule,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    bind_group_order: Vec<u32>,
    /// The group that holds only `drawIndex` (see
    /// [`index_group`](Self::index_group)).
    index_group: Option<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    /// What the program binds, by which its inputs have their locations.
    program: &'static Program,
    bindings_per_group: Vec<Vec<BindingInfo>>,
    /// The bytes of each buffer the sources read (`buffer_sizes`).
    buffer_sizes: Vec<((u32, u32), u64)>,
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
    /// `program`, from `sources`, with layout `index_layout` for the group
    /// of `drawIndex` (see [`index_group`](Self::index_group)).
    ///
    /// Its pipelines set the stages' `DEVICE_PIXEL_RATIO` override to
    /// `pixel_ratio`.
    ///
    /// Fails if either source is empty or doesn't compile.
    pub(crate) fn new(
        device: &wgpu::Device,
        index_layout: &wgpu::BindGroupLayout,
        sources: StageSources,
        program: &'static Program,
        pixel_ratio: f32,
    ) -> Result<Self, GpuError> {
        let name = program.name;
        let StageSources { stages: [vertex_source, fragment_source], bindings: binding_infos, index_group } = sources;
        let (vertex_source, fragment_source) = (vertex_source.as_str(), fragment_source.as_str());

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

        let groups = BindGroups::new(device, binding_infos, index_group, index_layout);
        let layouts: Vec<Option<&wgpu::BindGroupLayout>> = groups.layouts.iter().map(Some).collect();
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
            buffer_sizes: buffer_sizes(&[vertex_source, fragment_source]),
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

    /// The bytes of the buffer at `binding` of `group` that the sources
    /// read: of one that ends in an array of any length, up to its first
    /// element. 0 where it isn't known, as in a browser.
    pub(crate) fn buffer_size(&self, group: u32, binding: u32) -> u64 {
        self.buffer_sizes.iter().find(|&&(at, _)| at == (group, binding)).map_or(0, |&(_, size)| size)
    }

    pub(crate) fn bind_group_layout(&self, group: u32) -> Option<&wgpu::BindGroupLayout> {
        self.bind_group_order.iter().position(|&g| g == group).map(|i| &self.bind_group_layouts[i])
    }

    /// The groups the sources declare, in the order of the pipeline layout's
    /// slots.
    pub(crate) fn bind_group_order(&self) -> &[u32] {
        &self.bind_group_order
    }

    /// The group of `drawIndex`, if the stages read it
    /// ([`StageSources::index_group`]).
    pub(crate) fn index_group(&self) -> Option<u32> {
        self.index_group
    }

    pub(crate) fn name(&self) -> &str {
        &self.shader_name
    }

    /// The slot of the program's texture whose texture or sampler the
    /// shader declares at `binding` of the texture group, as
    /// [`location`] places them.
    pub(crate) fn texture_at(&self, binding: u32) -> Option<usize> {
        let slot = binding as usize / 2;
        (slot < self.program.textures.len()).then_some(slot)
    }

    /// The shader location of the input `id` that steps by `step`.
    fn location(&self, step: VertexStep, id: u32) -> Option<u32> {
        self.program.location(step, id as usize).map(|location| location as u32)
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
                        let shader_location = self.location(layout.step, attribute.id).ok_or_else(|| {
                            GpuError::Invalid(format!("{} has no attribute {}", self.name(), attribute.id))
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
            cull_mode: state.cull.enabled.then_some(match state.cull.side {
                CullFaceSideType::Front => wgpu::Face::Front,
                CullFaceSideType::Back => wgpu::Face::Back,
            }),
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
    /// Sources that declare no binding get a group 0 with a uniform buffer
    /// at binding 0.
    fn new(
        device: &wgpu::Device,
        mut binding_infos: Vec<BindingInfo>,
        index_group: Option<u32>,
        index_layout: &wgpu::BindGroupLayout,
    ) -> BindGroups {
        if binding_infos.is_empty() {
            binding_infos.push(BindingInfo {
                group: 0,
                binding: 0,
                type_: BindingType::UniformBuffer,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            });
        }

        let mut grouped: VecMap<u32, Vec<BindingInfo>> = VecMap::new();
        for info in binding_infos {
            grouped.entry(info.group).or_default().push(info);
        }
        // A pass binds a group at its place in the pipeline's layout, so a
        // group that a stage declares nothing of, below one it does, is
        // there and empty: the textures of a program without buffers.
        for group in 0..grouped.keys().copied().max().unwrap_or_default() {
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

/// Compiles the program of `key` from its WGSL ([`stage_sources`]). Fails
/// with [`GpuError::Unsupported`] for a program without WGSL.
pub(crate) fn compile(
    device: &wgpu::Device,
    index_layout: &wgpu::BindGroupLayout,
    key: &ShaderKey,
) -> Result<ShaderProgram, GpuError> {
    let sources = stage_sources(key)?;
    let pixel_ratio = key
        .define_values()
        .find(|&(name, _)| name == PIXEL_RATIO_OVERRIDE)
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(1.0);
    ShaderProgram::new(device, index_layout, sources, key.program(), pixel_ratio)
}

/// A program's stages in WGSL, and what they bind.
pub(crate) struct StageSources {
    /// The vertex and fragment stages.
    pub(crate) stages: [String; 2],
    /// The bindings that the stages declare, each once, visible to the
    /// stages that declare it.
    pub(crate) bindings: Vec<BindingInfo>,
    /// The group of `drawIndex`, the one after the program's others, if a
    /// stage declares it: its layout is the index table's, and a draw binds
    /// it at the dynamic offset of its index, so that its other groups bind
    /// only what it shares with the layer's other draws.
    pub(crate) index_group: Option<u32>,
}

/// The vertex and fragment stages of `key`'s program: the common prelude,
/// the structs of the engine's blocks and the program's, the declarations of
/// what the stage names of what it may read ([`declared`]), the program's
/// own prelude and the stage with its properties written out
/// ([`wgsl::expand`]), preprocessed with the key's defines. Fails with
/// [`GpuError::Unsupported`] for a program without WGSL, and as
/// [`wgsl::expand`] does.
pub(crate) fn stage_sources(key: &ShaderKey) -> Result<StageSources, GpuError> {
    let program = key.program();
    let source: &Wgsl =
        program.sources.wgsl.ok_or_else(|| GpuError::Unsupported(format!("no WGSL for {}", program.name)))?;
    // The common prelude goes through the preprocessor with the program, so
    // its `#ifdef`s see the defines.
    let shader_prelude = source.prelude;
    let blocks = wgsl::blocks(program);
    // WGSL reads no define's value; the pixel ratio's is an override's.
    let defines: HashMap<String, bool> = key.define_values().map(|(name, _)| (name.to_owned(), true)).collect();
    let [vertex, fragment] = wgsl::expand(program, source)?;
    let stages = [(Stage::Vertex, vertex), (Stage::Fragment, fragment)];
    let declared =
        stages.each_ref().map(|(stage, text)| declared(program, *stage, &format!("{shader_prelude}{text}"), &defines));
    let all = || declared.iter().flatten();
    let index_group = all().any(|binding| matches!(binding, Binding::DrawIndex)).then(|| {
        let textures = all().any(|binding| matches!(binding, Binding::Texture(..) | Binding::Sampler(..)));
        if textures { TEXTURE_GROUP + 1 } else { BUFFER_GROUP + 1 }
    });
    let mut bindings: Vec<BindingInfo> = Vec::new();
    let [vertex, fragment] = [0, 1].map(|index| {
        let (stage, text) = &stages[index];
        let visibility = match stage {
            Stage::Vertex => wgpu::ShaderStages::VERTEX,
            Stage::Fragment => wgpu::ShaderStages::FRAGMENT,
        };
        let mut declarations = String::new();
        for binding in &declared[index] {
            declarations += &declaration(binding, index_group.unwrap_or_default());
            declarations.push('\n');
            let (group, at) = location(binding, index_group.unwrap_or_default());
            match bindings.iter_mut().find(|info| (info.group, info.binding) == (group, at)) {
                Some(info) => info.visibility |= visibility,
                None => bindings.push(BindingInfo { group, binding: at, type_: binding_type(binding), visibility }),
            }
        }
        preprocess_wgsl(&format!("{PRELUDE}\n{blocks}{declarations}{shader_prelude}{text}"), &defines)
    });
    Ok(StageSources { stages: [vertex, fragment], bindings, index_group })
}

/// What `text`, a stage and the program's prelude, names outside its
/// comments of what `stage` may read ([`Program::bindings`]): of the draw's
/// index, the buffers that have blocks, and the textures and samplers,
/// which a draw binds in every variant, those it names in any variant, and
/// of the vertex buffers, which a draw binds where it has them, those it
/// names in the variant of `defines`. WebGPU lays out the bindings that a
/// variant declares, whether it reads them or not.
fn declared<'p>(program: &'p Program, stage: Stage, text: &str, defines: &HashMap<String, bool>) -> Vec<Binding<'p>> {
    let named: HashSet<&str> = identifiers(text).collect();
    let vertices = |binding: &Binding<'_>| matches!(binding, Binding::Buffer(buffer) if matches!(buffer.kind, BufferKind::Vertices { .. }));
    let preprocessed =
        program.bindings(stage).any(|binding| vertices(&binding)).then(|| preprocess_wgsl(text, defines));
    let named_in_variant: HashSet<&str> = preprocessed.as_deref().map(identifiers).into_iter().flatten().collect();
    program
        .bindings(stage)
        .filter(|binding| {
            let named = if vertices(binding) { &named_in_variant } else { &named };
            named.contains(&*binding.name())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mln_gfx::gfx_types::AttributeDataType::{Float, Short4};
    use mln_gfx::shaders::layer_ubo::ViewUBO;
    use mln_gfx::shaders::program::{
        DRAW_PROJECTION_BUFFER, PROJECTION_BUFFER, ProgramBuffer, ProgramTexture, Projection, Sources, VIEW_BUFFER,
    };

    use super::*;

    fn declared_text(program: &Program, stage: Stage, text: &str, defines: &[&str]) -> String {
        let defines = defines.iter().map(|&name| (name.to_owned(), true)).collect();
        declared(program, stage, text, &defines).iter().map(|binding| declaration(binding, 2) + "\n").collect()
    }

    #[test]
    fn the_engines_bindings_are_at_their_ids() {
        let declared = |binding| declaration(&binding, 2);
        assert_eq!(declared(Binding::DrawIndex), "@group(2) @binding(1) var<uniform> drawIndex: u32;");
        assert_eq!(declared(Binding::Buffer(&VIEW_BUFFER)), "@group(0) @binding(0) var<uniform> view: ViewUBO;");
        assert_eq!(
            declared(Binding::Buffer(&PROJECTION_BUFFER)),
            "@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;"
        );
        assert_eq!(
            declared(Binding::Buffer(&DRAW_PROJECTION_BUFFER)),
            "@group(0) @binding(4) var<uniform> drawProjection: ProjectionUBO;"
        );
    }

    #[test]
    fn a_stage_declares_the_buffers_it_names() {
        static PROGRAM: Program = Program {
            name: "pulled",
            buffers: &[ProgramBuffer::uniform::<ViewUBO>("props", 0), ProgramBuffer::vertices("paint", 0, Float)],
            attributes: &[],
            instance_attributes: &[],
            textures: &[],
            properties: &[],
            projection: Projection::PerDraw,
            sources: Sources::NONE,
        };
        let text = "let p = projectionVector[drawIndex]; // view\n#ifdef PAINT\nlet x = paint[0];\n#endif\nprops";
        let declared = |defines| declared_text(&PROGRAM, Stage::Vertex, text, defines);
        let [props, paint] =
            [&PROGRAM.buffers[0], &PROGRAM.buffers[1]].map(|buffer| declaration(&Binding::Buffer(buffer), 2));
        let engine = format!(
            "{}\n{}\n{props}\n",
            declaration(&Binding::DrawIndex, 2),
            declaration(&Binding::Buffer(&PROJECTION_BUFFER), 2)
        );
        assert_eq!(declared(&[]), engine);
        assert_eq!(declared(&["PAINT"]), format!("{engine}{paint}\n"));
        // A vertex buffer is the vertex stage's alone.
        assert_eq!(declared_text(&PROGRAM, Stage::Fragment, text, &["PAINT"]), engine);
    }

    #[test]
    fn a_vertex_buffer_is_an_array_of_its_words() {
        assert_eq!(
            declaration(&Binding::Buffer(&ProgramBuffer::vertices("walls", 0, Short4)), 2),
            "@group(0) @binding(7) var<storage, read> walls: array<u32>;"
        );
        assert_eq!(
            declaration(&Binding::Buffer(&ProgramBuffer::vertices("paint", 1, Float)), 2),
            "@group(0) @binding(8) var<storage, read> paint: array<f32>;"
        );
    }

    #[test]
    fn a_fragment_stage_declares_the_textures_and_samplers_it_names_at_their_slots() {
        static PROGRAM: Program = Program {
            name: "two textures",
            buffers: &[],
            attributes: &[],
            instance_attributes: &[],
            textures: &[ProgramTexture::new("first"), ProgramTexture::unfilterable("second")],
            properties: &[],
            projection: Projection::None,
            sources: Sources::NONE,
        };
        let text = "textureSample(second, first_sampler, uv)";
        assert_eq!(
            declared_text(&PROGRAM, Stage::Fragment, text, &[]),
            "@group(1) @binding(0) var first_sampler: sampler;\n@group(1) @binding(3) var second: texture_2d<f32>;\n"
        );
        assert_eq!(declared_text(&PROGRAM, Stage::Vertex, text, &[]), "");
        let types =
            declared(&PROGRAM, Stage::Fragment, text, &HashMap::default()).iter().map(binding_type).collect::<Vec<_>>();
        assert_eq!(types, [BindingType::Sampler, BindingType::UnfilterableTexture]);
    }
}
