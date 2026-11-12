// A ring of the host's images that frames are drawn into and given back to
// the host, with what the host waits for before it reads them, and what the
// target waits for before it draws into an image the host read.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ash::vk;
use gpu_allocator::MemoryLocation;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{ConsumerSync, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::{Context, RenderPassKey};
use crate::device::Device;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::{BackendImpl, RendererBackend};
use crate::texture2d::ImageAllocation;

/// The queue family that a borrowed image goes to after each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageOwner {
    /// The image stays with the device's queue family.
    #[default]
    GraphicsQueue,
    /// Each frame releases the image to `VK_QUEUE_FAMILY_EXTERNAL`.
    External,
    /// Each frame releases the image to `VK_QUEUE_FAMILY_FOREIGN_EXT`.
    Foreign,
}

/// One of the host's images, and a 2D view of it with one mip level and one
/// layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BorrowedImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
}

/// A ring of the host's images, all of `size` physical pixels and `format`,
/// with `VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedImages {
    pub images: Vec<BorrowedImage>,
    pub size: Size,
    pub format: vk::Format,
    /// The layout that each frame leaves its image in.
    pub final_layout: vk::ImageLayout,
    pub owner: ImageOwner,
}

impl BorrowedImages {
    /// The kinds of sync with which the host can wait for a frame (the
    /// producer's) and the target for the host's reads (the consumer's), on
    /// `device`.
    pub fn sync_kinds(device: &Device) -> (SyncKinds, SyncKinds) {
        device.sync_kinds()
    }

    /// Checks what the images need of `device`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// images that go to the foreign queue family on a device without
    /// `VK_EXT_queue_family_foreign`.
    pub fn check(&self, device: &Device) -> Result<(), Error> {
        if self.owner == ImageOwner::Foreign && !device.features().foreign_queue_family {
            return Err(Error::unsupported("the Vulkan device has no foreign queue family"));
        }
        Ok(())
    }
}

/// The render pass and framebuffers over the ring's images, and their depth
/// and stencil attachment.
struct Resource {
    backend: Rc<BackendImpl>,
    extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    /// The image the next frame draws into.
    selected: Cell<usize>,
    _depth: ImageAllocation,
}

impl Resource {
    fn new(context: &Context, images: &BorrowedImages) -> Result<Resource, Error> {
        let backend = context.get_backend().clone();
        let extent = vk::Extent2D { width: images.size.width, height: images.size.height };
        let formats = [vk::Format::D24_UNORM_S8_UINT, vk::Format::D32_SFLOAT_S8_UINT, vk::Format::D16_UNORM_S8_UINT];
        let depth_format = formats
            .into_iter()
            .find(|&format| {
                // SAFETY: a plain query of the physical device.
                let properties = unsafe {
                    backend.get_instance().get_physical_device_format_properties(backend.get_physical_device(), format)
                };
                properties.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
            })
            .ok_or_else(|| Error::unsupported("Depth/Stencil format not available"))?;
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(depth_format)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let mut depth = ImageAllocation::create(&backend, &image_info, MemoryLocation::GpuOnly, "depth")
            .ok_or_else(|| Error::native("Vulkan depth texture allocation failed"))?;
        if !depth.create_view(
            depth_format,
            vk::ComponentMapping::default(),
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
            1,
        ) {
            return Err(Error::native("Vulkan depth texture view creation failed"));
        }
        let render_pass = context.get_render_pass(RenderPassKey {
            color_format: images.format,
            depth_format: Some(depth_format),
            color_final_layout: images.final_layout,
            color_load: false,
        });
        if render_pass == vk::RenderPass::null() {
            return Err(Error::native("Failed to create a render pass for the borrowed images"));
        }
        let mut resource =
            Resource { backend, extent, render_pass, framebuffers: Vec::new(), selected: Cell::new(0), _depth: depth };
        for image in &images.images {
            let attachments = [image.view, resource._depth.image_view()];
            let create_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            // SAFETY: the host's view and the depth view are live, of the
            // render pass's formats and of the framebuffer's size.
            let framebuffer = unsafe { resource.backend.get_device().create_framebuffer(&create_info, None) }
                .map_err(|e| Error::native(format!("Vulkan framebuffer creation failed: {e}")))?;
            resource.framebuffers.push(framebuffer);
        }
        Ok(resource)
    }
}

impl TargetResource for Resource {
    fn bind(&self) {}

    fn get_extent(&self) -> vk::Extent2D {
        self.extent
    }

    fn get_render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    fn get_framebuffer(&self) -> vk::Framebuffer {
        self.framebuffers.get(self.selected.get()).copied().unwrap_or_default()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        // SAFETY: the backend waited for the frames that used the
        // framebuffers (`BorrowedTarget`'s replacement and drop).
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                self.backend.get_device().destroy_framebuffer(framebuffer, None);
            }
        }
    }
}

