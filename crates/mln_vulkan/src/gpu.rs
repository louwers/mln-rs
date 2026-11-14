//! The [`Gpu`] interface on Vulkan.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::rc::{Rc, Weak};

use ash::vk;
use gpu_allocator::MemoryLocation;
use mln_gfx::color_mode::BlendFunction;
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, Gpu, GpuError, Limits, LoadOp, Pass,
    PipelineId, PipelineKey, RenderCommand, SamplerId, ShaderKey, TargetFormats, TextureDesc, TextureFormat, TextureId,
    TextureRegion, TextureUsage, VertexStep,
};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, LAYER_SSBO_START_ID};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{DepthFunctionType, TextureFilterType, TextureWrapType};
use mln_util::size::Size;
use slotmap::SlotMap;

use crate::buffer_resource::BufferAllocation;
use crate::context::{build_descriptor_set_layout, build_pipeline_layout};
use crate::descriptor_set::DescriptorPoolGrowable;
use crate::device::Device;
use crate::pipeline::PipelineInfo;
use crate::renderer_backend::BackendImpl;
use crate::shaders::compiler::{self, Stage};
use crate::shaders::layout::{
    DescriptorSetType, IMAGE_SAMPLER_START, IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT, UBO_INDEX_PUSH_CONSTANT_SIZE,
    image_sampler_binding, image_texture_binding,
};
use crate::shaders::shader_source::{AttributeInfo, TextureInfo, shader_source};
use crate::texture2d::{ImageAllocation, transition_image};

/// The sets each descriptor pool starts with room for.
const DESCRIPTOR_POOL_SIZE: u32 = 256;

/// The largest write that goes into the command buffer itself
/// (`vkCmdUpdateBuffer`) rather than through a staging buffer.
const MAX_INLINE_WRITE: usize = 65536;

struct Buffer {
    allocation: BufferAllocation,
    size: u64,
    usage: BufferUsage,
}

struct Texture {
    image: ImageAllocation,
    format: TextureFormat,
    vk_format: vk::Format,
    size: Size,
    usage: TextureUsage,
    aspect: vk::ImageAspectFlags,
    /// The layout the image is in between commands.
    layout: vk::ImageLayout,
}

/// What a shader binding reads, by the shaders' ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    Buffer(u32),
    Texture(u32),
    Sampler(u32),
}

/// A compiled program.
pub(crate) struct Shader {
    name: &'static str,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    /// The bindings its stages use, by descriptor set.
    bindings: [Vec<(u32, Binding)>; DescriptorSetType::COUNT],
    /// The location of each per-vertex attribute, by id.
    attributes: &'static [AttributeInfo],
    /// The location of each per-instance attribute, by id.
    instance_attributes: &'static [AttributeInfo],
}

impl Shader {
    /// The location of the per-vertex or per-instance attribute `id`.
    fn location(&self, step: VertexStep, id: u32) -> Result<u32, GpuError> {
        let attributes = match step {
            VertexStep::Vertex => self.attributes,
            VertexStep::Instance => self.instance_attributes,
        };
        attributes
            .iter()
            .find(|attribute| attribute.id == id as usize)
            .map(|attribute| attribute.index as u32)
            .ok_or_else(|| GpuError::Invalid(format!("{} has no {step:?} attribute {id}", self.name)))
    }
}

struct Pipeline {
    key: PipelineKey,
    pipeline: vk::Pipeline,
    shader: Rc<Shader>,
}

/// A resource a descriptor set binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Resource {
    Buffer(BufferId, u64, u64),
    Texture(TextureId),
    Sampler(SamplerId),
}

/// What a descriptor set is made from. The bindings its shader doesn't use
/// bind placeholders.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SetKey {
    set: DescriptorSetType,
    resources: Vec<(u32, Resource)>,
}

/// An attachment of a render pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct AttachmentKey {
    format: vk::Format,
    /// The layout the image is in before and after the pass.
    layout: vk::ImageLayout,
    /// Whether the pass keeps the color or depth, rather than clearing it.
    load: bool,
    /// Whether the pass keeps the stencil, for a format with one.
    stencil_load: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RenderPassKey {
    color: Option<AttachmentKey>,
    depth_stencil: Option<AttachmentKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FramebufferKey {
    render_pass: vk::RenderPass,
    color: Option<TextureId>,
    depth_stencil: Option<TextureId>,
}

/// A submit the GPU may still be running.
struct Submission {
    serial: u64,
    fence: vk::Fence,
    command_buffers: Vec<vk::CommandBuffer>,
}

/// Something to destroy once the submits that may use it are done.
enum Garbage {
    Buffer(BufferAllocation),
    Image(ImageAllocation),
    Sampler(vk::Sampler),
    Pipeline(vk::Pipeline),
    Framebuffer(vk::Framebuffer),
    DescriptorSet(DescriptorSetType, usize, vk::DescriptorSet),
}

/// A [`Gpu`] on a Vulkan [`Device`].
///
/// Each submit is one batch on the device's queue. Writes and new textures
/// record their commands into a command buffer that runs before the next
/// submit's passes. A destroyed resource goes once the submits that may use
/// it have finished, which the next submit checks for.
///
/// A draw binds a descriptor set for each set of the shared pipeline
/// layout that its shader uses (`shaders::layout`), made at the first draw
/// that binds its resources and kept until one of them is destroyed.
/// `globalIndex` is a push constant.
pub struct VulkanGpu {
    backend: Rc<BackendImpl>,
    set_layouts: [vk::DescriptorSetLayout; DescriptorSetType::COUNT],
    pipeline_layout: vk::PipelineLayout,
    descriptor_pools: [DescriptorPoolGrowable; DescriptorSetType::COUNT],
    /// The format of `Depth24PlusStencil8` textures.
    depth24_stencil8: vk::Format,
    /// Bound where a descriptor set has a binding its shader doesn't use.
    placeholder_buffer: BufferAllocation,
    placeholder_image: ImageAllocation,
    placeholder_sampler: vk::Sampler,

    buffers: SlotMap<BufferId, Buffer>,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, vk::Sampler>,
    shaders: HashMap<ShaderKey, Rc<Shader>>,
    pipelines: SlotMap<PipelineId, Rc<Pipeline>>,
    /// The pipelines by key, so that ids of equal keys share one.
    pipeline_cache: HashMap<PipelineKey, Weak<Pipeline>>,
    bind_groups: SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    descriptor_sets: HashMap<SetKey, (vk::DescriptorSet, usize)>,
    render_passes: HashMap<RenderPassKey, vk::RenderPass>,
    framebuffers: HashMap<FramebufferKey, vk::Framebuffer>,

    /// The commands of the writes since the last submit.
    uploads: Option<vk::CommandBuffer>,
    /// The serial of the next submit.
    next_serial: u64,
    /// The serial of the last submit known to have finished.
    finished_serial: u64,
    in_flight: VecDeque<Submission>,
    free_command_buffers: Vec<vk::CommandBuffer>,
    free_fences: Vec<vk::Fence>,
    /// What to destroy once the submit of each serial has finished.
    garbage: Vec<(u64, Garbage)>,
}

fn vk_error(what: &str) -> impl FnOnce(vk::Result) -> GpuError + '_ {
    move |error| GpuError::Invalid(format!("{what}: {error}"))
}

fn has_depth(format: TextureFormat) -> bool {
    format.bytes_per_texel().is_none()
}

fn aspect(format: TextureFormat) -> vk::ImageAspectFlags {
    if format.has_stencil() {
        vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
    } else if has_depth(format) {
        vk::ImageAspectFlags::DEPTH
    } else {
        vk::ImageAspectFlags::COLOR
    }
}

/// The layout a texture of `usage` stays in between commands.
fn resting_layout(format: TextureFormat, usage: TextureUsage) -> vk::ImageLayout {
    if usage.sampled {
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
    } else if usage.render_target && has_depth(format) {
        vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
    } else if usage.render_target {
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
    } else {
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL
    }
}

fn whole_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: aspect,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

fn clear_color(color: mln_util::Color) -> vk::ClearValue {
    vk::ClearValue { color: vk::ClearColorValue { float32: [color.r, color.g, color.b, color.a] } }
}

