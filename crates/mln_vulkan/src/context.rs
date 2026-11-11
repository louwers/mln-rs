// Ports include/mln/vulkan/context.hpp and src/mln/vulkan/context.cpp
//
// Resources hold a `Weak<Context>` where C++ holds `Context&`, and the
// device through `Rc<BackendImpl>`: a resource that outlives its context is
// destroyed when dropped, instead of through the context's deletion queue.
//
// C++ reaches the default renderable's resource through its backend
// (`backend.getDefaultRenderable()`); the platform backend of the port hands
// it to the context when it makes it (`set_surface`).
//
// Differences from C++:
// - The render passes are the context's, one for each set of attachment
//   formats (`get_render_pass`), where C++ makes one for each renderable
//   resource. A pipeline is made for a render pass, so the renderables of
//   one format share their pipelines, and a resized one keeps its own.
// - The descriptor set layouts are the port's (`shaders::layout`), and they
//   and the pipeline layouts are made with the context.
// - The surface pre-rotation of Android (`GlobalPlatformParamsUBO`, the
//   rotation of the clip masks' matrices, the polling for a new surface
//   transform) is not ported.
// - `SurfaceNotReady` is not ported: acquiring a surface image waits.
// - The globe's clip mask meshes come from the caller, which C++ makes with
//   the renderer's `rawGlobeTileMesh` (see
//   `gfx::Context::render_globe_tile_clipping_masks`).
// - Errors C++ throws (a failed wait for a frame, a surface that stays out
//   of date) are logged. A frame that acquires no image, for a swapchain or
//   a surface that couldn't be made again, is recorded without a
//   framebuffer for the default renderable, and submitted without
//   presenting.
// - Port-only: `request_surface_recreation` has the platform surface made
//   again without it being lost (`SurfaceBackend::restore_surface`).
//
// TODO(port): `createDynamicTexture`, `createRenderTarget`,
// `createRenderbufferResource` and the render jobs of the backend thread
// pool (`runRenderJobs` in `beginFrame`), as in the WebGPU backend.

use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::globe_clip_mask::GlobeClipMask;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::texture2d::{Texture2D as _, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr, UniqueUniformBufferArray};
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_tile_id::CanonicalTileID;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::buffer_resource::BufferResource;
use crate::command_encoder::CommandEncoder;
use crate::descriptor_set::{DescriptorPoolGrowable, ImageBinding, UniformDescriptorSet};
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::LayerGroup;
use crate::offscreen_texture::OffscreenTexture;
use crate::pipeline::PipelineInfo;
use crate::render_pass::RenderPass;
use crate::renderable_resource::SurfaceRenderableResource;
use crate::renderer_backend::BackendImpl;
use crate::shaders::compiler::{self, Stage};
use crate::shaders::layout::{
    CLIP_MASK_PUSH_CONSTANT_SIZE, DescriptorSetType, IMAGE_TEXTURE_COUNT, UBO_INDEX_PUSH_CONSTANT_SIZE,
    image_sampler_binding, image_texture_binding,
};
use crate::shaders::shader_program::{ShaderProgram, as_vulkan_shader};
use crate::shaders::shader_source::shader_source;
use crate::texture2d::Texture2D;
use crate::tile_layer_group::TileLayerGroup;
use crate::uniform_buffer::UniformBuffer;

/// Maximum number of vertex attributes, per vertex descriptor: 32 on most
/// devices (~30% Android use 16), per
/// https://vulkan.gpuinfo.org/displaydevicelimit.php?name=maxVertexInputBindings
/// this can be queried at runtime
/// (VkPhysicalDeviceLimits.maxVertexInputBindings).
const MAXIMUM_VERTEX_BINDING_COUNT: u32 = 16;

const GLOBAL_DESCRIPTOR_POOL_SIZE: u32 = 2;
const LAYER_DESCRIPTOR_POOL_SIZE: u32 = 2 * 256;
const DRAWABLE_UNIFORM_DESCRIPTOR_POOL_SIZE: u32 = 2 * 1024;
const DRAWABLE_IMAGE_DESCRIPTOR_POOL_SIZE: u32 = DRAWABLE_UNIFORM_DESCRIPTOR_POOL_SIZE / 2;

const MAXIMUM_SURFACE_UPDATE_RETRIES: u8 = 3;

/// The rest of a globe clip mask's projection block, which rides as
/// per-instance vertex attributes: the push constants hold its two matrices.
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct GlobeClipMaskInstance {
    tile_mercator_coords: [f32; 4],
    clipping_plane: [f32; 4],
    transition: [f32; 4],
}

/// `DeletionTask`: destroys something once the frame it was last used in is
/// done.
pub type DeletionTask = Box<dyn FnOnce(&Context)>;

/// `FrameResources`: what a frame in flight records into and waits on, and
/// what is destroyed when it is done.
struct FrameResources {
    command_buffer: vk::CommandBuffer,
    flight_frame_fence: vk::Fence,
    deletion_queue: Vec<DeletionTask>,
}

/// The attachments a render pass is made for. Without a depth format, the
/// pass has a color attachment only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderPassKey {
    pub color_format: vk::Format,
    pub depth_format: Option<vk::Format>,
    /// The layout the pass leaves the color attachment in.
    pub color_final_layout: vk::ImageLayout,
    /// Whether the pass draws over what the color attachment holds, which
    /// is in `color_final_layout` when the pass begins, instead of clearing
    /// it.
    pub color_load: bool,
}

#[derive(Default)]
struct Clipping {
    shader: Option<Weak<dyn ShaderProgramBase>>,
    vertex_buffer: Option<BufferResource>,
    index_buffer: Option<BufferResource>,
    index_count: u32,
    pipeline_info: PipelineInfo,
}

/// `GlobeClipMesh`: a tile's pole-capped mesh for its globe clip mask.
struct GlobeClipMesh {
    vertices: BufferResource,
    indices: BufferResource,
    index_count: u32,
}

#[derive(Default)]
struct GlobeClipping {
    shader: Option<Weak<dyn ShaderProgramBase>>,
    /// The meshes, by zoom, whether the tile is on the top or bottom row
    /// (with a pole cap) and whether it has a border.
    meshes: HashMap<(u8, bool, bool, bool), GlobeClipMesh>,
    pipeline_info: PipelineInfo,
}

