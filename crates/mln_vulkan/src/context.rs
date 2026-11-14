// Resources hold a `Weak<Context>` where C++ holds `Context&`, and the
// device through `Rc<BackendImpl>`: a resource that outlives its context is
// destroyed when dropped, instead of through the context's deletion queue.
//
// Frames go through the context's `VulkanGpu`, and their targets acquire and
// present their images themselves; the context keeps the descriptor set and
// pipeline layouts the `Gpu` is made with, and what the native resources of
// an overlay (`Texture2D`, `BufferResource`) need: uploads, and a deletion
// queue that waits for the submits that may use them.

use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::logging::{Event, Log};

use crate::buffer_resource::BufferResource;
use crate::device::Device;
use crate::gpu::VulkanGpu;
use crate::renderer_backend::BackendImpl;
use crate::shaders::layout::{DescriptorSetType, IMAGE_TEXTURE_COUNT, image_sampler_binding, image_texture_binding};
use crate::texture2d::Texture2D;

/// Destroys something once the submits that may use it are done.
pub(crate) type DeletionTask = Box<dyn FnOnce(&Context)>;

pub struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    /// The `Gpu` the renderer's resources belong to and its frames are
    /// submitted through.
    gpu: RefCell<VulkanGpu>,
    /// What to destroy once the `Gpu`'s submit of each serial has finished.
    deletions: RefCell<Vec<(u64, DeletionTask)>>,
    destroyed: Cell<bool>,
}

pub(crate) fn build_descriptor_set_layout(
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

pub(crate) fn build_pipeline_layout(
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
    /// Creates the context of `backend`, or an error if the device can't make
    /// its `Gpu`.
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Result<Rc<Context>, String> {
        let gpu = VulkanGpu::on(Rc::clone(&backend)).map_err(|error| error.to_string())?;
        Ok(Rc::new_cyclic(|weak| Context {
            base: ContextBase::default(),
            backend,
            weak_self: Weak::clone(weak),
            gpu: RefCell::new(gpu),
            deletions: RefCell::new(Vec::new()),
            destroyed: Cell::new(false),
        }))
    }

    pub fn weak(&self) -> Weak<Context> {
        Weak::clone(&self.weak_self)
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// The device the context renders on.
    pub fn device(&self) -> &Device {
        self.backend.device()
    }

    /// Waits for the device, runs the deletion queue and destroys what the
    /// context made. Called before the backend lets go of the context, and when
    /// it is dropped.
    pub(crate) fn destroy_resources(&self) {
        if self.destroyed.replace(true) {
            return;
        }
        self.backend.wait_idle("context shutdown");
        *self.mutable_global_uniform_buffers() = UniformBufferArray::new();
        self.release_cached_resources();
        self.destroy_released();
        // A deletion can enqueue another, which runs at once.
        let tasks = std::mem::take(&mut *self.deletions.borrow_mut());
        for (_, task) in tasks {
            task(self);
        }
    }

    /// The `Gpu` the renderer's resources belong to and its frames are
    /// submitted through.
    pub(crate) fn vulkan_gpu_mut(&self) -> RefMut<'_, VulkanGpu> {
        self.gpu.borrow_mut()
    }

    /// Runs `function` once the `Gpu`'s submits so far, and the next one,
    /// which takes what is being recorded, are done.
    pub fn enqueue_deletion(&self, function: DeletionTask) {
        if self.destroyed.get() {
            function(self);
            return;
        }
        let serial = self.gpu.borrow().last_serial() + 1;
        self.deletions.borrow_mut().push((serial, function));
    }

    /// Runs the deletions whose submits are done.
    fn run_deletions(&self) {
        let finished = self.gpu.borrow_mut().finished_serial();
        // Not borrowed while the tasks run: they can enqueue more.
        let (done, waiting) =
            std::mem::take(&mut *self.deletions.borrow_mut()).into_iter().partition(|(serial, _)| *serial <= finished);
        self.deletions.borrow_mut().extend::<Vec<_>>(waiting);
        let done: Vec<(u64, DeletionTask)> = done;
        for (_, task) in done {
            task(self);
        }
    }

    /// Records `function`'s commands into a command buffer of their own, and
    /// waits for them.
    pub(crate) fn submit_one_time_command(&self, function: impl FnOnce(vk::CommandBuffer)) {
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
                        let done = self
                            .backend
                            .submit(&[submit_info], fence)
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

    /// Records an upload's commands into a command buffer that the `Gpu`
    /// submits after the work submitted before it, or else into one of its
    /// own, which is waited for. Returns whether the commands have run.
    pub(crate) fn record_upload(&self, function: impl FnOnce(vk::CommandBuffer)) -> bool {
        let recording = self.gpu.borrow_mut().begin_recording();
        let command_buffer = match recording {
            Ok(command_buffer) => command_buffer,
            Err(error) => {
                Log::error(Event::Render, format!("Recording an upload failed: {error}"));
                self.submit_one_time_command(function);
                return true;
            }
        };
        function(command_buffer);
        if let Err(error) = self.gpu.borrow_mut().submit_recording(command_buffer) {
            Log::error(Event::Render, format!("Submitting an upload failed: {error}"));
        }
        false
    }

    pub fn create_buffer(&self, data: Option<&[u8]>, size: usize, usage: vk::BufferUsageFlags) -> BufferResource {
        BufferResource::new(self, data, size, usage)
    }

    /// Creates a texture, of the concrete type.
    pub fn create_vulkan_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self)))
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

    fn backend_type(&self) -> BackendType {
        BackendType::Vulkan
    }

    fn begin_frame(&self) {}

    /// Runs the deletions whose submits are done.
    fn end_frame(&self) {
        self.run_deletions();
    }

    fn reduce_memory_usage(&self) {}

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