/// The resource of `binding` in `set` of the shared layout, by the shaders'
/// ids: the inverse of `shaders::layout`'s `*_binding` functions.
fn binding_of(
    name: &str,
    textures: &[TextureInfo],
    set: u32,
    binding: u32,
) -> Result<(DescriptorSetType, Binding), GpuError> {
    let texture_id = |index: u32| {
        textures
            .iter()
            .find(|texture| texture.index == index as usize)
            .map(|texture| texture.id as u32)
            .ok_or_else(|| GpuError::Invalid(format!("{name}: no texture at index {index} of set {set}")))
    };
    let start = LAYER_SSBO_START_ID as u32;
    let storage = LAYER_STORAGE_COUNT as u32;
    let resource = match DescriptorSetType::ALL.get(set as usize) {
        Some(DescriptorSetType::Global) => (DescriptorSetType::Global, Binding::Buffer(binding)),
        Some(DescriptorSetType::Layer) if binding < storage => {
            (DescriptorSetType::Layer, Binding::Buffer(binding + start))
        }
        Some(DescriptorSetType::Layer) => {
            (DescriptorSetType::Layer, Binding::Buffer(binding - storage + DRAWABLE_RESERVED_UBO_COUNT as u32))
        }
        Some(DescriptorSetType::DrawableUniform) => {
            (DescriptorSetType::DrawableUniform, Binding::Buffer(binding + start))
        }
        Some(DescriptorSetType::DrawableImage) if binding < IMAGE_SAMPLER_START as u32 => {
            (DescriptorSetType::DrawableImage, Binding::Texture(texture_id(binding)?))
        }
        Some(DescriptorSetType::DrawableImage) => {
            (DescriptorSetType::DrawableImage, Binding::Sampler(texture_id(binding - IMAGE_SAMPLER_START as u32)?))
        }
        None => return Err(GpuError::Invalid(format!("{name}: no descriptor set {set}"))),
    };
    Ok(resource)
}

impl VulkanGpu {
    /// A backend on `device`. Fails if the device can't make its pipeline
    /// layout or the placeholders it binds.
    pub fn new(device: &Device) -> Result<VulkanGpu, GpuError> {
        let backend = Rc::new(BackendImpl::new(device, 1).map_err(|error| GpuError::Invalid(error.to_string()))?);
        let placeholder_buffer = BufferAllocation::create(
            &backend,
            256,
            vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::GpuOnly,
            "Gpu_PlaceholderBuffer",
        )
        .ok_or_else(|| GpuError::Invalid("Vulkan couldn't make the placeholder buffer".to_owned()))?;
        let mut placeholder_image = ImageAllocation::create(
            &backend,
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D { width: 1, height: 1, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::SAMPLED),
            MemoryLocation::GpuOnly,
            "Gpu_PlaceholderImage",
        )
        .ok_or_else(|| GpuError::Invalid("Vulkan couldn't make the placeholder image".to_owned()))?;
        if !placeholder_image.create_view(
            vk::Format::R8G8B8A8_UNORM,
            vk::ComponentMapping::default(),
            vk::ImageAspectFlags::COLOR,
            1,
        ) {
            return Err(GpuError::Invalid("Vulkan couldn't make the placeholder image's view".to_owned()));
        }

        let pool = |type_: DescriptorSetType, textures: usize| {
            let shape = type_.buffer_shape();
            DescriptorPoolGrowable::new(
                DESCRIPTOR_POOL_SIZE,
                shape.storage as u32,
                shape.uniform as u32,
                textures as u32,
            )
        };
        // SAFETY: a plain query of the GPU.
        let d24s8 = unsafe {
            backend
                .get_instance()
                .get_physical_device_format_properties(backend.get_physical_device(), vk::Format::D24_UNORM_S8_UINT)
        };
        let depth24_stencil8 =
            if d24s8.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
                vk::Format::D24_UNORM_S8_UINT
            } else {
                vk::Format::D32_SFLOAT_S8_UINT
            };