pub struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,

    global_uniform_buffers: RefCell<UniformBufferArray>,
    global_descriptor_set: RefCell<Option<UniformDescriptorSet>>,
    descriptor_pools: RefCell<[DescriptorPoolGrowable; DescriptorSetType::COUNT]>,

    dummy_buffer: RefCell<Option<BufferResource>>,
    dummy_texture: RefCell<Option<Texture2D>>,
    descriptor_set_layouts: Cell<[vk::DescriptorSetLayout; DescriptorSetType::COUNT]>,
    general_pipeline_layout: Cell<vk::PipelineLayout>,
    push_constant_pipeline_layout: Cell<vk::PipelineLayout>,
    render_passes: RefCell<HashMap<RenderPassKey, vk::RenderPass>>,

    frame_resource_index: Cell<u8>,
    frame_resources: RefCell<Vec<FrameResources>>,
    /// The number of the frame being recorded, or of the last one.
    frame_serial: Cell<u64>,
    /// Whether the frame's command buffer is recording.
    frame_recording: Cell<bool>,
    /// Whether the frame's command buffer is inside a render pass.
    render_pass_active: Cell<bool>,
    /// Whether the frame has an image of the default renderable to draw
    /// into: without a platform surface always, with one when it was
    /// acquired from the swapchain.
    frame_image_acquired: Cell<bool>,

    surface: RefCell<Weak<SurfaceRenderableResource>>,
    surface_update_requested: Cell<bool>,
    /// The platform surface must be recreated before the swapchain is
    /// rebuilt.
    surface_lost: Cell<bool>,
    surface_update_latency: Cell<i32>,

    clipping: RefCell<Clipping>,
    globe_clipping: RefCell<GlobeClipping>,
    destroyed: Cell<bool>,
}

/// `buildUniformDescriptorSetLayout` and `buildImageDescriptorSetLayout`
fn build_descriptor_set_layout(
    backend: &BackendImpl,
    type_: DescriptorSetType,
) -> Result<vk::DescriptorSetLayout, String> {
    let mut bindings = Vec::new();
    if type_ == DescriptorSetType::DrawableImage {
        for index in 0..IMAGE_TEXTURE_COUNT {
            for (binding, descriptor_type) in [
                (image_texture_binding(index), vk::DescriptorType::SAMPLED_IMAGE),
                (image_sampler_binding(index), vk::DescriptorType::SAMPLER),
            ] {
                bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                        .descriptor_type(descriptor_type)
                        .descriptor_count(1),
                );
            }
        }
    } else {
        let shape = type_.buffer_shape();
        for binding in 0..(shape.storage + shape.uniform) as u32 {
            let descriptor_type = if (binding as usize) < shape.storage {
                vk::DescriptorType::STORAGE_BUFFER
            } else {
                vk::DescriptorType::UNIFORM_BUFFER
            };
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .stage_flags(type_.buffer_stages(binding))
                    .descriptor_type(descriptor_type)
                    .descriptor_count(1),
            );
        }
    }
    let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    // SAFETY: the create info is valid for the device.
    let layout = unsafe { backend.get_device().create_descriptor_set_layout(&create_info, None) }
        .map_err(|e| format!("Failed to create a descriptor set layout: {e}"))?;
    backend.set_debug_name(layout, &format!("{type_:?}DescriptorSetLayout"));
    Ok(layout)
}

fn build_pipeline_layout(
    backend: &BackendImpl,
    layouts: &[vk::DescriptorSetLayout],
    push_constant_size: u32,
    name: &str,
) -> Result<vk::PipelineLayout, String> {
    let push_constant = [vk::PushConstantRange::default()
        .size(push_constant_size)
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
    let create_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant).set_layouts(layouts);
    // SAFETY: the layouts are live, and the push constants fit the 128
    // bytes every device has.
    let layout = unsafe { backend.get_device().create_pipeline_layout(&create_info, None) }
        .map_err(|e| format!("Failed to create a pipeline layout: {e}"))?;
    backend.set_debug_name(layout, name);
    Ok(layout)
}

