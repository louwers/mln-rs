//! WGSL programs and their pipelines. Shader modules and pipelines are
//! created inside a validation error scope, so that a WGSL compile error or a
//! rejected pipeline is an `Err`. On a browser's WebGPU the scope is skipped
//! (see [`validated`]).

use std::borrow::Cow;

use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{FixedState, GpuError, ShaderKey, VertexBufferLayout, VertexStep};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::wgsl::Wgsl;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use mln_util::hash::HashMap;
use mln_vec_map::VecMap;

use crate::shaders::wgsl_preprocessor::{is_space, preprocess_wgsl};
use crate::shaders::wgsl_sources::{PRELUDE, RESERVED_BINDINGS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingType {
    UniformBuffer,
    ReadOnlyStorageBuffer,
    StorageBuffer,
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
/// layouts as its sources declare them.
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
    /// What the program binds, by which its inputs have their locations.
    program: &'static Program,
    /// The texture slot of each binding of a texture or a sampler
    /// ([`texture_slots`]).
    texture_slots: Vec<(u32, usize)>,
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
    /// `program`, from `vertex_source` and `fragment_source`, its WGSL
    /// preprocessed already.
    ///
    /// A `globalIndex` the sources declare in group 0 moves to a group of its
    /// own, with layout `index_layout` (see
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
        pixel_ratio: f32,
    ) -> Result<Self, GpuError> {
        let name = program.name;
        let index_group = index_group_of(&[vertex_source, fragment_source]);
        let (vertex_source, fragment_source) = match index_group {
            Some(group) => (move_global_index(vertex_source, group), move_global_index(fragment_source, group)),
            None => (Cow::Borrowed(vertex_source), Cow::Borrowed(fragment_source)),
        };
        let (vertex_source, fragment_source) = (vertex_source.as_ref(), fragment_source.as_ref());

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

        let mut binding_infos = Vec::new();
        analyze_shader_bindings(&mut binding_infos, vertex_source, wgpu::ShaderStages::VERTEX);
        analyze_shader_bindings(&mut binding_infos, fragment_source, wgpu::ShaderStages::FRAGMENT);
        let texture_slots = texture_slots(&binding_infos);
        for info in &mut binding_infos {
            let sampler = match info.type_ {
                BindingType::Sampler => true,
                BindingType::Texture => false,
                _ => continue,
            };
            let slot = texture_slots.iter().find(|&&(binding, _)| binding == info.binding).map(|&(_, slot)| slot);
            if slot.and_then(|slot| program.textures.get(slot)).is_some_and(|texture| !texture.filterable) {
                info.type_ = if sampler { BindingType::NonFilteringSampler } else { BindingType::UnfilterableTexture };
            }
        }
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
            texture_slots,
            bindings_per_group: groups.bindings,
            buffer_sizes: buffer_sizes(&[vertex_source, fragment_source]),
            vertex_constants: constants(vertex_source, pixel_ratio),
            fragment_constants: constants(fragment_source, pixel_ratio),
        })
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

    /// The group that holds only `globalIndex`, the index of the draw's
    /// entries in the layer's UBO vectors, at binding `ID_GLOBAL_UBO_INDEX`.
    /// Its layout is the index table's, and a draw binds it with the dynamic
    /// offset of its index, so its other groups bind only what it shares
    /// with the layer's other draws.
    pub(crate) fn index_group(&self) -> Option<u32> {
        self.index_group
    }

    pub(crate) fn name(&self) -> &str {
        &self.shader_name
    }

    /// The slot of the program's texture whose texture or sampler the
    /// shader declares at `binding`.
    pub(crate) fn texture_at(&self, binding: u32) -> Option<usize> {
        let slot = self.texture_slots.iter().find(|&&(at, _)| at == binding).map(|&(_, slot)| slot)?;
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
            BindingType::StorageBuffer => buffer(wgpu::BufferBindingType::Storage { read_only: false }),
            BindingType::Sampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            BindingType::NonFilteringSampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            BindingType::Texture => texture(true),
            BindingType::UnfilterableTexture => texture(false),
        }
    }
}

/// The texture slot of each texture and sampler binding of `infos`: the
/// textures take the slots in binding order, and a sampler is the one of the
/// texture at the binding after it.
fn texture_slots(infos: &[BindingInfo]) -> Vec<(u32, usize)> {
    let is_texture =
        |info: &&BindingInfo| matches!(info.type_, BindingType::Texture | BindingType::UnfilterableTexture);
    let mut textures: Vec<u32> = infos.iter().filter(is_texture).map(|info| info.binding).collect();
    textures.sort_unstable();
    textures.dedup();
    let mut slots: Vec<(u32, usize)> = textures.iter().enumerate().map(|(slot, &binding)| (binding, slot)).collect();
    for info in infos.iter().filter(|info| info.type_ == BindingType::Sampler) {
        if let Some(slot) = textures.iter().position(|&binding| binding == info.binding + 1) {
            slots.push((info.binding, slot));
        }
    }
    slots
}