        // Made with null handles, which `drop` destroys as it does the
        // others if a step below fails.
        let mut gpu = VulkanGpu {
            backend,
            set_layouts: [vk::DescriptorSetLayout::null(); DescriptorSetType::COUNT],
            pipeline_layout: vk::PipelineLayout::null(),
            descriptor_pools: [
                pool(DescriptorSetType::Global, 0),
                pool(DescriptorSetType::Layer, 0),
                pool(DescriptorSetType::DrawableUniform, 0),
                pool(DescriptorSetType::DrawableImage, IMAGE_TEXTURE_COUNT),
            ],
            depth24_stencil8,
            placeholder_buffer,
            placeholder_image,
            placeholder_sampler: vk::Sampler::null(),
            buffers: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: HashMap::new(),
            pipelines: SlotMap::with_key(),
            pipeline_cache: HashMap::new(),
            bind_groups: SlotMap::with_key(),
            descriptor_sets: HashMap::new(),
            render_passes: HashMap::new(),
            framebuffers: HashMap::new(),
            uploads: None,
            next_serial: 1,
            finished_serial: 0,
            in_flight: VecDeque::new(),
            free_command_buffers: Vec::new(),
            free_fences: Vec::new(),
            garbage: Vec::new(),
        };
        for type_ in DescriptorSetType::ALL {
            gpu.set_layouts[type_ as usize] =
                build_descriptor_set_layout(&gpu.backend, type_).map_err(GpuError::Invalid)?;
        }
        gpu.pipeline_layout =
            build_pipeline_layout(&gpu.backend, &gpu.set_layouts, UBO_INDEX_PUSH_CONSTANT_SIZE, "Gpu_PipelineLayout")
                .map_err(GpuError::Invalid)?;
        // SAFETY: the create info is valid for the device.
        gpu.placeholder_sampler = unsafe { gpu.device().create_sampler(&vk::SamplerCreateInfo::default(), None) }
            .map_err(vk_error("the placeholder sampler"))?;
        let uploads = gpu.uploads()?;
        transition_image(
            gpu.device(),
            uploads,
            gpu.placeholder_image.image(),
            whole_range(vk::ImageAspectFlags::COLOR),
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            (vk::AccessFlags::empty(), vk::AccessFlags::SHADER_READ),
            (vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::ALL_COMMANDS),
        );
        Ok(gpu)
    }

    fn device(&self) -> &ash::Device {
        self.backend.get_device()
    }

    fn vk_format(&self, format: TextureFormat) -> vk::Format {
        match format {
            TextureFormat::R8Unorm => vk::Format::R8_UNORM,
            TextureFormat::R16Float => vk::Format::R16_SFLOAT,
            TextureFormat::R32Float => vk::Format::R32_SFLOAT,
            TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
            TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
            TextureFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
            TextureFormat::Rgb10a2Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
            TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
            TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
            TextureFormat::Depth32Float => vk::Format::D32_SFLOAT,
            TextureFormat::Depth24PlusStencil8 => self.depth24_stencil8,
            TextureFormat::Depth32FloatStencil8 => vk::Format::D32_SFLOAT_S8_UINT,
        }
    }

    fn wide_lines(&self) -> bool {
        self.backend.get_device_features().wide_lines == vk::TRUE
    }

    /// A command buffer, recording.
    fn begin_command_buffer(&mut self) -> Result<vk::CommandBuffer, GpuError> {
        let command_buffer = if let Some(command_buffer) = self.free_command_buffers.pop() {
            command_buffer
        } else {
            let allocate_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(self.backend.get_command_pool())
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            // SAFETY: the pool is the backend's, which only this thread uses.
            unsafe { self.device().allocate_command_buffers(&allocate_info) }
                .map_err(vk_error("allocating a command buffer"))?[0]
        };
        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer isn't pending: it is new, or its
        // submission's fence has signaled. Beginning it resets it, as its
        // pool allows.
        if let Err(error) = unsafe { self.device().begin_command_buffer(command_buffer, &begin_info) } {
            self.free_command_buffers.push(command_buffer);
            return Err(vk_error("beginning a command buffer")(error));
        }
        Ok(command_buffer)
    }

    /// The command buffer of the writes since the last submit, which waits
    /// for the submits before it.
    fn uploads(&mut self) -> Result<vk::CommandBuffer, GpuError> {
        if let Some(uploads) = self.uploads {
            return Ok(uploads);
        }
        let uploads = self.begin_command_buffer()?;
        memory_barrier(
            self.device(),
            uploads,
            (vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER),
            (vk::AccessFlags::MEMORY_WRITE, vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE),
        );
        self.uploads = Some(uploads);
        Ok(uploads)
    }

    /// Destroys `garbage` once the next submit has finished.
    fn discard(&mut self, garbage: Garbage) {
        self.garbage.push((self.next_serial, garbage));
    }

    /// Takes back what the finished submits used, and destroys what waited
    /// for them.
    fn collect_garbage(&mut self) {
        // SAFETY: the fences are this backend's.
        let backend = Rc::clone(&self.backend);
        let signaled = |fence| unsafe { backend.get_device().get_fence_status(fence) }.unwrap_or(false);
        while let Some(submission) = self.in_flight.pop_front_if(|submission| signaled(submission.fence)) {
            self.finished_serial = submission.serial;
            self.free_command_buffers.extend(submission.command_buffers);
            self.free_fences.push(submission.fence);
        }
        let finished = self.finished_serial;
        let (done, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.garbage).into_iter().partition(|(serial, _)| *serial <= finished);
        self.garbage = waiting;
        for (_, garbage) in done {
            self.destroy_now(garbage);
        }
    }

    fn destroy_now(&mut self, garbage: Garbage) {
        let device = self.backend.get_device();
        // SAFETY: the submits that used it have finished, and nothing
        // recorded since names it.
        unsafe {
            match garbage {
                Garbage::Buffer(buffer) => drop(buffer),
                Garbage::Image(image) => drop(image),
                Garbage::Sampler(sampler) => device.destroy_sampler(sampler, None),
                Garbage::Pipeline(pipeline) => device.destroy_pipeline(pipeline, None),
                Garbage::Framebuffer(framebuffer) => device.destroy_framebuffer(framebuffer, None),
                Garbage::DescriptorSet(type_, pool, set) => {
                    self.descriptor_pools[type_ as usize].give_back(pool, vec![set]);
                }
            }
        }
    }

    /// Drops the descriptor sets that bind what `uses` matches.
    fn forget_descriptor_sets(&mut self, uses: impl Fn(&Resource) -> bool) {
        let mut forgotten = Vec::new();
        self.descriptor_sets.retain(|key, &mut (set, pool)| {
            let used = key.resources.iter().any(|(_, resource)| uses(resource));
            if used {
                forgotten.push(Garbage::DescriptorSet(key.set, pool, set));
            }
            !used
        });
        for garbage in forgotten {
            self.discard(garbage);
        }
    }

    /// The program of `key`, compiled on first use.
    pub(crate) fn shader(&mut self, key: &ShaderKey) -> Result<Rc<Shader>, GpuError> {
        if let Some(shader) = self.shaders.get(key) {
            return Ok(Rc::clone(shader));
        }
        let shader = Rc::new(self.compile(key)?);
        self.shaders.insert(key.clone(), Rc::clone(&shader));
        Ok(shader)
    }

    /// Compiles the program of `key`.
    fn compile(&self, key: &ShaderKey) -> Result<Shader, GpuError> {
        let program = key.program();
        let source = shader_source(program).ok_or_else(|| GpuError::Unsupported(format!("no GLSL for {program:?}")))?;
        let mut defines = String::new();
        for (name, value) in key.define_values() {
            let _ = writeln!(defines, "#define {name} {value}");
        }

        let mut bindings: [Vec<(u32, Binding)>; DescriptorSetType::COUNT] = Default::default();
        let mut modules = Vec::with_capacity(2);
        let destroy = |modules: &[vk::ShaderModule]| {
            // SAFETY: no pipeline is made from them yet.
            unsafe {
                for &module in modules {
                    self.device().destroy_shader_module(module, None);
                }
            }
        };
        for (stage, stage_source) in [(Stage::Vertex, source.glsl.vertex), (Stage::Fragment, source.glsl.fragment)] {
            let composed = compiler::compose(stage, &defines, source.glsl.prelude, stage_source);
            let result = compiler::compile_reflected(stage, &composed)
                .map_err(|error| GpuError::Invalid(format!("{}: {error}", source.name)))
                .and_then(|compiled| {
                    for (set, binding) in compiled.bindings {
                        let (type_, resource) = binding_of(source.name, source.textures, set, binding)?;
                        let set_bindings = &mut bindings[type_ as usize];
                        if !set_bindings.contains(&(binding, resource)) {
                            set_bindings.push((binding, resource));
                        }
                    }
                    let create_info = vk::ShaderModuleCreateInfo::default().code(&compiled.spirv);
                    // SAFETY: the code is the SPIR-V module naga wrote.
                    unsafe { self.device().create_shader_module(&create_info, None) }.map_err(vk_error(source.name))
                });
            match result {
                Ok(module) => modules.push(module),
                Err(error) => {
                    destroy(&modules);
                    return Err(error);
                }
            }
        }
        for set_bindings in &mut bindings {
            set_bindings.sort_unstable_by_key(|&(binding, _)| binding);
        }
        self.backend.set_debug_name(modules[0], &format!("{}.vert", source.name));
        self.backend.set_debug_name(modules[1], &format!("{}.frag", source.name));
        Ok(Shader {
            name: source.name,
            vertex: modules[0],
            fragment: modules[1],
            bindings,
            attributes: source.attributes,
            instance_attributes: source.instance_attributes,
        })
    }

    /// The render pass of `key`, made on first use.
    fn render_pass(&mut self, key: RenderPassKey) -> Result<vk::RenderPass, GpuError> {
        if let Some(&render_pass) = self.render_passes.get(&key) {
            return Ok(render_pass);
        }
        let load_op = |load| if load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::CLEAR };
        let mut attachments = Vec::with_capacity(2);
        let mut color_ref = Vec::with_capacity(1);
        let mut depth_ref = None;
        if let Some(color) = key.color {
            color_ref.push(
                vk::AttachmentReference::default()
                    .attachment(attachments.len() as u32)
                    .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            );
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(color.format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(load_op(color.load))
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(if color.load { color.layout } else { vk::ImageLayout::UNDEFINED })
                    .final_layout(color.layout),
            );
        }
        if let Some(depth) = key.depth_stencil {
            depth_ref = Some(
                vk::AttachmentReference::default()
                    .attachment(attachments.len() as u32)
                    .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            );
            let loads = depth.load || depth.stencil_load;
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(depth.format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(load_op(depth.load))
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(load_op(depth.stencil_load))
                    .stencil_store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(if loads { depth.layout } else { vk::ImageLayout::UNDEFINED })
                    .final_layout(depth.layout),
            );
        }
        let mut subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref);
        if let Some(depth_ref) = &depth_ref {
            subpass = subpass.depth_stencil_attachment(depth_ref);
        }
        // Every pass waits for what the commands before it wrote, and the
        // commands after it for what it wrote: a pass may sample or load
        // what an earlier one drew, or draw over what an earlier one
        // sampled.
        let attachment_stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
            | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        let attachment_writes =
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE;
        let attachment_access =
            attachment_writes | vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ;
        let shader_stages = vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER;
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(attachment_stages | shader_stages | vk::PipelineStageFlags::TRANSFER)
                .dst_stage_mask(attachment_stages | shader_stages)
                .src_access_mask(attachment_writes | vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(attachment_access | vk::AccessFlags::SHADER_READ),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(attachment_stages)
                .dst_stage_mask(attachment_stages | shader_stages | vk::PipelineStageFlags::TRANSFER)
                .src_access_mask(attachment_writes)
                .dst_access_mask(attachment_access | vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ),
        ];
        let subpasses = [subpass];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: the create info points at live state.
        let render_pass = unsafe { self.device().create_render_pass(&create_info, None) }
            .map_err(vk_error("creating a render pass"))?;
        self.render_passes.insert(key, render_pass);
        Ok(render_pass)
    }

    /// A render pass that pipelines drawing into `targets` are made for:
    /// any pass with attachments of these formats can use them.
    fn compatible_render_pass(&mut self, targets: TargetFormats) -> Result<vk::RenderPass, GpuError> {
        let color = AttachmentKey {
            format: self.vk_format(targets.color),
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            load: false,
            stencil_load: false,
        };
        let depth_stencil = targets.depth_stencil.map(|format| AttachmentKey {
            format: self.vk_format(format),
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            load: false,
            stencil_load: false,
        });
        self.render_pass(RenderPassKey { color: Some(color), depth_stencil })
    }

    fn create_vk_pipeline(&mut self, key: &PipelineKey, shader: &Shader) -> Result<vk::Pipeline, GpuError> {
        let state = &key.state;
        let mut input_bindings = Vec::with_capacity(state.vertex_buffers.len());
        let mut input_attributes = Vec::new();
        for (slot, layout) in state.vertex_buffers.iter().enumerate() {
            input_bindings.push(
                vk::VertexInputBindingDescription::default().binding(slot as u32).stride(layout.stride).input_rate(
                    match layout.step {
                        VertexStep::Vertex => vk::VertexInputRate::VERTEX,
                        VertexStep::Instance => vk::VertexInputRate::INSTANCE,
                    },
                ),
            );
            for attribute in &layout.attributes {
                let format = PipelineInfo::vulkan_format(attribute.format);
                if format == vk::Format::UNDEFINED {
                    return Err(GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)));
                }
                input_attributes.push(
                    vk::VertexInputAttributeDescription::default()
                        .location(shader.location(layout.step, attribute.id)?)
                        .binding(slot as u32)
                        .format(format)
                        .offset(attribute.offset),
                );
            }
        }
        let vertex_input_state = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&input_bindings)
            .vertex_attribute_descriptions(&input_attributes);
        let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(PipelineInfo::vulkan_primitive_topology(state.primitive));
        let viewport_state = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);

        let raster_state = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(if state.cull.enabled {
                PipelineInfo::vulkan_cull_mode(state.cull.side)
            } else {
                vk::CullModeFlags::NONE
            })
            .front_face(PipelineInfo::vulkan_front_face(state.cull.winding))
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0);
        let multisample_state =
            vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let has_depth_stencil = key.targets.depth_stencil.is_some();
        let stencil = &state.stencil;
        let stencil_op = vk::StencilOpState::default()
            .compare_op(PipelineInfo::vulkan_stencil_compare_op(stencil.test.func()))
            .compare_mask(stencil.test.masked().unwrap_or(0xFF))
            .write_mask(stencil.write_mask)
            .pass_op(PipelineInfo::vulkan_stencil_op(stencil.pass))
            .fail_op(PipelineInfo::vulkan_stencil_op(stencil.fail))
            .depth_fail_op(PipelineInfo::vulkan_stencil_op(stencil.depth_fail));
        let depth = &state.depth;
        // Vulkan writes depth only where it tests it.
        let depth_test = has_depth_stencil && (depth.write || depth.compare != DepthFunctionType::Always);
        let depth_stencil_state = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(depth_test)
            .depth_write_enable(has_depth_stencil && depth.write)
            .depth_compare_op(PipelineInfo::vulkan_depth_compare_op(depth.compare))
            .stencil_test_enable(has_depth_stencil && !stencil.is_disabled())
            .front(stencil_op)
            .back(stencil_op)
            .min_depth_bounds(0.0)
            .max_depth_bounds(1.0);

        let blend = state.blend;
        let mask = state.color_mask;
        let mut color_mask = vk::ColorComponentFlags::empty();
        for (enabled, bit) in [
            (mask.r, vk::ColorComponentFlags::R),
            (mask.g, vk::ColorComponentFlags::G),
            (mask.b, vk::ColorComponentFlags::B),
            (mask.a, vk::ColorComponentFlags::A),
        ] {
            if enabled {
                color_mask |= bit;
            }
        }
        let blend_op = PipelineInfo::vulkan_blend_op(blend.equation());
        let src_factor = PipelineInfo::vulkan_blend_factor(blend.src_factor());
        let dst_factor = PipelineInfo::vulkan_blend_factor(blend.dst_factor());
        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(!matches!(blend, BlendFunction::Replace))
            .color_blend_op(blend_op)
            .src_color_blend_factor(src_factor)
            .dst_color_blend_factor(dst_factor)
            .alpha_blend_op(blend_op)
            .src_alpha_blend_factor(src_factor)
            .dst_alpha_blend_factor(dst_factor)
            .color_write_mask(color_mask)];
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);

        // The state that commands set is dynamic in every pipeline, so
        // binding one keeps what was set.
        let mut dynamic_states = vec![
            vk::DynamicState::VIEWPORT,
            vk::DynamicState::SCISSOR,
            vk::DynamicState::STENCIL_REFERENCE,
            vk::DynamicState::BLEND_CONSTANTS,
        ];
        if self.wide_lines() {
            dynamic_states.push(vk::DynamicState::LINE_WIDTH);
        }
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(shader.vertex)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(shader.fragment)
                .name(c"main"),
        ];
        let render_pass = self.compatible_render_pass(key.targets)?;
        let create_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_state)
            .input_assembly_state(&input_assembly_state)
            .viewport_state(&viewport_state)
            .rasterization_state(&raster_state)
            .multisample_state(&multisample_state)
            .depth_stencil_state(&depth_stencil_state)
            .color_blend_state(&color_blend_state)
            .dynamic_state(&dynamic_state)
            .layout(self.pipeline_layout)
            .render_pass(render_pass);
        // SAFETY: the create info points at live state, and the modules,
        // the layout and the render pass are this backend's.
        let pipeline =
            unsafe { self.device().create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None) }
                .map_err(|(_, error)| GpuError::Invalid(format!("{} pipeline: {error}", shader.name)))?[0];
        self.backend.set_debug_name(pipeline, &format!("{}_pipeline", shader.name));
        Ok(pipeline)
    }

    /// The descriptor set of `key`, made on first use.
    fn descriptor_set(&mut self, key: SetKey) -> Result<vk::DescriptorSet, GpuError> {
        if let Some(&(set, _)) = self.descriptor_sets.get(&key) {
            return Ok(set);
        }
        let type_ = key.set;
        let resource = |binding: u32| key.resources.iter().find(|(b, _)| *b == binding).map(|&(_, resource)| resource);

        let mut buffer_infos = Vec::new();
        let mut image_infos = Vec::new();
        // The descriptors in binding order: (binding, type, index into the
        // infos).
        let mut descriptors = Vec::new();
        if type_ == DescriptorSetType::DrawableImage {
            for index in 0..IMAGE_TEXTURE_COUNT {
                for (binding, descriptor_type) in [
                    (image_texture_binding(index), vk::DescriptorType::SAMPLED_IMAGE),
                    (image_sampler_binding(index), vk::DescriptorType::SAMPLER),
                ] {
                    let info = match resource(binding) {
                        Some(Resource::Texture(id)) => {
                            let texture = self.textures.get(id).ok_or(GpuError::UnknownId("texture"))?;
                            if !texture.usage.sampled {
                                return Err(GpuError::Invalid(
                                    "a draw samples a texture that isn't sampled".to_owned(),
                                ));
                            }
                            vk::DescriptorImageInfo::default()
                                .image_view(texture.image.image_view())
                                .image_layout(texture.layout)
                        }
                        Some(Resource::Sampler(id)) => vk::DescriptorImageInfo::default()
                            .sampler(*self.samplers.get(id).ok_or(GpuError::UnknownId("sampler"))?),
                        _ if descriptor_type == vk::DescriptorType::SAMPLER => {
                            vk::DescriptorImageInfo::default().sampler(self.placeholder_sampler)
                        }
                        _ => vk::DescriptorImageInfo::default()
                            .image_view(self.placeholder_image.image_view())
                            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                    };
                    descriptors.push((binding, descriptor_type, image_infos.len()));
                    image_infos.push(info);
                }
            }
        } else {
            let shape = type_.buffer_shape();
            for binding in 0..(shape.storage + shape.uniform) as u32 {
                let storage = (binding as usize) < shape.storage;
                let info = match resource(binding) {
                    Some(Resource::Buffer(id, offset, size)) => {
                        let buffer = self.buffers.get(id).ok_or(GpuError::UnknownId("buffer"))?;
                        let expected = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
                        if buffer.usage != expected {
                            return Err(GpuError::Invalid(format!(
                                "a {:?} buffer bound where the shader reads a {expected:?} buffer",
                                buffer.usage
                            )));
                        }
                        vk::DescriptorBufferInfo::default()
                            .buffer(buffer.allocation.buffer())
                            .offset(offset)
                            .range(size)
                    }
                    _ => vk::DescriptorBufferInfo::default()
                        .buffer(self.placeholder_buffer.buffer())
                        .offset(0)
                        .range(vk::WHOLE_SIZE),
                };
                let descriptor_type =
                    if storage { vk::DescriptorType::STORAGE_BUFFER } else { vk::DescriptorType::UNIFORM_BUFFER };
                descriptors.push((binding, descriptor_type, buffer_infos.len()));
                buffer_infos.push(info);
            }
        }

        let device = self.backend.get_device();
        let (sets, pool) = self.descriptor_pools[type_ as usize]
            .allocate(device, self.set_layouts[type_ as usize], 1)
            .ok_or_else(|| GpuError::Invalid("Vulkan couldn't allocate a descriptor set".to_owned()))?;
        let set = sets[0];
        let writes: Vec<vk::WriteDescriptorSet<'_>> = descriptors
            .iter()
            .map(|&(binding, descriptor_type, index)| {
                let write = vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(binding)
                    .descriptor_type(descriptor_type);
                if type_ == DescriptorSetType::DrawableImage {
                    write.image_info(std::slice::from_ref(&image_infos[index]))
                } else {
                    write.buffer_info(std::slice::from_ref(&buffer_infos[index]))
                }
            })
            .collect();
        // SAFETY: the set is new, and the writes name live resources of
        // the descriptor types of its layout.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
        self.descriptor_sets.insert(key, (set, pool));
        Ok(set)
    }

    /// Binds the descriptor sets that the shader of `pipeline` reads, from
    /// what `bound` binds, where they differ from `current`.
    fn bind_descriptor_sets(
        &mut self,
        command_buffer: vk::CommandBuffer,
        pipeline: &Pipeline,
        bound: &[Option<BindGroupId>],
        current: &mut [vk::DescriptorSet; DescriptorSetType::COUNT],
    ) -> Result<(), GpuError> {
        let shader = &pipeline.shader;
        for type_ in DescriptorSetType::ALL {
            let bindings = &shader.bindings[type_ as usize];
            if bindings.is_empty() {
                continue;
            }
            // The highest slot that binds an id wins.
            let entries = bound.iter().rev().flatten().filter_map(|id| self.bind_groups.get(*id)).flatten();
            let mut resources = Vec::with_capacity(bindings.len());
            for &(binding, read) in bindings {
                let resource = entries.clone().find_map(|entry| match (*entry, read) {
                    (BindGroupEntry::Buffer { id, buffer, offset, size }, Binding::Buffer(wanted)) if id == wanted => {
                        Some(Resource::Buffer(buffer, offset, size))
                    }
                    (BindGroupEntry::Texture { id, texture, .. }, Binding::Texture(wanted)) if id == wanted => {
                        Some(Resource::Texture(texture))
                    }
                    (BindGroupEntry::Texture { id, sampler, .. }, Binding::Sampler(wanted)) if id == wanted => {
                        Some(Resource::Sampler(sampler))
                    }
                    _ => None,
                });
                let resource = resource.ok_or_else(|| {
                    GpuError::Invalid(format!("{} reads {read:?}, which no bind group binds", shader.name))
                })?;
                resources.push((binding, resource));
            }
            let set = self.descriptor_set(SetKey { set: type_, resources })?;
            if current[type_ as usize] != set {
                // SAFETY: the command buffer is recording, and the set has
                // the layout of its index in the pipeline layout.
                unsafe {
                    self.device().cmd_bind_descriptor_sets(
                        command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.pipeline_layout,
                        type_.set_index(),
                        &[set],
                        &[],
                    );
                }
                current[type_ as usize] = set;
            }
        }
        Ok(())
    }

    /// The framebuffer of `render_pass` over the attachments, made on first
    /// use.
    fn framebuffer(
        &mut self,
        render_pass: vk::RenderPass,
        color: Option<TextureId>,
        depth_stencil: Option<TextureId>,
        size: Size,
    ) -> Result<vk::Framebuffer, GpuError> {
        let key = FramebufferKey { render_pass, color, depth_stencil };
        if let Some(&framebuffer) = self.framebuffers.get(&key) {
            return Ok(framebuffer);
        }
        let views: Vec<vk::ImageView> = [color, depth_stencil]
            .into_iter()
            .flatten()
            .map(|id| self.textures.get(id).map(|texture| texture.image.image_view()))
            .collect::<Option<_>>()
            .ok_or(GpuError::UnknownId("texture"))?;
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&views)
            .width(size.width)
            .height(size.height)
            .layers(1);
        // SAFETY: the views are of live images of the pass's formats and of
        // at least `size`.
        let framebuffer = unsafe { self.device().create_framebuffer(&create_info, None) }
            .map_err(vk_error("creating a framebuffer"))?;
        self.framebuffers.insert(key, framebuffer);
        Ok(framebuffer)
    }

    /// Records `pass` into `command_buffer`.
    fn encode(&mut self, command_buffer: vk::CommandBuffer, pass: &Pass) -> Result<(), GpuError> {
        let lookup = |id| self.textures.get(id).ok_or(GpuError::UnknownId("texture"));
        let mut size = None;
        let mut targets = (None, None);
        let mut key = RenderPassKey { color: None, depth_stencil: None };
        let mut clear_values = Vec::with_capacity(2);
        if let Some(attachment) = pass.color {
            let texture = lookup(attachment.texture)?;
            if !texture.usage.render_target || has_depth(texture.format) {
                return Err(GpuError::Invalid(format!(
                    "{}: the color attachment isn't a color render target",
                    pass.label
                )));
            }
            size = Some(texture.size);
            targets.0 = Some(texture.format);
            key.color = Some(AttachmentKey {
                format: texture.vk_format,
                layout: texture.layout,
                load: attachment.load == LoadOp::Load,
                stencil_load: false,
            });
            clear_values.push(match attachment.load {
                LoadOp::Clear(color) => clear_color(color),
                LoadOp::Load => vk::ClearValue::default(),
            });
        }
        if let Some(attachment) = pass.depth_stencil {
            let texture = lookup(attachment.texture)?;
            if !texture.usage.render_target || !has_depth(texture.format) {
                return Err(GpuError::Invalid(format!(
                    "{}: the depth attachment isn't a depth render target",
                    pass.label
                )));
            }
            if size.is_some_and(|size| size != texture.size) {
                return Err(GpuError::Invalid(format!("{}: attachments of different sizes", pass.label)));
            }
            size = Some(texture.size);
            targets.1 = Some(texture.format);
            key.depth_stencil = Some(AttachmentKey {
                format: texture.vk_format,
                layout: texture.layout,
                load: attachment.depth == LoadOp::Load,
                stencil_load: texture.format.has_stencil() && attachment.stencil == LoadOp::Load,
            });
            let depth = match attachment.depth {
                LoadOp::Clear(depth) => depth,
                LoadOp::Load => 0.0,
            };
            let stencil = match attachment.stencil {
                LoadOp::Clear(stencil) => stencil,
                LoadOp::Load => 0,
            };
            clear_values.push(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth, stencil } });
        }
        let Some(size) = size else {
            return if pass.commands.is_empty() {
                Ok(())
            } else {
                Err(GpuError::Invalid(format!("{}: draws without attachments", pass.label)))
            };
        };
        let render_pass = self.render_pass(key)?;
        let framebuffer = self.framebuffer(
            render_pass,
            pass.color.map(|attachment| attachment.texture),
            pass.depth_stencil.map(|attachment| attachment.texture),
            size,
        )?;

        let extent = vk::Extent2D { width: size.width, height: size.height };
        let full = vk::Rect2D { offset: vk::Offset2D::default(), extent };
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(full)
            .clear_values(&clear_values);
        let device = self.backend.get_device();
        self.backend.begin_debug_label(command_buffer, &pass.label, [0.0, 0.0, 0.0, 0.0]);
        // SAFETY: the command buffer is recording outside a render pass,
        // the framebuffer is of the render pass, and the dynamic state set
        // is dynamic in every pipeline.
        unsafe {
            device.cmd_begin_render_pass(command_buffer, &begin_info, vk::SubpassContents::INLINE);
            device.cmd_set_viewport(
                command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: size.width as f32,
                    height: size.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(command_buffer, 0, &[full]);
            device.cmd_set_stencil_reference(command_buffer, vk::StencilFaceFlags::FRONT_AND_BACK, 0);
            device.cmd_set_blend_constants(command_buffer, &[0.0; 4]);
            if self.wide_lines() {
                device.cmd_set_line_width(command_buffer, 1.0);
            }
            device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                &0u32.to_ne_bytes(),
            );
        }
        let result = self.encode_commands(command_buffer, pass, (targets.0, targets.1), extent);
        // SAFETY: the render pass begun above.
        unsafe { self.device().cmd_end_render_pass(command_buffer) };
        self.backend.end_debug_label(command_buffer);
        result
    }

    /// Records the commands of `pass`, inside its render pass over
    /// attachments of `targets` and `extent`.
    fn encode_commands(
        &mut self,
        command_buffer: vk::CommandBuffer,
        pass: &Pass,
        targets: (Option<TextureFormat>, Option<TextureFormat>),
        extent: vk::Extent2D,
    ) -> Result<(), GpuError> {
        let mut pipeline: Option<Rc<Pipeline>> = None;
        let mut bound: Vec<Option<BindGroupId>> = Vec::new();
        let mut current_sets = [vk::DescriptorSet::null(); DescriptorSetType::COUNT];
        // Whether the next draw binds its descriptor sets.
        let mut sets_dirty = true;
        // The vertex buffer slots and the index buffer the pass has bound:
        // Vulkan keeps a binding from an earlier pass.
        let mut vertex_buffers: Vec<bool> = Vec::new();
        let mut index_buffer = false;
        let (min_line_width, max_line_width) = {
            let range = self.backend.get_device_properties().limits.line_width_range;
            (range[0], range[1])
        };
        for command in &pass.commands {
            let device = self.backend.get_device();
            // SAFETY (each command): the command buffer is recording inside
            // the pass's render pass; what a command names is live, and a
            // draw's pipeline, buffers and sets are bound.
            match command {
                RenderCommand::SetPipeline(id) => {
                    let entry = Rc::clone(self.pipelines.get(*id).ok_or(GpuError::UnknownId("pipeline"))?);
                    let targets_match =
                        targets.0 == Some(entry.key.targets.color) && targets.1 == entry.key.targets.depth_stencil;
                    if !targets_match {
                        return Err(GpuError::Invalid(format!(
                            "{}: a pipeline for {:?} in a pass over {targets:?}",
                            pass.label, entry.key.targets
                        )));
                    }
                    unsafe {
                        device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, entry.pipeline);
                    }
                    pipeline = Some(entry);
                    sets_dirty = true;
                }
                RenderCommand::SetBindGroup { slot, group } => {
                    if !self.bind_groups.contains_key(*group) {
                        return Err(GpuError::UnknownId("bind group"));
                    }
                    let slot = *slot as usize;
                    if bound.len() <= slot {
                        bound.resize(slot + 1, None);
                    }
                    bound[slot] = Some(*group);
                    sets_dirty = true;
                }
                RenderCommand::ClearBindGroup { slot } => {
                    if let Some(group) = bound.get_mut(*slot as usize) {
                        *group = None;
                    }
                    sets_dirty = true;
                }
                RenderCommand::SetGlobalIndex(index) => unsafe {
                    device.cmd_push_constants(
                        command_buffer,
                        self.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        &index.to_ne_bytes(),
                    );
                },
                RenderCommand::SetVertexBuffer { slot, buffer, offset } => {
                    let buffer = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    let slot_index = *slot as usize;
                    if vertex_buffers.len() <= slot_index {
                        vertex_buffers.resize(slot_index + 1, false);
                    }
                    vertex_buffers[slot_index] = true;
                    unsafe {
                        device.cmd_bind_vertex_buffers(
                            command_buffer,
                            *slot,
                            &[buffer.allocation.buffer()],
                            &[*offset],
                        );
                    }
                }
                RenderCommand::SetIndexBuffer { buffer, offset } => {
                    let buffer = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    index_buffer = true;
                    unsafe {
                        device.cmd_bind_index_buffer(
                            command_buffer,
                            buffer.allocation.buffer(),
                            *offset,
                            vk::IndexType::UINT16,
                        );
                    }
                }
                RenderCommand::SetViewport(viewport) => unsafe {
                    device.cmd_set_viewport(
                        command_buffer,
                        0,
                        &[vk::Viewport {
                            x: viewport.x,
                            y: viewport.y,
                            width: viewport.width,
                            height: viewport.height,
                            min_depth: viewport.min_depth,
                            max_depth: viewport.max_depth,
                        }],
                    );
                },
                RenderCommand::SetScissor(scissor) => {
                    let x = scissor.x.min(extent.width);
                    let y = scissor.y.min(extent.height);
                    let rect = vk::Rect2D {
                        offset: vk::Offset2D { x: x as i32, y: y as i32 },
                        extent: vk::Extent2D {
                            width: scissor.width.min(extent.width - x),
                            height: scissor.height.min(extent.height - y),
                        },
                    };
                    unsafe { device.cmd_set_scissor(command_buffer, 0, &[rect]) };
                }
                RenderCommand::SetStencilReference(reference) => unsafe {
                    device.cmd_set_stencil_reference(command_buffer, vk::StencilFaceFlags::FRONT_AND_BACK, *reference);
                },
                RenderCommand::SetBlendConstant(color) => unsafe {
                    device.cmd_set_blend_constants(command_buffer, &[color.r, color.g, color.b, color.a]);
                },
                RenderCommand::SetLineWidth(width) => {
                    if self.wide_lines() {
                        unsafe {
                            device.cmd_set_line_width(command_buffer, width.clamp(min_line_width, max_line_width));
                        }
                    }
                }
                RenderCommand::Draw { .. } | RenderCommand::DrawIndexed { .. } => {
                    let entry = pipeline
                        .as_ref()
                        .ok_or_else(|| GpuError::Invalid(format!("{}: a draw before a pipeline", pass.label)))?;
                    let slots = entry.key.state.vertex_buffers.len();
                    let indexed = matches!(command, RenderCommand::DrawIndexed { .. });
                    if (0..slots).any(|slot| !vertex_buffers.get(slot).copied().unwrap_or(false))
                        || (indexed && !index_buffer)
                    {
                        return Err(GpuError::Invalid(format!("{}: a draw without its buffers", pass.label)));
                    }
                    if sets_dirty {
                        self.bind_descriptor_sets(command_buffer, entry, &bound, &mut current_sets)?;
                        sets_dirty = false;
                    }
                    let device = self.backend.get_device();
                    match command {
                        RenderCommand::Draw { vertices, instances } => unsafe {
                            device.cmd_draw(
                                command_buffer,
                                vertices.len() as u32,
                                instances.len() as u32,
                                vertices.start,
                                instances.start,
                            );
                        },
                        RenderCommand::DrawIndexed { indices, base_vertex, instances } => unsafe {
                            device.cmd_draw_indexed(
                                command_buffer,
                                indices.len() as u32,
                                instances.len() as u32,
                                indices.start,
                                *base_vertex,
                                instances.start,
                            );
                        },
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// The texels of `texture`, a color texture that is
    /// [`copy_src`](TextureUsage::copy_src), in rows from the top with no
    /// padding, once the work submitted so far is done.
    #[cfg(test)]
    pub(crate) fn read_texture(&mut self, texture: TextureId) -> Result<Vec<u8>, GpuError> {
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        let bytes_per_texel =
            entry.format.bytes_per_texel().filter(|_| entry.usage.copy_src).ok_or_else(|| {
                GpuError::Invalid(format!("{:?} texture without copy_src: no readback", entry.format))
            })?;
        let (image, layout, size) = (entry.image.image(), entry.layout, entry.size);
        let length = (size.width * size.height * bytes_per_texel) as usize;
        let staging = BufferAllocation::create(
            &self.backend,
            length,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
            "Gpu_Readback",
        )
        .ok_or_else(|| GpuError::Invalid("Vulkan couldn't make a readback buffer".to_owned()))?;
        let uploads = self.uploads()?;
        let device = self.backend.get_device();
        let range = whole_range(vk::ImageAspectFlags::COLOR);
        transition_image(
            device,
            uploads,
            image,
            range,
            (layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 });
        // SAFETY: the image is in the transfer source layout, and the
        // buffer holds its texels.
        unsafe {
            device.cmd_copy_image_to_buffer(
                uploads,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer(),
                &[region],
            );
        }
        transition_image(
            device,
            uploads,
            image,
            range,
            (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, layout),
            (vk::AccessFlags::empty(), vk::AccessFlags::empty()),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS),
        );
        memory_barrier(
            device,
            uploads,
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::HOST_READ),
        );
        self.submit(&[])?;
        let fence = self.in_flight.back().expect("the readback's submission").fence;
        // SAFETY: the fence is this backend's, and its submission was made.
        unsafe { self.device().wait_for_fences(&[fence], true, u64::MAX) }.map_err(vk_error("reading back"))?;
        let mapped = staging.mapped_buffer();
        if mapped.is_null() {
            return Err(GpuError::Invalid("the readback buffer isn't mapped".to_owned()));
        }
        // SAFETY: the copy has finished, and the mapping holds `length`
        // bytes.
        Ok(unsafe { std::slice::from_raw_parts(mapped, length) }.to_vec())
    }
}

/// Records a barrier between all memory accesses of `stages`.
fn memory_barrier(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    (src_stage, dst_stage): (vk::PipelineStageFlags, vk::PipelineStageFlags),
    (src_access, dst_access): (vk::AccessFlags, vk::AccessFlags),
) {
    let barrier = vk::MemoryBarrier::default().src_access_mask(src_access).dst_access_mask(dst_access);
    // SAFETY: the command buffer is recording outside a render pass.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );
    }
}

