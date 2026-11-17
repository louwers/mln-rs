//! The programs, the WGSL that `mln_shaders` links, and their pipelines.
//! Shader modules and pipelines are created inside a validation error scope,
//! so that a rejected module or pipeline is an `Err`. On a browser's WebGPU
//! the scope is skipped (see [`validated`]).

use std::borrow::Cow;

use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{FixedState, GpuError, ShaderKey, VertexBufferLayout, VertexStep};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use mln_shaders::{ResourceKind, Stage};
use mln_vec_map::VecMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingType {
    UniformBuffer,
    ReadOnlyStorageBuffer,
    Sampler,
    Texture,
    /// A texture that the shader only reads with `textureLoad`, such as an
    /// `Rgba32Float` one.
    UnfilterableTexture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BindingInfo {
    pub(crate) group: u32,
    pub(crate) binding: u32,
    pub(crate) type_: BindingType,
    pub(crate) visibility: wgpu::ShaderStages,
    /// The renderer's id of the buffer, or of the texture a texture or
    /// sampler binding takes.
    pub(crate) id: u32,
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

/// A program: its module, with both entry points, and its bind group
/// layouts.
pub(crate) struct ShaderProgram {
    shader_name: &'static str,
    device: wgpu::Device,
    module: wgpu::ShaderModule,
    bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    bind_group_order: Vec<u32>,
    /// The group that holds only `globalIndex` (see
    /// [`index_group`](Self::index_group)).
    index_group: Option<u32>,
    pipeline_layout: wgpu::PipelineLayout,
    /// The attributes the vertex stage reads, by id: each at the shader
    /// location of its id.
    attributes: Vec<u32>,
    bindings_per_group: Vec<Vec<BindingInfo>>,
    /// The bytes of each buffer the program reads, by group and binding.
    buffer_sizes: Vec<((u32, u32), u64)>,
}

impl ShaderProgram {
    /// The program of `program`, its WGSL linked by `mln_shaders`: one
    /// module with both entry points, whose bindings, group of the draw's
    /// index and buffer sizes `program` reports.
    pub(crate) fn new(
        device: &wgpu::Device,
        index_layout: &wgpu::BindGroupLayout,
        program: &mln_shaders::Program,
    ) -> Result<Self, GpuError> {
        let name = program.name;
        let module = validated(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: source(program) })
        })
        .map_err(|error| GpuError::Invalid(format!("shader {name}: {error}")))?;
        let binding_infos = program
            .resources
            .iter()
            .map(|resource| {
                let mut visibility = wgpu::ShaderStages::empty();
                visibility.set(wgpu::ShaderStages::VERTEX, resource.vertex);
                visibility.set(wgpu::ShaderStages::FRAGMENT, resource.fragment);
                BindingInfo {
                    group: resource.binding.group,
                    binding: resource.binding.binding,
                    type_: match resource.kind {
                        ResourceKind::UniformBuffer => BindingType::UniformBuffer,
                        ResourceKind::StorageBuffer => BindingType::ReadOnlyStorageBuffer,
                        ResourceKind::Texture { filterable: true } => BindingType::Texture,
                        ResourceKind::Texture { filterable: false } => BindingType::UnfilterableTexture,
                        ResourceKind::Sampler => BindingType::Sampler,
                    },
                    visibility,
                    id: resource.id,
                }
            })
            .collect();
        let groups = BindGroups::new(device, binding_infos, program.index_group, index_layout);
        let layouts: Vec<Option<&wgpu::BindGroupLayout>> = groups.layouts.iter().map(Some).collect();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(name),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        let buffer_sizes = program
            .resources
            .iter()
            .filter(|resource| matches!(resource.kind, ResourceKind::UniformBuffer | ResourceKind::StorageBuffer))
            .map(|resource| ((resource.binding.group, resource.binding.binding), resource.size))
            .collect();
        Ok(ShaderProgram {
            shader_name: name,
            device: device.clone(),
            module,
            bind_group_layouts: groups.layouts,
            bind_group_order: groups.order,
            index_group: program.index_group,
            pipeline_layout,
            attributes: program.inputs.iter().map(|input| input.location).collect(),
            bindings_per_group: groups.bindings,
            buffer_sizes,
        })
    }

    /// The bindings of `group`, by binding.
    pub(crate) fn binding_infos(&self, group: u32) -> &[BindingInfo] {
        self.bind_group_order.iter().position(|&g| g == group).map_or(&[], |i| &self.bindings_per_group[i])
    }

    /// The bytes of the buffer at `binding` of `group` that the program
    /// reads: of one element of a layer's buffer of an element for each
    /// drawable.
    pub(crate) fn buffer_size(&self, group: u32, binding: u32) -> u64 {
        self.buffer_sizes.iter().find(|&&(at, _)| at == (group, binding)).map_or(0, |&(_, size)| size)
    }

    pub(crate) fn bind_group_layout(&self, group: u32) -> Option<&wgpu::BindGroupLayout> {
        self.bind_group_order.iter().position(|&g| g == group).map(|i| &self.bind_group_layouts[i])
    }

    /// The groups the program uses, in the order of the pipeline layout's
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
        self.shader_name
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
                // An attribute that the variant doesn't read isn't described.
                layout
                    .attributes
                    .iter()
                    .filter(|attribute| self.attributes.contains(&attribute.id))
                    .map(|attribute| {
                        let format = vertex_format(attribute.format).ok_or_else(|| {
                            GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format))
                        })?;
                        Ok(wgpu::VertexAttribute {
                            format,
                            offset: u64::from(attribute.offset),
                            shader_location: attribute.id,
                        })
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
                label: Some(self.shader_name),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some(Stage::Vertex.entry_point()),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                primitive,
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some(Stage::Fragment.entry_point()),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
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
            BindingType::Texture => texture(true),
            BindingType::UnfilterableTexture => texture(false),
        }
    }
}

/// `program` for wgpu: natively, the module naga parsed already; in a
/// browser, its WGSL.
#[cfg(not(target_family = "wasm"))]
fn source(program: &mln_shaders::Program) -> wgpu::ShaderSource<'_> {
    wgpu::ShaderSource::Naga(Cow::Owned(program.module.clone()))
}

#[cfg(target_family = "wasm")]
fn source(program: &mln_shaders::Program) -> wgpu::ShaderSource<'_> {
    wgpu::ShaderSource::Wgsl(Cow::Borrowed(&program.wgsl))
}

/// Compiles the program of `key`.
pub(crate) fn compile(
    device: &wgpu::Device,
    index_layout: &wgpu::BindGroupLayout,
    key: &ShaderKey,
) -> Result<ShaderProgram, GpuError> {
    let program = mln_shaders::program_of(key, mln_shaders::Target::WebGpu)
        .map_err(|error| GpuError::Invalid(format!("{:?}: {error}", key.program())))?;
    ShaderProgram::new(device, index_layout, &program)
}