impl Context {
    /// `Context(RendererBackend&)`, or an error if the device can't make
    /// the frames' command buffers, the descriptor set layouts or the
    /// pipeline layouts.
    pub fn new(backend: Rc<BackendImpl>) -> Result<Rc<Context>, String> {
        let storage = |type_: DescriptorSetType| type_.buffer_shape().storage as u32;
        let uniform = |type_: DescriptorSetType| type_.buffer_shape().uniform as u32;
        let (global, layer, drawable) =
            (DescriptorSetType::Global, DescriptorSetType::Layer, DescriptorSetType::DrawableUniform);
        let descriptor_pools = [
            DescriptorPoolGrowable::new(GLOBAL_DESCRIPTOR_POOL_SIZE, 0, uniform(global), 0),
            DescriptorPoolGrowable::new(LAYER_DESCRIPTOR_POOL_SIZE, storage(layer), uniform(layer), 0),
            DescriptorPoolGrowable::new(DRAWABLE_UNIFORM_DESCRIPTOR_POOL_SIZE, 0, uniform(drawable), 0),
            DescriptorPoolGrowable::new(DRAWABLE_IMAGE_DESCRIPTOR_POOL_SIZE, 0, 0, IMAGE_TEXTURE_COUNT as u32),
        ];

        let context = Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(MAXIMUM_VERTEX_BINDING_COUNT),
            backend,
            weak_self: weak.clone(),
            global_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            global_descriptor_set: RefCell::new(None),
            descriptor_pools: RefCell::new(descriptor_pools),
            dummy_buffer: RefCell::new(None),
            dummy_texture: RefCell::new(None),
            descriptor_set_layouts: Cell::new([vk::DescriptorSetLayout::null(); DescriptorSetType::COUNT]),
            general_pipeline_layout: Cell::new(vk::PipelineLayout::null()),
            push_constant_pipeline_layout: Cell::new(vk::PipelineLayout::null()),
            render_passes: RefCell::new(HashMap::new()),
            frame_resource_index: Cell::new(0),
            frame_resources: RefCell::new(Vec::new()),
            frame_serial: Cell::new(0),
            frame_recording: Cell::new(false),
            render_pass_active: Cell::new(false),
            frame_image_acquired: Cell::new(false),
            surface: RefCell::new(Weak::new()),
            surface_update_requested: Cell::new(false),
            surface_lost: Cell::new(false),
            surface_update_latency: Cell::new(0),
            clipping: RefCell::new(Clipping::default()),
            globe_clipping: RefCell::new(GlobeClipping::default()),
            destroyed: Cell::new(false),
        });
        // Dropping the context destroys what a failed step leaves.
        context.init_frame_resources()?;
        // Force placeholder texture upload before any descriptor sets.
        context.get_dummy_texture();
        Ok(context)
    }

    /// `initFrameResources()`: the command buffer and the fence of each
    /// frame in flight, the descriptor set layouts and the pipeline layouts.
    fn init_frame_resources(&self) -> Result<(), String> {
        let backend = &self.backend;
        let device = backend.get_device();
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(backend.get_command_pool())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(backend.get_max_frames());
        // SAFETY: the pool is the device's, and only this thread uses it.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|e| format!("Failed to allocate the frames' command buffers: {e}"))?;
        for (index, command_buffer) in command_buffers.into_iter().enumerate() {
            let create_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            // SAFETY: the create info is valid for the device.
            let flight_frame_fence = unsafe { device.create_fence(&create_info, None) }.unwrap_or_default();
            self.frame_resources.borrow_mut().push(FrameResources {
                command_buffer,
                flight_frame_fence,
                deletion_queue: Vec::new(),
            });
            if flight_frame_fence == vk::Fence::null() {
                return Err("Failed to create a frame's fence".to_owned());
            }
            backend.set_debug_name(command_buffer, &format!("FrameCommandBuffer_{index}"));
            backend.set_debug_name(flight_frame_fence, &format!("FrameFence_{index}"));
        }

        let mut layouts = [vk::DescriptorSetLayout::null(); DescriptorSetType::COUNT];
        for type_ in DescriptorSetType::ALL {
            let layout = build_descriptor_set_layout(backend, type_);
            if let Ok(layout) = layout {
                layouts[type_ as usize] = layout;
            }
            self.descriptor_set_layouts.set(layouts);
            layout?;
        }
        self.general_pipeline_layout.set(build_pipeline_layout(
            backend,
            &layouts,
            UBO_INDEX_PUSH_CONSTANT_SIZE,
            "PipelineLayout_general",
        )?);
        self.push_constant_pipeline_layout.set(build_pipeline_layout(
            backend,
            &layouts,
            CLIP_MASK_PUSH_CONSTANT_SIZE,
            "PipelineLayout_pushConstants",
        )?);
        Ok(())
    }

    /// The owning `Rc`.
    pub fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("vulkan::Context used after drop")
    }

    pub fn weak(&self) -> Weak<Context> {
        self.weak_self.clone()
    }

    /// `getBackend()`
    pub fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// Makes `surface` the resource of the default renderable, which frames
    /// are acquired from and presented to.
    pub fn set_surface(&self, surface: &Rc<SurfaceRenderableResource>) {
        *self.surface.borrow_mut() = Rc::downgrade(surface);
    }

    /// `destroyResources()`: waits for the device, runs the deletion queues
    /// and destroys what the context made. Called before the backend lets go
    /// of the context, and when it is dropped.
    pub(crate) fn destroy_resources(&self) {
        if self.destroyed.replace(true) {
            return;
        }
        self.backend.wait_idle("context shutdown");
        self.frame_recording.set(false);

        *self.global_descriptor_set.borrow_mut() = None;
        *self.global_uniform_buffers.borrow_mut() = UniformBufferArray::new();
        *self.dummy_buffer.borrow_mut() = None;
        *self.dummy_texture.borrow_mut() = None;
        *self.clipping.borrow_mut() = Clipping::default();
        *self.globe_clipping.borrow_mut() = GlobeClipping::default();

        // A deletion can enqueue another.
        loop {
            let tasks: Vec<DeletionTask> = self
                .frame_resources
                .borrow_mut()
                .iter_mut()
                .flat_map(|frame| std::mem::take(&mut frame.deletion_queue))
                .collect();
            if tasks.is_empty() {
                break;
            }
            for task in tasks {
                task(self);
            }
        }

        let device = self.backend.get_device();
        let frames = std::mem::take(&mut *self.frame_resources.borrow_mut());
        // SAFETY: the device is idle, and nothing made from these objects
        // is left: the deletion queues have run, and what is enqueued from
        // here on is destroyed at once.
        unsafe {
            for frame in frames {
                device.free_command_buffers(self.backend.get_command_pool(), &[frame.command_buffer]);
                device.destroy_fence(frame.flight_frame_fence, None);
            }
            for pool in self.descriptor_pools.borrow_mut().iter_mut() {
                for info in pool.pools.drain(..) {
                    device.destroy_descriptor_pool(info.pool, None);
                }
            }
            for (_, render_pass) in self.render_passes.borrow_mut().drain() {
                device.destroy_render_pass(render_pass, None);
            }
            device.destroy_pipeline_layout(self.general_pipeline_layout.get(), None);
            device.destroy_pipeline_layout(self.push_constant_pipeline_layout.get(), None);
            for layout in self.descriptor_set_layouts.get() {
                device.destroy_descriptor_set_layout(layout, None);
            }
        }
    }

    /// `enqueueDeletion(function)`: runs `function` when the frame being
    /// recorded, or the last one, is done.
    pub fn enqueue_deletion(&self, function: DeletionTask) {
        let index = usize::from(self.frame_resource_index.get());
        let mut frames = self.frame_resources.borrow_mut();
        match frames.get_mut(index) {
            Some(frame) => frame.deletion_queue.push(function),
            None => {
                drop(frames);
                function(self);
            }
        }
    }

    /// `FrameResources::runDeletionQueue(context)`
    fn run_deletion_queue(&self, index: usize) {
        let tasks = self.frame_resources.borrow_mut().get_mut(index).map(|f| std::mem::take(&mut f.deletion_queue));
        for task in tasks.into_iter().flatten() {
            task(self);
        }
    }

    fn run_deletion_queues(&self) {
        // Not borrowed while the tasks run: they reach the frames.
        let frame_count = self.frame_resources.borrow().len();
        for index in 0..frame_count {
            self.run_deletion_queue(index);
        }
    }

    /// `submitOneTimeCommand(function)`: records `function`'s commands into
    /// a command buffer of their own, and waits for them.
    pub fn submit_one_time_command(&self, function: impl FnOnce(vk::CommandBuffer)) {
        let device = self.backend.get_device();
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.backend.get_command_pool())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is the device's and only this thread uses it;
        // the command buffer and the fence are used here alone, and the
        // wait ends before they are freed.
        unsafe {
            let Ok(command_buffers) = device.allocate_command_buffers(&allocate_info) else {
                Log::error(Event::Render, "OneTimeCommand - Command buffer allocation failed");
                return;
            };
            let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            if device.begin_command_buffer(command_buffers[0], &begin_info).is_ok() {
                function(command_buffers[0]);
                let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
                let submitted = device.end_command_buffer(command_buffers[0]).is_ok()
                    && device.create_fence(&vk::FenceCreateInfo::default(), None).is_ok_and(|fence| {
                        let done = device
                            .queue_submit(self.backend.get_graphics_queue(), &[submit_info], fence)
                            .and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
                        device.destroy_fence(fence, None);
                        done.is_ok()
                    });
                if !submitted {
                    Log::error(Event::Render, "OneTimeCommand - Wait fence failed");
                }
            }
            device.free_command_buffers(self.backend.get_command_pool(), &command_buffers);
        }
    }

    /// Records an upload's commands: into the frame's command buffer
    /// between its passes, where they run before the passes that follow, or
    /// else into a command buffer of their own, which is waited for.
    /// Returns whether the commands have run.
    pub fn record_upload(&self, function: impl FnOnce(vk::CommandBuffer)) -> bool {
        if self.frame_recording.get() && !self.render_pass_active.get() {
            function(self.get_command_buffer());
            false
        } else {
            self.submit_one_time_command(function);
            true
        }
    }

    /// Tells the context that the frame's command buffer entered or left a
    /// render pass.
    pub(crate) fn set_render_pass_active(&self, active: bool) {
        self.render_pass_active.set(active);
    }

    /// Whether a frame is being recorded that has an image of the default
    /// renderable to draw into.
    pub(crate) fn frame_has_image(&self) -> bool {
        self.frame_recording.get() && self.frame_image_acquired.get()
    }

    /// `requestSurfaceUpdate(useDelay)`: has the swapchain rebuilt, a few
    /// frames from now or before the next one.
    pub fn request_surface_update(&self, use_delay: bool) {
        if self.surface_update_requested.get() {
            if !use_delay {
                self.surface_update_latency.set(0);
            }
            return;
        }
        self.surface_update_requested.set(true);
        self.surface_update_latency.set(if use_delay { self.backend.get_max_frames() as i32 * 3 } else { 0 });
    }

    /// Has the platform surface made again, as a lost one is, and its
    /// swapchain with it, before the next frame.
    pub fn request_surface_recreation(&self) {
        self.surface_lost.set(true);
        self.request_surface_update(false);
    }

    /// `updateSurface()`
    fn update_surface(&self) {
        let Some(surface) = self.surface.borrow().upgrade() else {
            return;
        };
        // The request stays pending if this fails, so the next frame
        // retries it.
        if !surface.recreate_swapchain(self.surface_lost.get()) {
            return;
        }
        self.surface_lost.set(false);

        // We wait for an idle device to recreate the swapchain so it's a
        // good opportunity to delete all queued items.
        self.run_deletion_queues();

        // Sync resources with swapchain.
        self.frame_resource_index.set(0);
        self.surface_update_requested.set(false);
    }

    /// `waitFrame()`: waits for the frame of the current frame resources.
    pub fn wait_frame(&self) -> bool {
        let fence = self
            .frame_resources
            .borrow()
            .get(usize::from(self.frame_resource_index.get()))
            .map(|frame| frame.flight_frame_fence);
        // SAFETY: the fence is the device's.
        fence
            .is_some_and(|fence| unsafe { self.backend.get_device().wait_for_fences(&[fence], true, u64::MAX) }.is_ok())
    }

    /// `submitFrame()`: ends the frame's command buffer, submits it and,
    /// with a platform surface, presents the image.
    pub fn submit_frame(&self) {
        if !self.frame_recording.replace(false) {
            return;
        }
        let device = self.backend.get_device();
        let Some((command_buffer, fence)) = self
            .frame_resources
            .borrow()
            .get(usize::from(self.frame_resource_index.get()))
            .map(|frame| (frame.command_buffer, frame.flight_frame_fence))
        else {
            return;
        };
        // A frame that acquired no image has nothing to present, and no
        // semaphore to wait for.
        let surface = self
            .surface
            .borrow()
            .upgrade()
            .filter(|surface| surface.has_platform_surface() && self.frame_image_acquired.get());

        // Submit frame commands.
        let command_buffers = [command_buffer];
        let wait_stage_mask = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let frame_index = self.frame_resource_index.get();
        let semaphores =
            surface.as_ref().map(|s| ([s.get_acquire_semaphore(frame_index)], [s.get_present_semaphore()]));
        let mut submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        if let Some((acquire, present)) = &semaphores {
            submit_info =
                submit_info.signal_semaphores(present).wait_semaphores(acquire).wait_dst_stage_mask(&wait_stage_mask);
        }
        // SAFETY: the command buffer is recording and complete, the fence
        // was waited for when the frame began, and the semaphores are the
        // acquired image's.
        unsafe {
            if device.end_command_buffer(command_buffer).is_err() {
                Log::error(Event::Render, "Ending the frame's command buffer failed");
            }
            if device.reset_fences(&[fence]).is_err() {
                Log::error(Event::Render, "Reset fence failed");
            }
            if device.queue_submit(self.backend.get_graphics_queue(), &[submit_info], fence).is_err() {
                Log::error(Event::Render, "Submitting the frame failed");
            }
        }

        // Present rendered frame.
        if let (Some(surface), Some((_, present))) = (&surface, &semaphores)
            && let (Some(loader), Some(queue)) = (self.backend.get_swapchain_loader(), self.backend.get_present_queue())
        {
            let swapchains = [surface.get_swapchain()];
            let image_indices = [surface.get_acquired_image_index()];
            let present_info = vk::PresentInfoKHR::default()
                .swapchains(&swapchains)
                .wait_semaphores(present)
                .image_indices(&image_indices);
            // SAFETY: the image was acquired from the swapchain, and the
            // submitted frame signals the semaphore.
            match unsafe { loader.queue_present(queue, &present_info) } {
                Ok(true) => self.request_surface_update(true),
                Ok(false) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.request_surface_update(false),
                Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                    // The frame was submitted, recreate the surface before
                    // the next one.
                    self.surface_lost.set(true);
                    self.request_surface_update(false);
                }
                Err(error) => Log::error(Event::Render, format!("queuePresentKHR result: {error}")),
            }
        }
    }

    /// `createBuffer(data, size, usage, persistent)`
    pub fn create_buffer(
        &self,
        data: Option<&[u8]>,
        size: usize,
        usage: vk::BufferUsageFlags,
        persistent: bool,
    ) -> BufferResource {
        BufferResource::new(self, data, size, usage, persistent)
    }

    /// `createProgram(shaderID, name, vertex, fragment, programParameters,
    /// additionalDefines)`: compiles the two stages with the program's
    /// defines and `additional_defines`.
    pub fn create_program(
        &self,
        shader_id: BuiltIn,
        name: String,
        shader_prelude: &str,
        vertex: &str,
        fragment: &str,
        program_parameters: &ProgramParameters,
        additional_defines: &HashMap<String, String>,
    ) -> Option<ShaderProgram> {
        let mut define_str = format!("{}\n\n", program_parameters.get_defines_string());
        let mut additional: Vec<_> = additional_defines.iter().collect();
        additional.sort();
        for (key, value) in additional {
            define_str.push_str(&format!("#define {key} {value}\n"));
        }

        let observer = self.get_observer();
        if let Some(observer) = &observer {
            observer.on_pre_compile_shader(shader_id, BackendType::Vulkan, &define_str);
        }
        let compile = |stage: Stage, source: &str| {
            let spirv = compiler::compile(stage, &compiler::compose(stage, &define_str, shader_prelude, source))
                .map_err(|error| Log::error(Event::Shader, format!("{name} - {error}")))
                .ok()?;
            let create_info = vk::ShaderModuleCreateInfo::default().code(&spirv);
            // SAFETY: the code is the SPIR-V module naga wrote.
            unsafe { self.backend.get_device().create_shader_module(&create_info, None) }
                .map_err(|error| Log::error(Event::Shader, format!("{name} - {error}")))
                .ok()
        };
        let vertex_shader = compile(Stage::Vertex, vertex);
        let fragment_shader = compile(Stage::Fragment, fragment);
        let (Some(vertex_shader), Some(fragment_shader)) = (vertex_shader, fragment_shader) else {
            // SAFETY: nothing uses the stage that did compile.
            unsafe {
                for module in [vertex_shader, fragment_shader].into_iter().flatten() {
                    self.backend.get_device().destroy_shader_module(module, None);
                }
            }
            if let Some(observer) = &observer {
                observer.on_shader_compile_failed(shader_id, BackendType::Vulkan, &define_str);
            }
            debug_assert!(false, "shader compile failed");
            return None;
        };
        self.backend.set_debug_name(vertex_shader, &format!("{name}.vert"));
        self.backend.set_debug_name(fragment_shader, &format!("{name}.frag"));

        let shader = ShaderProgram::new(name, self, vertex_shader, fragment_shader);
        if let Some(observer) = &observer {
            observer.on_post_compile_shader(shader_id, BackendType::Vulkan, &define_str);
        }
        Some(shader)
    }

    /// `createTexture2D()` with the concrete type.
    pub fn create_vulkan_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self)))
    }

    /// `getDummyBuffer()`: bound where a buffer or a vertex attribute has
    /// no data. Shaders don't read it.
    pub fn get_dummy_buffer(&self) -> vk::Buffer {
        if let Some(buffer) = self.dummy_buffer.borrow().as_ref() {
            return buffer.get_vulkan_buffer();
        }
        let usage = vk::BufferUsageFlags::VERTEX_BUFFER
            | vk::BufferUsageFlags::UNIFORM_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER;
        let buffer = BufferResource::new(self, None, 16, usage, false);
        let handle = buffer.get_vulkan_buffer();
        *self.dummy_buffer.borrow_mut() = Some(buffer);
        handle
    }

    /// `getDummyTexture()`: bound where a shader's texture has no image.
    pub fn get_dummy_texture(&self) -> Option<ImageBinding> {
        let mut dummy = self.dummy_texture.borrow_mut();
        let texture = match dummy.as_mut() {
            Some(texture) => texture,
            None => {
                let size = Size::new(2, 2);
                let data = vec![0xFFu8; 4 * size.width as usize * size.height as usize];
                let mut texture = Texture2D::new(self);
                texture.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
                texture.upload_data(Some(&data), size);
                dummy.insert(texture)
            }
        };
        let image_view = texture.get_vulkan_image_view();
        let sampler = texture.get_vulkan_sampler();
        (image_view != vk::ImageView::null() && sampler != vk::Sampler::null()).then_some(ImageBinding {
            image_id: 0,
            sampler_id: 0,
            image_view,
            image_layout: texture.get_vulkan_image_layout(),
            sampler,
        })
    }

    /// `getDescriptorSetLayout(type)`
    pub fn get_descriptor_set_layout(&self, type_: DescriptorSetType) -> vk::DescriptorSetLayout {
        self.descriptor_set_layouts.get()[type_ as usize]
    }

    /// A descriptor set of `type_` for each frame in flight, and the pool
    /// they are from (`DescriptorSet::allocate`).
    pub(crate) fn allocate_descriptor_sets(&self, type_: DescriptorSetType) -> Option<(Vec<vk::DescriptorSet>, usize)> {
        if self.destroyed.get() {
            return None;
        }
        let sets = self.descriptor_pools.borrow_mut()[type_ as usize].allocate(
            self.backend.get_device(),
            self.get_descriptor_set_layout(type_),
            self.backend.get_max_frames() as usize,
        );
        if sets.is_none() {
            Log::error(Event::Render, "Vulkan descriptor set allocation failed");
        }
        sets
    }

    /// Takes back the sets of `allocate_descriptor_sets`.
    pub(crate) fn give_back_descriptor_sets(
        &self,
        type_: DescriptorSetType,
        pool_index: usize,
        sets: Vec<vk::DescriptorSet>,
    ) {
        self.descriptor_pools.borrow_mut()[type_ as usize].give_back(pool_index, sets);
    }

    /// `getGeneralPipelineLayout()`: the four descriptor sets, and the
    /// drawable's index as the push constant.
    pub fn get_general_pipeline_layout(&self) -> vk::PipelineLayout {
        self.general_pipeline_layout.get()
    }

    /// `getPushConstantPipelineLayout()`: the layout of the clip mask
    /// pipelines, whose push constants are matrices.
    pub fn get_push_constant_pipeline_layout(&self) -> vk::PipelineLayout {
        self.push_constant_pipeline_layout.get()
    }

    /// The render pass over attachments of `key`'s formats, null if the
    /// device can't make it.
    ///
    /// A pass clears its attachments, unless it loads the color
    /// (`RenderPassKey::color_load`). A pass with depth and stencil
    /// (`SurfaceRenderableResource::initRenderPass`) keeps the color only; a
    /// pass without (`OffscreenTextureResource::createRenderPass`) leaves
    /// the color attachment ready to be sampled.
    pub fn get_render_pass(&self, key: RenderPassKey) -> vk::RenderPass {
        if let Some(&render_pass) = self.render_passes.borrow().get(&key) {
            return render_pass;
        }
        let color_attachment = vk::AttachmentDescription::default()
            .format(key.color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(if key.color_load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::CLEAR })
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(if key.color_load { key.color_final_layout } else { vk::ImageLayout::UNDEFINED })
            .final_layout(key.color_final_layout);
        let color_attachment_ref =
            [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_attachment_ref =
            vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let mut attachments = vec![color_attachment];
        let mut subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_attachment_ref);
        let dependencies;
        if let Some(depth_format) = key.depth_format {
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(depth_format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::CLEAR)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            );
            subpass = subpass.depth_stencil_attachment(&depth_attachment_ref);
            let depth_stages =
                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
            dependencies = vec![
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dependency_flags(vk::DependencyFlags::BY_REGION),
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(depth_stages)
                    .dst_stage_mask(depth_stages)
                    .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dependency_flags(vk::DependencyFlags::BY_REGION),
            ];
        } else if key.color_load {
            // The pass before wrote the color, which this one blends over.
            dependencies = vec![
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            ];
        } else {
            dependencies = vec![
                vk::SubpassDependency::default()
                    .src_subpass(vk::SUBPASS_EXTERNAL)
                    .dst_subpass(0)
                    .src_stage_mask(
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                    )
                    .dst_stage_mask(
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                    )
                    .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_access_mask(
                        vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                    ),
            ];
        }
        let subpasses = [subpass];
        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: the create info is valid for the device.
        match unsafe { self.backend.get_device().create_render_pass(&create_info, None) } {
            Ok(render_pass) => {
                self.render_passes.borrow_mut().insert(key, render_pass);
                render_pass
            }
            Err(error) => {
                Log::error(Event::Render, format!("Failed to create a render pass: {error}"));
                vk::RenderPass::null()
            }
        }
    }

    /// `getCurrentFrameResourceIndex()`
    pub fn get_current_frame_resource_index(&self) -> u8 {
        self.frame_resource_index.get()
    }

    /// The number of the frame being recorded, or of the last one. No two
    /// frames have the same.
    pub fn frame_serial(&self) -> u64 {
        self.frame_serial.get()
    }

    /// `getCommandBuffer()`: the command buffer of the frame.
    pub fn get_command_buffer(&self) -> vk::CommandBuffer {
        self.frame_resources
            .borrow()
            .get(usize::from(self.frame_resource_index.get()))
            .map_or(vk::CommandBuffer::null(), |frame| frame.command_buffer)
    }

    /// The clipping mask shader of `variant`, kept in `cached`.
    fn get_clip_mask_shader(
        &self,
        shaders: &ShaderRegistry,
        variant: ProjectionVariant,
        cached: &mut Option<Weak<dyn ShaderProgramBase>>,
    ) -> Option<ShaderProgramBasePtr> {
        if let Some(shader) = cached.as_ref().and_then(Weak::upgrade) {
            return Some(shader);
        }
        let group = shaders.get_shader_group("ClippingMaskProgram")?;
        let shader = group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "")?;
        *cached = Some(Rc::downgrade(&shader));
        Some(shader)
    }

    /// The pipeline state both kinds of clip mask share: no color or depth,
    /// and the stencil value replaced by the reference, with the position
    /// attribute read from binding 0.
    fn init_clip_mask_pipeline_info(pipeline_info: &mut PipelineInfo) -> bool {
        let Some(attribute) = shader_source(BuiltIn::ClippingMaskProgram).and_then(|source| source.attributes.first())
        else {
            return false;
        };
        pipeline_info.use_push_constants = true;

        pipeline_info.color_blend = false;
        pipeline_info.color_mask = vk::ColorComponentFlags::empty();

        pipeline_info.depth_test = false;
        pipeline_info.depth_write = false;

        pipeline_info.stencil_test = true;
        pipeline_info.stencil_function = vk::CompareOp::ALWAYS;
        pipeline_info.stencil_pass = vk::StencilOp::REPLACE;
        pipeline_info.dynamic_values.stencil_write_mask = 0xFF;
        pipeline_info.dynamic_values.stencil_ref = 0xFF;
        pipeline_info.dynamic_values.stencil_compare_mask = 0xFF;

        pipeline_info.input_bindings.push(
            vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(VertexAttribute::get_stride_of(attribute.data_type) as u32)
                .input_rate(vk::VertexInputRate::VERTEX),
        );
        pipeline_info.input_attributes.push(
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(attribute.index as u32)
                .format(PipelineInfo::vulkan_format(attribute.data_type)),
        );
        true
    }

    /// `renderTileClippingMasks(renderPass, staticData, tileUBOs)`
    fn render_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        let mut clipping = self.clipping.borrow_mut();
        let clipping = &mut *clipping;
        let Some(shader_ptr) = self.get_clip_mask_shader(shaders, ProjectionVariant::Mercator, &mut clipping.shader)
        else {
            debug_assert!(false, "Failed to create shader for clip masking");
            return false;
        };
        let Some(shader) = as_vulkan_shader(&shader_ptr) else {
            return false;
        };
        let Some(render_pass) = render_pass.as_any().downcast_ref::<RenderPass>().filter(|pass| pass.is_encoding())
        else {
            return false;
        };

        // Create a vertex buffer from the fixed tile coordinates:
        // `RenderStaticData::tileVertices()`, the four corners of the tile
        // extent as short2 (inlined: RenderStaticData is a renderer type).
        let vertex_buffer = clipping.vertex_buffer.get_or_insert_with(|| {
            const EXTENT: i16 = 8192; // util::EXTENT
            let vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
            let bytes: &[u8] = bytemuck::cast_slice(&vertices);
            self.create_buffer(Some(bytes), bytes.len(), vk::BufferUsageFlags::VERTEX_BUFFER, false)
        });
        // Create a buffer from the fixed tile indexes:
        // `RenderStaticData::quadTriangleIndices()`.
        let index_buffer = clipping.index_buffer.get_or_insert_with(|| {
            let indices: [u16; 6] = [0, 1, 2, 1, 2, 3];
            let bytes: &[u8] = bytemuck::cast_slice(&indices);
            self.create_buffer(Some(bytes), bytes.len(), vk::BufferUsageFlags::INDEX_BUFFER, false)
        });
        clipping.index_count = 6;
        if !vertex_buffer.is_valid() || !index_buffer.is_valid() {
            return false;
        }

        // build pipeline
        let pipeline_info = &mut clipping.pipeline_info;
        if pipeline_info.input_attributes.is_empty() {
            if !Self::init_clip_mask_pipeline_info(pipeline_info) {
                return false;
            }
            pipeline_info.update_vertex_input_hash();
        }
        let extent = render_pass.get_extent();
        pipeline_info.set_render_target(render_pass.get_vulkan_render_pass(), extent);
        let Some(pipeline) = shader.get_pipeline(pipeline_info) else {
            return false;
        };

        let device = self.backend.get_device();
        let command_buffer = render_pass.get_command_buffer();
        pipeline_info.set_scissor_rect(&ScissorRect { x: 0, y: 0, width: extent.width, height: extent.height });
        // SAFETY: the command buffer is in the render pass the pipeline was
        // made for, the buffers hold the quad's vertices and indexes, and a
        // matrix fits the push constants of the clip masks' layout.
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline);
            pipeline_info.set_dynamic_values(&self.backend, command_buffer);
            device.cmd_bind_vertex_buffers(command_buffer, 0, &[vertex_buffer.get_vulkan_buffer()], &[0]);
            device.cmd_bind_index_buffer(command_buffer, index_buffer.get_vulkan_buffer(), 0, vk::IndexType::UINT16);
            for tile_info in tile_ubos {
                device.cmd_set_stencil_reference(
                    command_buffer,
                    vk::StencilFaceFlags::FRONT_AND_BACK,
                    tile_info.stencil_ref,
                );
                device.cmd_push_constants(
                    command_buffer,
                    self.push_constant_pipeline_layout.get(),
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::cast_slice(&tile_info.matrix),
                );
                device.cmd_draw_indexed(command_buffer, clipping.index_count, 1, 0, 0, 0);
            }
        }

        let mut stats = self.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        true
    }

    /// `renderGlobeTileClippingMasks(renderPass, staticData, masks)`, with
    /// `mesh` making a tile's mesh in place of C++'s `rawGlobeTileMesh`.
    fn render_globe_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &dyn Fn(&CanonicalTileID, bool) -> (Vec<u8>, Vec<u16>),
    ) -> bool {
        let mut globe_clipping = self.globe_clipping.borrow_mut();
        let globe_clipping = &mut *globe_clipping;
        let Some(shader_ptr) = self.get_clip_mask_shader(shaders, ProjectionVariant::Globe, &mut globe_clipping.shader)
        else {
            debug_assert!(false, "Failed to create shader for globe clip masking");
            return false;
        };
        let Some(shader) = as_vulkan_shader(&shader_ptr) else {
            return false;
        };
        let Some(render_pass) = render_pass.as_any().downcast_ref::<RenderPass>().filter(|pass| pass.is_encoding())
        else {
            return false;
        };

        let pipeline_info = &mut globe_clipping.pipeline_info;
        if pipeline_info.input_attributes.is_empty() {
            if !Self::init_clip_mask_pipeline_info(pipeline_info) {
                return false;
            }
            pipeline_info.dynamic_values.stencil_ref = 0;
            // The far side of a limb tile projects inside the disc; cull it
            // or it overwrites the tile in front. The shader flips Y for
            // Vulkan's downward framebuffer, so the far side is still
            // clockwise on screen.
            pipeline_info.set_cull_mode(&CullFaceMode::back_ccw());
            pipeline_info.input_bindings.push(
                vk::VertexInputBindingDescription::default()
                    .binding(1)
                    .stride(std::mem::size_of::<GlobeClipMaskInstance>() as u32)
                    .input_rate(vk::VertexInputRate::INSTANCE),
            );
            for i in 0..3 {
                pipeline_info.input_attributes.push(
                    vk::VertexInputAttributeDescription::default()
                        .binding(1)
                        .location(1 + i)
                        .format(PipelineInfo::vulkan_format(AttributeDataType::Float4))
                        .offset(i * std::mem::size_of::<[f32; 4]>() as u32),
                );
            }
            pipeline_info.update_vertex_input_hash();
        }
        let extent = render_pass.get_extent();
        pipeline_info.set_render_target(render_pass.get_vulkan_render_pass(), extent);
        let Some(pipeline) = shader.get_pipeline(pipeline_info) else {
            return false;
        };

        let device = self.backend.get_device();
        let command_buffer = render_pass.get_command_buffer();
        pipeline_info.set_scissor_rect(&ScissorRect { x: 0, y: 0, width: extent.width, height: extent.height });
        // SAFETY: the command buffer is in the render pass the pipeline was
        // made for.
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline);
            pipeline_info.set_dynamic_values(&self.backend, command_buffer);
        }

        let instances: Vec<GlobeClipMaskInstance> = masks
            .iter()
            .map(|mask| GlobeClipMaskInstance {
                tile_mercator_coords: mask.projection.tile_mercator_coords,
                clipping_plane: mask.projection.clipping_plane,
                transition: [mask.projection.projection_transition, 0.0, 0.0, 0.0],
            })
            .collect();
        let instance_bytes: &[u8] = bytemuck::cast_slice(&instances);
        // Dropped after the draws are recorded, and destroyed when the
        // frame is done.
        let instance_buffer =
            self.create_buffer(Some(instance_bytes), instance_bytes.len(), vk::BufferUsageFlags::VERTEX_BUFFER, false);
        if !instance_buffer.is_valid() {
            return false;
        }

        // Each mask is drawn with a border first, then exactly, as GL JS
        // does: where a tile meets finer ones, its coarser edge leaves a
        // sliver between the masks that only a neighbor's border covers.
        for borders in [true, false] {
            for (i, mask) in masks.iter().enumerate() {
                let tile = &mask.tile;
                let key = (tile.z, tile.y == 0, tile.y == (1u32 << tile.z) - 1, borders);
                let clip_mesh = match globe_clipping.meshes.entry(key) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let (vertices, indices) = mesh(tile, borders);
                        let index_bytes: &[u8] = bytemuck::cast_slice(&indices);
                        let vertex_buffer = self.create_buffer(
                            Some(&vertices),
                            vertices.len(),
                            vk::BufferUsageFlags::VERTEX_BUFFER,
                            false,
                        );
                        let index_buffer = self.create_buffer(
                            Some(index_bytes),
                            index_bytes.len(),
                            vk::BufferUsageFlags::INDEX_BUFFER,
                            false,
                        );
                        if !vertex_buffer.is_valid() || !index_buffer.is_valid() {
                            return false;
                        }
                        entry.insert(GlobeClipMesh {
                            vertices: vertex_buffer,
                            indices: index_buffer,
                            index_count: indices.len() as u32,
                        })
                    }
                };

                let constants: [[f32; 16]; 2] = [mask.projection.matrix, mask.projection.fallback_matrix];
                // SAFETY: the mesh's buffers hold its vertices and indexes,
                // the instance buffer the mask's instance, and the two
                // matrices fill the push constants of the clip masks'
                // layout.
                unsafe {
                    device.cmd_set_stencil_reference(
                        command_buffer,
                        vk::StencilFaceFlags::FRONT_AND_BACK,
                        mask.stencil_ref,
                    );
                    device.cmd_bind_vertex_buffers(
                        command_buffer,
                        0,
                        &[clip_mesh.vertices.get_vulkan_buffer(), instance_buffer.get_vulkan_buffer()],
                        &[0, (i * std::mem::size_of::<GlobeClipMaskInstance>()) as vk::DeviceSize],
                    );
                    device.cmd_bind_index_buffer(
                        command_buffer,
                        clip_mesh.indices.get_vulkan_buffer(),
                        0,
                        vk::IndexType::UINT16,
                    );
                    device.cmd_push_constants(
                        command_buffer,
                        self.push_constant_pipeline_layout.get(),
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        bytemuck::cast_slice(&constants),
                    );
                    device.cmd_draw_indexed(command_buffer, clip_mesh.index_count, 1, 0, 0, 0);
                }
            }
        }

        let mut stats = self.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        true
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.destroy_resources();
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn backend_type(&self) -> BackendType {
        BackendType::Vulkan
    }

    fn begin_frame(&self) {
        if self.destroyed.get() {
            return;
        }
        let frame_count = self.frame_resources.borrow().len().max(1);
        self.frame_resource_index.set(((usize::from(self.frame_resource_index.get()) + 1) % frame_count) as u8);

        let device = self.backend.get_device();
        let surface = self.surface.borrow().upgrade();
        // Asked again after each update, which makes a lost surface anew.
        let platform_surface = || surface.as_ref().is_some_and(|surface| surface.has_platform_surface());

        // A lost surface may have been destroyed by a failed recreation.
        if (platform_surface() || self.surface_lost.get()) && self.surface_update_requested.get() {
            self.surface_update_latency.set(self.surface_update_latency.get() - 1);
            if self.surface_update_latency.get() <= 0 {
                self.update_surface();
            }
        }

        let mut acquired = false;
        for _ in 0..MAXIMUM_SURFACE_UPDATE_RETRIES {
            if !self.wait_frame() {
                Log::error(Event::Render, "waitFrame failed");
            }
            self.run_deletion_queue(usize::from(self.frame_resource_index.get()));

            let frame_index = self.frame_resource_index.get();
            match &surface {
                Some(surface) if platform_surface() => {
                    let Some(loader) = self.backend.get_swapchain_loader() else {
                        break;
                    };
                    // A swapchain that couldn't be rebuilt has no images.
                    if surface.get_image_count() == 0 {
                        break;
                    }
                    // SAFETY: the swapchain and the semaphore are the
                    // surface's, and the semaphore isn't waited on.
                    let result = unsafe {
                        loader.acquire_next_image(
                            surface.get_swapchain(),
                            u64::MAX,
                            surface.get_acquire_semaphore(frame_index),
                            vk::Fence::null(),
                        )
                    };
                    match result {
                        Ok((index, suboptimal)) => {
                            surface.set_acquired_image_index(index);
                            if suboptimal {
                                self.request_surface_update(true);
                            }
                        }
                        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                            self.request_surface_update(false);
                            self.update_surface();
                            continue;
                        }
                        Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                            self.surface_lost.set(true);
                            self.request_surface_update(false);
                            self.update_surface();
                            continue;
                        }
                        Err(error) => {
                            Log::error(Event::Render, format!("acquireNextImageKHR result: {error}"));
                            break;
                        }
                    }
                }
                // The lost surface couldn't be made again.
                Some(_) if self.surface_lost.get() => break,
                Some(surface) => surface.set_acquired_image_index(u32::from(frame_index)),
                None => {}
            }
            // acquire successful
            acquired = true;
            break;
        }
        if !acquired {
            Log::error(Event::Render, "acquireNextImageKHR failed");
        }
        self.frame_image_acquired.set(acquired);

        let command_buffer = self.get_command_buffer();
        // SAFETY: the frame that used the command buffer is done.
        unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            if device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::RELEASE_RESOURCES).is_err()
                || device.begin_command_buffer(command_buffer, &begin_info).is_err()
            {
                Log::error(Event::Render, "Beginning the frame's command buffer failed");
                return;
            }
        }
        self.frame_serial.set(self.frame_serial.get() + 1);
        self.frame_recording.set(true);
        self.render_pass_active.set(false);
    }

    fn end_frame(&self) {}

    /// Called at the end of a frame.
    fn perform_cleanup(&self) {
        let mut stats = self.rendering_stats();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {}

    /// `createOffscreenTexture(size, type)`: neither depth nor stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(OffscreenTexture::new(self, size, type_))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc(), self.get_command_buffer()))
    }

    /// See `PaintParameters::clearStencil`.
    fn clear_stencil_buffer(&self, _value: i32) {
        debug_assert!(false);
    }

    fn set_dirty_state(&self) {}

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr {
        VertexAttributeArray::new_ptr()
    }

    fn create_drawable_builder(&self, name: String) -> UniqueDrawableBuilder {
        Box::new(DrawableBuilder::new(name))
    }

    fn create_uniform_buffer(&self, data: &[u8], persistent: bool, ssbo: bool) -> UniformBufferPtr {
        Rc::new(UniformBuffer::create(self, data, persistent, ssbo))
    }

    fn create_layer_uniform_buffer_array(&self) -> UniqueUniformBufferArray {
        Box::new(UniformBufferArray::new())
    }

    fn get_generic_shader(
        &self,
        registry: &ShaderRegistry,
        name: &str,
        variant: ProjectionVariant,
    ) -> Option<ShaderProgramBasePtr> {
        let shader_group = registry.get_shader_group(name)?;
        shader_group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "")
    }

    fn create_tile_layer_group(
        &self,
        layer_index: i32,
        initial_capacity: usize,
        name: String,
    ) -> Option<LayerGroupBasePtr> {
        Some(Rc::new(RefCell::new(TileLayerGroup::new(layer_index, initial_capacity, name))))
    }

    fn create_layer_group(&self, layer_index: i32, initial_capacity: usize, name: String) -> Option<LayerGroupBasePtr> {
        Some(Rc::new(RefCell::new(LayerGroup::new(layer_index, initial_capacity, name))))
    }

    fn create_texture_2d(&self) -> Texture2DPtr {
        self.create_vulkan_texture_2d()
    }

    fn reset_state(&self, _depth_mode: DepthMode, _color_mode: ColorMode) {}

    fn emplace_or_update_uniform_buffer(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &[u8],
        persistent: bool,
    ) -> bool {
        match ptr {
            Some(buffer) => {
                buffer.update(data);
                false
            }
            None => {
                *ptr = Some(self.create_uniform_buffer(data, persistent, false));
                true
            }
        }
    }

    fn get_global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow()
    }

    fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow_mut()
    }

    fn bind_global_uniform_buffers(&self, render_pass: &dyn mln_gfx::render_pass::RenderPass) {
        let Some(render_pass) = render_pass.as_any().downcast_ref::<RenderPass>() else {
            return;
        };
        let mut descriptor_set = self.global_descriptor_set.borrow_mut();
        let descriptor_set =
            descriptor_set.get_or_insert_with(|| UniformDescriptorSet::new(self, DescriptorSetType::Global));
        descriptor_set.bind(self, render_pass.get_command_buffer(), &self.global_uniform_buffers.borrow());
    }

    fn unbind_global_uniform_buffers(&self, _render_pass: &dyn mln_gfx::render_pass::RenderPass) {}

    fn render_tile_clipping_masks(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        self.render_tile_clipping_masks_impl(render_pass, shaders, tile_ubos)
    }

    fn release_globe_clip_masks(&self) {
        self.globe_clipping.borrow_mut().meshes.clear();
    }

    fn render_globe_tile_clipping_masks(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &dyn Fn(&CanonicalTileID, bool) -> (Vec<u8>, Vec<u16>),
    ) -> bool {
        self.render_globe_tile_clipping_masks_impl(render_pass, shaders, masks, mesh)
    }
}