impl Gpu for VulkanGpu {
    fn limits(&self) -> Limits {
        let limits = &self.backend.get_device_properties().limits;
        Limits {
            buffer_offset_alignment: limits
                .min_uniform_buffer_offset_alignment
                .max(limits.min_storage_buffer_offset_alignment) as u32,
            max_texture_size: limits.max_image_dimension2_d,
        }
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let usage = match desc.usage {
            BufferUsage::Vertex => vk::BufferUsageFlags::VERTEX_BUFFER,
            BufferUsage::Index => vk::BufferUsageFlags::INDEX_BUFFER,
            BufferUsage::Uniform => vk::BufferUsageFlags::UNIFORM_BUFFER,
            BufferUsage::Storage => vk::BufferUsageFlags::STORAGE_BUFFER,
        };
        let size = desc.size.max(4).next_multiple_of(4);
        let allocation = BufferAllocation::create(
            &self.backend,
            size as usize,
            usage | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            desc.label,
        )
        .ok_or_else(|| GpuError::Invalid(format!("{}: Vulkan couldn't make the buffer", desc.label)))?;
        self.backend.set_debug_name(allocation.buffer(), desc.label);
        Ok(self.buffers.insert(Buffer { allocation, size, usage: desc.usage }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let length = data.len() as u64;
        if !offset.is_multiple_of(4) || !length.is_multiple_of(4) || offset + length > entry.size {
            return Err(GpuError::Invalid(format!(
                "a write of {length} bytes at {offset} to a buffer of {}",
                entry.size
            )));
        }
        if data.is_empty() {
            return Ok(());
        }
        let target = entry.allocation.buffer();
        let uploads = self.uploads()?;
        // After the writes recorded before it, which may overlap it.
        memory_barrier(
            self.device(),
            uploads,
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_WRITE),
        );
        if data.len() <= MAX_INLINE_WRITE {
            // SAFETY: the command buffer is recording outside a render
            // pass, and the write fits the buffer in multiples of 4.
            unsafe { self.device().cmd_update_buffer(uploads, target, offset, data) };
            return Ok(());
        }
        let staging = BufferAllocation::create(
            &self.backend,
            data.len(),
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
            "Gpu_Staging",
        )
        .filter(|staging| !staging.mapped_buffer().is_null())
        .ok_or_else(|| GpuError::Invalid("Vulkan couldn't make a staging buffer".to_owned()))?;
        // SAFETY: the mapping holds `data.len()` bytes, which nothing reads
        // until the copy runs.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), staging.mapped_buffer(), data.len()) };
        let region = vk::BufferCopy { src_offset: 0, dst_offset: offset, size: length };
        // SAFETY: the command buffer is recording outside a render pass,
        // and the copy fits both buffers.
        unsafe { self.device().cmd_copy_buffer(uploads, staging.buffer(), target, &[region]) };
        self.discard(Garbage::Buffer(staging));
        Ok(())
    }

    fn destroy_buffer(&mut self, buffer: BufferId) {
        if let Some(entry) = self.buffers.remove(buffer) {
            self.forget_descriptor_sets(|resource| matches!(*resource, Resource::Buffer(id, ..) if id == buffer));
            self.discard(Garbage::Buffer(entry.allocation));
        }
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let format = desc.format;
        let vk_format = self.vk_format(format);
        let aspect = aspect(format);
        let usage = desc.usage;
        if usage.sampled && format.has_stencil() {
            return Err(GpuError::Unsupported(format!("sampling a {format:?} texture")));
        }
        let mut vk_usage = vk::ImageUsageFlags::empty();
        let mut features = vk::FormatFeatureFlags::empty();
        if usage.sampled {
            vk_usage |= vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
            features |= vk::FormatFeatureFlags::SAMPLED_IMAGE;
        }
        if usage.render_target {
            if has_depth(format) {
                vk_usage |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
                features |= vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT;
            } else {
                vk_usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
                features |= vk::FormatFeatureFlags::COLOR_ATTACHMENT;
            }
        }
        if usage.copy_src {
            vk_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }
        if vk_usage.is_empty() {
            return Err(GpuError::Invalid(format!("{}: a texture without a usage", desc.label)));
        }
        // SAFETY: a plain query of the GPU.
        let properties = unsafe {
            self.backend
                .get_instance()
                .get_physical_device_format_properties(self.backend.get_physical_device(), vk_format)
        };
        if !properties.optimal_tiling_features.contains(features) {
            return Err(GpuError::Unsupported(format!("{format:?} textures for {usage:?}")));
        }
        let max = self.limits().max_texture_size;
        let size = desc.size;
        if size.width == 0 || size.height == 0 || size.width > max || size.height > max {
            return Err(GpuError::Invalid(format!("{}: a texture of {}x{}", desc.label, size.width, size.height)));
        }

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk_usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let mut image = ImageAllocation::create(&self.backend, &image_info, MemoryLocation::GpuOnly, desc.label)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Vulkan couldn't make the texture", desc.label)))?;
        if !image.create_view(vk_format, vk::ComponentMapping::default(), aspect, 1) {
            return Err(GpuError::Invalid(format!("{}: Vulkan couldn't make the texture's view", desc.label)));
        }
        self.backend.set_debug_name(image.image(), desc.label);

        let layout = resting_layout(format, usage);
        let uploads = self.uploads()?;
        transition_image(
            self.device(),
            uploads,
            image.image(),
            whole_range(aspect),
            (vk::ImageLayout::UNDEFINED, layout),
            (vk::AccessFlags::empty(), vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE),
            (vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::ALL_COMMANDS),
        );
        Ok(self.textures.insert(Texture { image, format, vk_format, size, usage, aspect, layout }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        let bytes_per_texel = entry.format.bytes_per_texel().filter(|_| entry.usage.sampled).ok_or_else(|| {
            GpuError::Invalid(format!("{:?} textures that aren't sampled take no writes", entry.format))
        })?;
        let size = region.size;
        if region.x + size.width > entry.size.width || region.y + size.height > entry.size.height {
            return Err(GpuError::Invalid("a write outside the texture".to_owned()));
        }
        let expected = size.width as usize * size.height as usize * bytes_per_texel as usize;
        if data.len() != expected {
            return Err(GpuError::Invalid(format!("a write of {} bytes to a region of {expected}", data.len())));
        }
        if data.is_empty() {
            return Ok(());
        }
        let (image, layout, aspect) = (entry.image.image(), entry.layout, entry.aspect);
        let staging = BufferAllocation::create(
            &self.backend,
            data.len(),
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
            "Gpu_Staging",
        )
        .filter(|staging| !staging.mapped_buffer().is_null())
        .ok_or_else(|| GpuError::Invalid("Vulkan couldn't make a staging buffer".to_owned()))?;
        // SAFETY: the mapping holds `data.len()` bytes, which nothing reads
        // until the copy runs.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), staging.mapped_buffer(), data.len()) };

        let uploads = self.uploads()?;
        let device = self.backend.get_device();
        let range = whole_range(aspect);
        transition_image(
            device,
            uploads,
            image,
            range,
            (layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_WRITE),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
        );
        let copy = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: aspect,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: region.x as i32, y: region.y as i32, z: 0 })
            .image_extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 });
        // SAFETY: the image is in the transfer destination layout, and the
        // staging buffer holds the region's texels.
        unsafe {
            device.cmd_copy_buffer_to_image(
                uploads,
                staging.buffer(),
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
        }
        transition_image(
            device,
            uploads,
            image,
            range,
            (vk::ImageLayout::TRANSFER_DST_OPTIMAL, layout),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty()),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
        );
        self.discard(Garbage::Buffer(staging));
        Ok(())
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        let Some(entry) = self.textures.remove(texture) else {
            return;
        };
        self.forget_descriptor_sets(|resource| *resource == Resource::Texture(texture));
        let mut forgotten = Vec::new();
        self.framebuffers.retain(|key, &mut framebuffer| {
            let used = key.color == Some(texture) || key.depth_stencil == Some(texture);
            if used {
                forgotten.push(framebuffer);
            }
            !used
        });
        for framebuffer in forgotten {
            self.discard(Garbage::Framebuffer(framebuffer));
        }
        self.discard(Garbage::Image(entry.image));
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let filter = match state.filter {
            TextureFilterType::Linear => vk::Filter::LINEAR,
            TextureFilterType::Nearest => vk::Filter::NEAREST,
        };
        let address_mode = |wrap| match wrap {
            TextureWrapType::Repeat => vk::SamplerAddressMode::REPEAT,
            TextureWrapType::Clamp => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        };
        let mut create_info = vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(if state.mipmapped { vk::SamplerMipmapMode::LINEAR } else { vk::SamplerMipmapMode::NEAREST })
            .address_mode_u(address_mode(state.wrap_u))
            .address_mode_v(address_mode(state.wrap_v))
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0)
            .max_lod(0.0);
        if state.max_anisotropy > 1 && self.backend.get_device_features().sampler_anisotropy == vk::TRUE {
            let max = self.backend.get_device_properties().limits.max_sampler_anisotropy;
            create_info = create_info.anisotropy_enable(true).max_anisotropy(max.min(f32::from(state.max_anisotropy)));
        }
        // SAFETY: the create info is valid for the device.
        let sampler = unsafe { self.device().create_sampler(&create_info, None) }.map_err(vk_error("sampler"))?;
        Ok(self.samplers.insert(sampler))
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) {
        if let Some(vk_sampler) = self.samplers.remove(sampler) {
            self.forget_descriptor_sets(|resource| *resource == Resource::Sampler(sampler));
            self.discard(Garbage::Sampler(vk_sampler));
        }
    }

    fn create_pipeline(&mut self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        if let Some(pipeline) = self.pipeline_cache.get(key).and_then(Weak::upgrade) {
            return Ok(self.pipelines.insert(pipeline));
        }
        let shader = self.shader(&key.shader)?;
        let pipeline = self.create_vk_pipeline(key, &shader)?;
        let pipeline = Rc::new(Pipeline { key: key.clone(), pipeline, shader });
        self.pipeline_cache.insert(key.clone(), Rc::downgrade(&pipeline));
        Ok(self.pipelines.insert(pipeline))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        let Some(pipeline) = self.pipelines.remove(pipeline).and_then(Rc::into_inner) else {
            return;
        };
        self.pipeline_cache.remove(&pipeline.key);
        self.discard(Garbage::Pipeline(pipeline.pipeline));
    }

    fn create_bind_group(&mut self, desc: &BindGroupDesc<'_>) -> Result<BindGroupId, GpuError> {
        for entry in desc.entries {
            match *entry {
                BindGroupEntry::Buffer { buffer, .. } if !self.buffers.contains_key(buffer) => {
                    return Err(GpuError::UnknownId("buffer"));
                }
                BindGroupEntry::Texture { texture, .. } if !self.textures.contains_key(texture) => {
                    return Err(GpuError::UnknownId("texture"));
                }
                BindGroupEntry::Texture { sampler, .. } if !self.samplers.contains_key(sampler) => {
                    return Err(GpuError::UnknownId("sampler"));
                }
                _ => {}
            }
        }
        Ok(self.bind_groups.insert(desc.entries.to_vec()))
    }

    fn destroy_bind_group(&mut self, group: BindGroupId) {
        self.bind_groups.remove(group);
    }

    fn submit(&mut self, passes: &[Pass]) -> Result<(), GpuError> {
        self.collect_garbage();
        let mut command_buffers = Vec::with_capacity(2);
        if !passes.is_empty() {
            let command_buffer = self.begin_command_buffer()?;
            for pass in passes {
                if let Err(error) = self.encode(command_buffer, pass) {
                    // SAFETY: the command buffer is recording, not pending.
                    let _ = unsafe {
                        self.device().reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())
                    };
                    self.free_command_buffers.push(command_buffer);
                    return Err(error);
                }
            }
            command_buffers.push(command_buffer);
        }
        if let Some(uploads) = self.uploads.take() {
            memory_barrier(
                self.device(),
                uploads,
                (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS),
                (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE),
            );
            command_buffers.insert(0, uploads);
        }
        let fence = match self.free_fences.pop() {
            // SAFETY: the fence's submission has finished.
            Some(fence) => unsafe { self.device().reset_fences(&[fence]) }.map(|()| fence),
            // SAFETY: the create info is valid for the device.
            None => unsafe { self.device().create_fence(&vk::FenceCreateInfo::default(), None) },
        }
        .map_err(vk_error("making a fence"))?;
        let device = self.backend.get_device();
        // SAFETY: the command buffers are recording, and their commands
        // name live resources.
        let ended =
            command_buffers.iter().try_for_each(|&command_buffer| unsafe { device.end_command_buffer(command_buffer) });
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        let submitted = ended.and_then(|()| self.backend.submit(&[submit_info], fence));
        if let Err(error) = submitted {
            self.free_fences.push(fence);
            self.free_command_buffers.extend(command_buffers);
            return Err(vk_error("submitting")(error));
        }
        self.in_flight.push_back(Submission { serial: self.next_serial, fence, command_buffers });
        self.next_serial += 1;
        Ok(())
    }
}