/// What the next frame drawn into a slot's image waits for.
#[derive(Clone, Copy)]
enum ConsumerWait {
    /// The host's timeline, until it reaches the value.
    Timeline(vk::Semaphore, u64),
    /// A semaphore of the target's own that holds an imported sync file.
    #[cfg_attr(not(unix), allow(dead_code))]
    Imported(vk::Semaphore),
}

/// What a slot of the ring waits for, and what it handed the host.
#[derive(Default)]
struct Slot {
    consumer_wait: Option<ConsumerWait>,
    /// The sync file of the slot's last frame, which the host may read until
    /// it gives the slot back.
    #[cfg(unix)]
    producer_fd: Option<std::os::fd::OwnedFd>,
}

/// Frames drawn into the host's images, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    images: RefCell<BorrowedImages>,
    resource: RefCell<Rc<Resource>>,
    target: RefCell<Rc<DrawTarget>>,
    producer: SyncKind,
    slots: RefCell<Vec<Slot>>,
    /// The timeline that frames signal, for a timeline producer.
    timeline: vk::Semaphore,
    timeline_value: Cell<u64>,
    /// The binary semaphore that frames signal, for a sync file producer.
    export_semaphore: vk::Semaphore,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `images` and hands each frame
    /// to the host with sync of the kind `producer`, which must be one of
    /// [`BorrowedImages::sync_kinds`]'s producer kinds.
    ///
    /// Fails as [`BorrowedImages::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// the device doesn't have, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    pub fn new(backend: Rc<RendererBackend>, images: BorrowedImages, producer: SyncKind) -> Result<Self, Error> {
        let device = backend.backend_impl().device().clone();
        images.check(&device)?;
        if !device.sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("the Vulkan device can't hand frames over with {producer:?}")));
        }
        let context = backend.vulkan_context().clone();
        let resource = Rc::new(Resource::new(&context, &images)?);
        let target = Rc::new(DrawTarget::new(images.size, resource.clone()));
        let mut this = BorrowedTarget {
            backend,
            slots: RefCell::new(Vec::new()),
            images: RefCell::new(images),
            resource: RefCell::new(resource),
            target: RefCell::new(target),
            producer,
            timeline: vk::Semaphore::null(),
            timeline_value: Cell::new(0),
            export_semaphore: vk::Semaphore::null(),
        };
        let vk_device = this.backend.backend_impl().get_device().clone();
        // SAFETY: the create infos are valid for the device, which has the
        // features of the producer kind (checked above).
        unsafe {
            match producer {
                SyncKind::VulkanTimelineSemaphore => {
                    let mut type_info = vk::SemaphoreTypeCreateInfo::default()
                        .semaphore_type(vk::SemaphoreType::TIMELINE)
                        .initial_value(0);
                    let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
                    this.timeline = vk_device
                        .create_semaphore(&info, None)
                        .map_err(|e| Error::native(format!("Failed to create a timeline semaphore: {e}")))?;
                }
                SyncKind::SyncFd => {
                    let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                    let info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
                    this.export_semaphore = vk_device
                        .create_semaphore(&info, None)
                        .map_err(|e| Error::native(format!("Failed to create an exportable semaphore: {e}")))?;
                }
                _ => {}
            }
        }
        this.reset_slots();
        Ok(this)
    }

    /// How many images the ring has.
    pub fn depth(&self) -> usize {
        self.images.borrow().images.len()
    }

    /// The images' size, in physical pixels.
    pub fn size(&self) -> Size {
        self.images.borrow().size
    }

    /// The images of the ring.
    pub fn images(&self) -> BorrowedImages {
        self.images.borrow().clone()
    }

    /// The kind of sync that frames are handed over with.
    pub fn producer(&self) -> SyncKind {
        self.producer
    }

    /// Has the next frame draw into the image at `slot`.
    pub fn select(&self, slot: usize) {
        self.resource.borrow().selected.set(slot);
    }

    /// The host gave the image at `slot` back: the next frame drawn into it
    /// waits for `consumer`, a sync of one of
    /// [`BorrowedImages::sync_kinds`]'s consumer kinds. The sync file of the
    /// slot's last frame is closed.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when the target
    /// can't wait for `consumer`, such as for a sync file Vulkan can't
    /// import. The host's GPU may then still read the image.
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        use ash::vk::Handle as _;
        let mut slots = self.slots.borrow_mut();
        let Some(state) = slots.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        #[cfg(unix)]
        {
            state.producer_fd = None;
        }
        self.forget_wait(state);
        #[cfg(unix)]
        let ConsumerSync { sync: consumer, sync_file } = consumer;
        #[cfg(not(unix))]
        let ConsumerSync { sync: consumer } = consumer;
        match consumer.kind {
            SyncKind::CpuComplete => Ok(()),
            SyncKind::QueueOrder => {
                // The host's reads went to the queue before the next frame,
                // which waits for them before it clears the image.
                self.backend.vulkan_context().wait_for_queue_in_next_frame();
                Ok(())
            }
            SyncKind::VulkanTimelineSemaphore => {
                state.consumer_wait =
                    Some(ConsumerWait::Timeline(vk::Semaphore::from_raw(consumer.handle), consumer.value));
                Ok(())
            }
            #[cfg(unix)]
            SyncKind::SyncFd => {
                use std::os::fd::{FromRawFd as _, IntoRawFd as _, OwnedFd};
                let fd = sync_file.ok_or_else(|| Error::invalid_argument("a sync file release names no file"))?;
                let backend = self.backend.backend_impl();
                let loader = backend
                    .get_semaphore_fd_loader()
                    .ok_or_else(|| Error::native("the Vulkan device imports no sync files"))?;
                let device = backend.get_device();
                // SAFETY: the create info is valid for the device.
                let semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                    .map_err(|e| Error::native(format!("Failed to create a semaphore: {e}")))?;
                let raw = fd.into_raw_fd();
                let info = vk::ImportSemaphoreFdInfoKHR::default()
                    .semaphore(semaphore)
                    .flags(vk::SemaphoreImportFlags::TEMPORARY)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                    .fd(raw);
                // SAFETY: the semaphore is new; Vulkan owns the descriptor
                // once the import succeeds.
                match unsafe { loader.import_semaphore_fd(&info) } {
                    Ok(()) => {
                        state.consumer_wait = Some(ConsumerWait::Imported(semaphore));
                        Ok(())
                    }
                    Err(error) => {
                        // SAFETY: the import failed, so the descriptor is
                        // still the target's, and nothing uses the
                        // semaphore.
                        unsafe {
                            drop(OwnedFd::from_raw_fd(raw));
                            device.destroy_semaphore(semaphore, None);
                        }
                        Err(Error::native(format!("Failed to import the host's sync file: {error}")))
                    }
                }
            }
            kind => Err(Error::unsupported(format!("the Vulkan target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the image at `slot` without saying when its reads
    /// end: the slot's sync file is closed, and the image is never drawn
    /// into again.
    pub fn discard(&self, slot: usize) {
        if let Some(state) = self.slots.borrow_mut().get_mut(slot) {
            #[cfg(unix)]
            {
                state.producer_fd = None;
            }
            self.forget_wait(state);
        }
    }

    /// Drops what `slot` waits for, destroying a semaphore of the target's
    /// that no frame waits on.
    fn forget_wait(&self, slot: &mut Slot) {
        if let Some(ConsumerWait::Imported(semaphore)) = slot.consumer_wait.take() {
            // SAFETY: no submitted frame waits on the semaphore, which the
            // target made.
            unsafe { self.backend.backend_impl().get_device().destroy_semaphore(semaphore, None) };
        }
    }

    /// Draws into `images` from now on, which must have as many images, and
    /// the format, final layout and owner of the ring's.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of images, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another format,
    /// final layout or owner, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    /// The target then keeps its images.
    pub fn replace(&self, images: BorrowedImages) -> Result<(), Error> {
        {
            let current = self.images.borrow();
            if images.images.len() != current.images.len() {
                return Err(Error::invalid_argument("a ring keeps its depth"));
            }
            if (images.format, images.final_layout, images.owner)
                != (current.format, current.final_layout, current.owner)
            {
                return Err(Error::unsupported("a ring keeps its format, final layout and owner"));
            }
        }
        // The frames that drew into the old images are done before their
        // framebuffers go.
        self.backend.backend_impl().wait_idle("borrowed image replacement");
        let resource = Rc::new(Resource::new(self.backend.vulkan_context(), &images)?);
        *self.target.borrow_mut() = Rc::new(DrawTarget::new(images.size, resource.clone()));
        *self.resource.borrow_mut() = resource;
        *self.images.borrow_mut() = images;
        self.reset_slots();
        Ok(())
    }

    /// Forgets what each slot waits for and handed over.
    fn reset_slots(&self) {
        let depth = self.depth();
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            self.forget_wait(slot);
            #[cfg(unix)]
            {
                slot.producer_fd = None;
            }
        }
        slots.resize_with(depth, Slot::default);
    }

    /// Releases the frame's image to the host's queue family, after the
    /// frame's passes.
    fn record_release(&self, context: &Context) {
        let images = self.images.borrow();
        let destination = match images.owner {
            ImageOwner::GraphicsQueue => return,
            ImageOwner::External => vk::QUEUE_FAMILY_EXTERNAL,
            ImageOwner::Foreign => vk::QUEUE_FAMILY_FOREIGN_EXT,
        };
        let Some(image) = images.images.get(self.resource.borrow().selected.get()) else {
            return;
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(images.final_layout)
            .new_layout(images.final_layout)
            .src_queue_family_index(self.backend.backend_impl().get_graphics_queue_index())
            .dst_queue_family_index(destination)
            .image(image.image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: the frame's command buffer is recording outside a render
        // pass, and the image is the frame's, in its final layout.
        unsafe {
            self.backend.backend_impl().get_device().cmd_pipeline_barrier(
                context.get_command_buffer(),
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    /// The sync the host waits for, once the frame was submitted.
    #[cfg_attr(not(unix), allow(unused_variables))]
    fn producer_sync(&self, context: &Context, slot: usize) -> Option<GpuSync> {
        use ash::vk::Handle as _;
        match self.producer {
            SyncKind::CpuComplete => {
                if !context.wait_frame() {
                    Log::error(Event::Render, "Waiting for a borrowed image's frame failed");
                    return None;
                }
                Some(GpuSync::of(SyncKind::CpuComplete))
            }
            SyncKind::QueueOrder => Some(GpuSync::of(SyncKind::QueueOrder)),
            SyncKind::VulkanTimelineSemaphore => Some(GpuSync {
                kind: SyncKind::VulkanTimelineSemaphore,
                handle: self.timeline.as_raw(),
                value: self.timeline_value.get(),
            }),
            #[cfg(unix)]
            SyncKind::SyncFd => {
                use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
                let loader = self.backend.backend_impl().get_semaphore_fd_loader()?;
                let info = vk::SemaphoreGetFdInfoKHR::default()
                    .semaphore(self.export_semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                // SAFETY: the submitted frame signals the semaphore, which
                // the export unsignals for the next frame.
                let fd = match unsafe { loader.get_semaphore_fd(&info) } {
                    Ok(fd) => fd,
                    Err(error) => {
                        Log::error(Event::Render, format!("Exporting a frame's sync file failed: {error}"));
                        return None;
                    }
                };
                // SAFETY: the export made the descriptor, which is the
                // target's.
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                let raw = fd.as_raw_fd();
                if let Some(state) = self.slots.borrow_mut().get_mut(slot) {
                    state.producer_fd = Some(fd);
                }
                Some(GpuSync { kind: SyncKind::SyncFd, handle: raw as u64, value: 0 })
            }
            _ => None,
        }
    }
}

impl FrameTarget for BorrowedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        Some(self.target.borrow().clone())
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        let context = self.backend.vulkan_context();
        if !context.frame_has_image() {
            context.submit_frame();
            return Handoff::Dropped;
        }
        let slot = self.resource.borrow().selected.get();
        self.record_release(context);
        match self.slots.borrow_mut().get_mut(slot).and_then(|s| s.consumer_wait.take()) {
            Some(ConsumerWait::Timeline(semaphore, value)) => context.wait_in_frame(semaphore, Some(value)),
            Some(ConsumerWait::Imported(semaphore)) => {
                context.wait_in_frame(semaphore, None);
                // SAFETY: destroyed once the frame that waits on it is done.
                context.enqueue_deletion(Box::new(move |context: &Context| unsafe {
                    context.get_backend().get_device().destroy_semaphore(semaphore, None);
                }));
            }
            None => {}
        }
        match self.producer {
            SyncKind::VulkanTimelineSemaphore => {
                let value = self.timeline_value.get() + 1;
                self.timeline_value.set(value);
                context.signal_in_frame(self.timeline, Some(value));
            }
            SyncKind::SyncFd => context.signal_in_frame(self.export_semaphore, None),
            _ => {}
        }
        context.submit_frame();
        match self.producer_sync(context, slot) {
            Some(sync) => Handoff::GivenBack { slot, sync },
            None => Handoff::Dropped,
        }
    }
}

impl Drop for BorrowedTarget {
    fn drop(&mut self) {
        let backend = self.backend.backend_impl();
        backend.wait_idle("borrowed target deletion");
        let device = backend.get_device();
        // SAFETY: the backend's work is done, and the semaphores are the
        // target's.
        unsafe {
            for mut slot in std::mem::take(&mut *self.slots.borrow_mut()) {
                self.forget_wait(&mut slot);
            }
            if self.timeline != vk::Semaphore::null() {
                device.destroy_semaphore(self.timeline, None);
            }
            if self.export_semaphore != vk::Semaphore::null() {
                device.destroy_semaphore(self.export_semaphore, None);
            }
        }
    }
}