/// Adds the bindings of the `@group(N) @binding(M) var<...> name: type`
/// declarations in `source`, which `stage` reads, to `binding_infos`. A
/// text scan, not a parse: a texture only `textureLoad` reads is
/// unfilterable.
fn analyze_shader_bindings(binding_infos: &mut Vec<BindingInfo>, source: &str, stage: wgpu::ShaderStages) {
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
                let uses_texture_load = source.contains(&format!("textureLoad({var_name}"));
                let uses_texture_sample = source.contains(&format!("textureSample({var_name}"));
                type_ = if uses_texture_load && !uses_texture_sample {
                    BindingType::UnfilterableTexture
                } else {
                    BindingType::Texture
                };
            }
        }

        if let Some(existing) = binding_infos.iter_mut().find(|info| info.group == group && info.binding == binding) {
            existing.visibility |= stage;
            if existing.type_ == BindingType::UniformBuffer && type_ != BindingType::UniformBuffer {
                existing.type_ = type_;
            }
        } else {
            binding_infos.push(BindingInfo { group, binding, type_, visibility: stage });
        }

        search_pos = type_end;
    }
}

/// Compiles the program of `key` from its WGSL: the common prelude, the
/// declarations of the engine's buffers that the stage reads, the program's
/// own prelude and each stage, preprocessed with the key's defines. Fails
/// with [`GpuError::Unsupported`] for a program without WGSL.
pub(crate) fn compile(
    device: &wgpu::Device,
    index_layout: &wgpu::BindGroupLayout,
    key: &ShaderKey,
) -> Result<ShaderProgram, GpuError> {
    let program = key.program();
    let source: &Wgsl =
        program.sources.wgsl.ok_or_else(|| GpuError::Unsupported(format!("no WGSL for {}", program.name)))?;
    // The common prelude goes through the preprocessor with the program, so
    // its `#ifdef`s see the defines.
    let shader_prelude = source.prelude.unwrap_or("");
    let assemble = |stage: Option<&str>| {
        let stage = stage.unwrap_or("");
        format!("{PRELUDE}\n{}{shader_prelude}{stage}", reserved_bindings(&[shader_prelude, stage]))
    };
    // WGSL reads no define's value; the pixel ratio's is an override's.
    let defines: HashMap<String, bool> = key.define_values().map(|(name, _)| (name.to_owned(), true)).collect();
    let vertex_source = preprocess_wgsl(&assemble(source.vertex), &defines);
    let fragment_source = preprocess_wgsl(&assemble(source.fragment), &defines);
    let pixel_ratio = key
        .define_values()
        .find(|&(name, _)| name == PIXEL_RATIO_OVERRIDE)
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(1.0);

    ShaderProgram::new(device, index_layout, &vertex_source, &fragment_source, program, pixel_ratio)
}

/// The declarations of the engine's buffers that `texts`, a stage and the
/// program's prelude, read: of each one whose variable they name outside
/// their comments.
fn reserved_bindings(texts: &[&str]) -> String {
    let named = |name: &str| texts.iter().any(|text| identifiers(text).any(|identifier| identifier == name));
    let mut declarations = String::new();
    for (_, declaration) in RESERVED_BINDINGS.iter().filter(|(name, _)| named(name)) {
        declarations.push_str(declaration);
        declarations.push('\n');
    }
    declarations
}

/// The identifiers of `text`, outside its `//` comments.
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .flat_map(|code| code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
        .filter(|word| !word.is_empty())
}

/// How the backend declares `globalIndex`.
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
    use mln_gfx::shaders::shader_defines::{ID_GLOBAL_UBO_INDEX, ID_PROJECTION_UBO, ID_VIEW_UBO};

    use super::*;

    #[test]
    fn the_engines_buffers_bind_at_their_ids() {
        let ids = [ID_VIEW_UBO, ID_GLOBAL_UBO_INDEX, ID_PROJECTION_UBO];
        for (&(_, declaration), id) in RESERVED_BINDINGS.iter().zip(ids) {
            assert!(declaration.starts_with(&format!("@group(0) @binding({id}) ")), "{declaration}");
        }
        assert!(RESERVED_BINDINGS[1].1.starts_with(GLOBAL_INDEX_DECLARATION));
    }

    #[test]
    fn a_stage_declares_the_engines_buffers_it_names() {
        let declared = reserved_bindings(&["", "let p = projectionVector[globalIndex.value]; // view"]);
        assert_eq!(declared, format!("{}\n{}\n", RESERVED_BINDINGS[1].1, RESERVED_BINDINGS[2].1));
    }

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