impl Drop for VulkanGpu {
    fn drop(&mut self) {
        self.backend.wait_idle("dropping a VulkanGpu");
        self.collect_garbage();
        for (_, garbage) in std::mem::take(&mut self.garbage) {
            self.destroy_now(garbage);
        }
        let device = self.backend.get_device();
        let command_buffers: Vec<vk::CommandBuffer> = self
            .uploads
            .take()
            .into_iter()
            .chain(self.in_flight.iter().flat_map(|submission| submission.command_buffers.iter().copied()))
            .chain(self.free_command_buffers.drain(..))
            .collect();
        // SAFETY: the device has finished this backend's work, so nothing
        // uses what is destroyed here; the buffers and images go with their
        // fields.
        unsafe {
            if !command_buffers.is_empty() {
                device.free_command_buffers(self.backend.get_command_pool(), &command_buffers);
            }
            for fence in self.in_flight.drain(..).map(|submission| submission.fence).chain(self.free_fences.drain(..)) {
                device.destroy_fence(fence, None);
            }
            for pipeline in self.pipelines.drain().map(|(_, pipeline)| pipeline) {
                if let Some(pipeline) = Rc::into_inner(pipeline) {
                    device.destroy_pipeline(pipeline.pipeline, None);
                }
            }
            for shader in self.shaders.drain().map(|(_, shader)| shader) {
                device.destroy_shader_module(shader.vertex, None);
                device.destroy_shader_module(shader.fragment, None);
            }
            for sampler in self.samplers.drain().map(|(_, sampler)| sampler).chain([self.placeholder_sampler]) {
                device.destroy_sampler(sampler, None);
            }
            for pool in &mut self.descriptor_pools {
                for info in pool.pools.drain(..) {
                    device.destroy_descriptor_pool(info.pool, None);
                }
            }
            for (_, framebuffer) in self.framebuffers.drain() {
                device.destroy_framebuffer(framebuffer, None);
            }
            for (_, render_pass) in self.render_passes.drain() {
                device.destroy_render_pass(render_pass, None);
            }
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            for layout in self.set_layouts {
                device.destroy_descriptor_set_layout(layout, None);
            }
        }
    }
}
