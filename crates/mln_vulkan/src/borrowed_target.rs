// A ring of the host's images that frames are drawn into and given back to
// the host, with what the host waits for before it reads them, and what the
// target waits for before it draws into an image the host read.

use std::rc::Rc;
use std::time::Instant;

use ash::vk;
use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{
    Gpu as _, ResourceId, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage, destroy_unused,
};
use mln_gfx::{ConsumerSync, GpuSync, SyncKind};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::device::Device;
use crate::gpu::{ImportedImage, VulkanGpu};
use crate::renderer_backend::{BackendImpl, RendererBackend};

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
    /// The images that frames draw their depth and stencil into and store,
    /// or `None` for depth of the target's own, which frames discard.
    pub depth: Option<BorrowedDepth>,
}

/// The host's depth and stencil images of a ring, one for the color image
/// of each slot, of the ring's size, with
/// `VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT`, and views of both aspects.
/// They go to the ring's owner as its color images do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedDepth {
    pub images: Vec<BorrowedImage>,
    pub format: vk::Format,
    /// The layout that each frame leaves its depth image in.
    pub final_layout: vk::ImageLayout,
}

impl BorrowedImages {
    /// Checks what the images need of `device`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// images that go to the foreign queue family on a device without
    /// `VK_EXT_queue_family_foreign`, or depth images of a format that the
    /// device draws no depth into, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a size
    /// that is zero or larger than the device's largest image, or for depth
    /// images that aren't one for each image.
    pub fn check(&self, device: &Device) -> Result<(), Error> {
        let max = device.shared.properties().limits.max_image_dimension2_d;
        let Size { width, height } = self.size;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(Error::invalid_argument(format!(
                "a ring's images of {width}x{height}, where the Vulkan device's are 1 to {max} on a side"
            )));
        }
        if self.owner == ImageOwner::Foreign && !device.features().foreign_queue_family {
            return Err(Error::unsupported("the Vulkan device has no foreign queue family"));
        }
        if let Some(depth) = &self.depth {
            if depth.images.len() != self.images.len() {
                return Err(Error::invalid_argument("a ring's depth images must be one for each of its images"));
            }
            if !crate::device::depth_stencil_format(depth.format)
                .is_some_and(|format| device.depth_stencil_formats().contains(format))
            {
                return Err(Error::unsupported(format!(
                    "the Vulkan device draws no depth and stencil into {:?}",
                    depth.format
                )));
            }
        }
        Ok(())
    }
}

/// The ring's images as textures of the backend's `Gpu`, with their depth
/// and stencil attachments: the host's, or one of the target's own.
pub(crate) struct Imports {
    size: Size,
    targets: TargetFormats,
    color: Vec<TextureId>,
    /// The host's depth and stencil images, one for each slot.
    depth: Vec<TextureId>,
    /// The target's own depth and stencil, when the host gave none.
    own_depth: Option<TextureId>,
}

impl Imports {
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// format the `Gpu` has no texture of, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it can't make the
    /// depth and stencil texture.
    pub(crate) fn new(gpu: &mut VulkanGpu, images: &BorrowedImages) -> Result<Imports, Error> {
        let mut imports = Imports {
            size: images.size,
            targets: TargetFormats { color: None, depth_stencil: Some(TextureFormat::Depth24PlusStencil8) },
            color: Vec::new(),
            depth: Vec::new(),
            own_depth: None,
        };
        match imports.import(gpu, images) {
            Ok(()) => Ok(imports),
            Err(error) => {
                imports.tear_down(gpu);
                Err(error)
            }
        }
    }

    fn import(&mut self, gpu: &mut VulkanGpu, images: &BorrowedImages) -> Result<(), Error> {
        let release_to = match images.owner {
            ImageOwner::GraphicsQueue => None,
            ImageOwner::External => Some(vk::QUEUE_FAMILY_EXTERNAL),
            ImageOwner::Foreign => Some(vk::QUEUE_FAMILY_FOREIGN_EXT),
        };
        self.targets.color = Some(
            gpu.texture_format(images.format)
                .ok_or_else(|| Error::unsupported(format!("the Vulkan target draws into no {:?}", images.format)))?,
        );
        let attachment = TextureUsage { sampled: false, render_target: true, copy_src: false };
        let import = |gpu: &mut VulkanGpu, image: &BorrowedImage, format, layout| {
            gpu.import_image(&ImportedImage {
                image: image.image,
                view: image.view,
                format,
                size: images.size,
                usage: attachment,
                layout,
                release_to,
            })
            .map_err(|error| Error::native(format!("Importing a borrowed image failed: {error}")))
        };
        for image in &images.images {
            self.color.push(import(gpu, image, images.format, images.final_layout)?);
        }
        if let Some(depth) = &images.depth {
            self.targets.depth_stencil = gpu.texture_format(depth.format);
            for image in &depth.images {
                self.depth.push(import(gpu, image, depth.format, depth.final_layout)?);
            }
        } else {
            let texture = gpu
                .create_texture(&TextureDesc {
                    label: "BorrowedDepth",
                    size: images.size,
                    format: TextureFormat::Depth24PlusStencil8,
                    usage: attachment,
                })
                .map_err(|error| Error::native(format!("Vulkan depth texture allocation failed: {error}")))?;
            self.own_depth = Some(texture);
        }
        Ok(())
    }

    /// The textures that a frame drawn into `slot` draws into.
    fn frame_image(&self, slot: usize) -> Option<FrameImage> {
        Some(FrameImage {
            size: self.size,
            color: *self.color.get(slot)?,
            depth_stencil: self.own_depth.or_else(|| self.depth.get(slot).copied()),
            targets: self.targets,
        })
    }
}

impl Imports {
    /// Destroys the imports in `gpu`, which keeps them until the work that
    /// uses them finishes.
    pub(crate) fn tear_down(&mut self, gpu: &mut VulkanGpu) {
        let textures = self.color.drain(..).chain(self.depth.drain(..)).chain(self.own_depth.take());
        destroy_unused(gpu, textures.map(ResourceId::Texture));
    }
}

/// What the next frame drawn into a slot's image waits for.
enum ConsumerWait {
    /// The host's timeline, until it reaches the value.
    Timeline(vk::Semaphore, u64),
    /// A semaphore of the target's own that holds an imported sync file, and
    /// the file again, which a release of the image polls.
    #[cfg(unix)]
    Imported { semaphore: vk::Semaphore, file: std::os::fd::OwnedFd },
    /// The host's D3D12 fence, imported, until it reaches the value.
    #[cfg(windows)]
    Fence(Rc<ImportedFence>, u64),
}

/// A timeline semaphore of the target's own that shares a D3D12 fence of the
/// host's, which dropping destroys.
#[cfg(windows)]
struct ImportedFence {
    device: ash::Device,
    semaphore: vk::Semaphore,
}

#[cfg(windows)]
impl ImportedFence {
    /// The fence whose NT handle is `handle`, imported into a new timeline.
    /// The handle stays the host's.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when Vulkan
    /// can't import it.
    fn import(backend: &BackendImpl, handle: u64) -> Result<ImportedFence, Error> {
        let loader = backend
            .device()
            .shared
            .semaphore_win32()
            .ok_or_else(|| Error::native("the Vulkan device imports no D3D12 fences"))?;
        let device = backend.get_device().clone();
        let mut type_info = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
        // SAFETY: the create info is valid for the device, which has
        // timelines when it imports D3D12 fences.
        let semaphore = unsafe { device.create_semaphore(&info, None) }
            .map_err(|e| Error::native(format!("Failed to create a timeline semaphore: {e}")))?;
        let fence = ImportedFence { device, semaphore };
        let import = vk::ImportSemaphoreWin32HandleInfoKHR::default()
            .semaphore(semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE)
            .handle(handle as vk::HANDLE);
        // SAFETY: the semaphore is new, and an import of an NT handle leaves
        // the handle to its owner.
        unsafe { loader.import_semaphore_win32_handle(&import) }
            .map_err(|e| Error::native(format!("Failed to import the host's D3D12 fence: {e}")))?;
        Ok(fence)
    }
}

#[cfg(windows)]
impl Drop for ImportedFence {
    fn drop(&mut self) {
        // SAFETY: the last frame that waited on the semaphore is done: its
        // target waited for the queue before it let go of its fences.
        unsafe { self.device.destroy_semaphore(self.semaphore, None) };
    }
}

/// What a slot of the ring waits for, and what it handed the host.
#[derive(Default)]
struct Slot {
    consumer_wait: Option<ConsumerWait>,
    /// The texture that has the next pass that draws into it wait for
    /// `consumer_wait`, from the frame begun on the slot until it ends.
    attached: Option<TextureId>,
    /// The sync file of the slot's last frame, which the host may read until
    /// it gives the slot back.
    #[cfg(unix)]
    producer_fd: Option<std::os::fd::OwnedFd>,
}

/// Frames drawn into the host's images, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<BackendImpl>,
    images: BorrowedImages,
    imports: Imports,
    /// The slot the next frame draws into.
    selected: usize,
    producer: SyncKind,
    slots: Vec<Slot>,
    /// The timeline that frames signal, for a timeline or D3D12 fence
    /// producer.
    timeline: vk::Semaphore,
    timeline_value: u64,
    /// The binary semaphore that frames signal, for a sync file producer.
    export_semaphore: vk::Semaphore,
    /// The NT handle that shares `timeline` as a D3D12 fence, for a D3D12
    /// fence producer.
    #[cfg(windows)]
    shared_fence: Option<crate::win32::OwnedHandle>,
    /// The host's D3D12 fences that releases named, by their handles.
    #[cfg(windows)]
    imported_fences: Vec<(u64, Rc<ImportedFence>)>,
    /// Whether a tear-down waited for the device's work, after which the
    /// target's semaphores are free to destroy.
    torn_down: bool,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `images` and hands each frame
    /// to the host with sync of the kind `producer`, which must be one of
    /// [`Device::sync_kinds`](crate::Device::sync_kinds)'s producer kinds.
    ///
    /// Fails as [`BorrowedImages::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// the device doesn't have, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    pub fn new(backend: &mut RendererBackend, images: BorrowedImages, producer: SyncKind) -> Result<Self, Error> {
        use mln_gfx::RendererBackend as _;
        let backend_impl = Rc::clone(backend.backend_impl());
        let device = backend_impl.device().clone();
        images.check(&device)?;
        if !device.sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("the Vulkan device can't hand frames over with {producer:?}")));
        }
        let gpu = backend.gpu();
        let imports = Imports::new(gpu, &images)?;
        let mut this = BorrowedTarget {
            backend: backend_impl,
            slots: Vec::new(),
            images,
            imports,
            selected: 0,
            producer,
            timeline: vk::Semaphore::null(),
            timeline_value: 0,
            export_semaphore: vk::Semaphore::null(),
            #[cfg(windows)]
            shared_fence: None,
            #[cfg(windows)]
            imported_fences: Vec::new(),
            torn_down: false,
        };
        if let Err(error) = this.create_sync() {
            this.tear_down(gpu);
            return Err(error);
        }
        drop(this.reset_slots(&[]));
        Ok(this)
    }

    /// Makes what frames signal for the target's producer kind, which the
    /// device has.
    fn create_sync(&mut self) -> Result<(), Error> {
        let producer = self.producer;
        let vk_device = self.backend.get_device().clone();
        // SAFETY: the create infos are valid for the device, which has the
        // features of the producer kind (checked above).
        unsafe {
            match producer {
                #[cfg(windows)]
                SyncKind::D3d12Fence => {
                    let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE);
                    let mut type_info = vk::SemaphoreTypeCreateInfo::default()
                        .semaphore_type(vk::SemaphoreType::TIMELINE)
                        .initial_value(0);
                    let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info).push_next(&mut export_info);
                    self.timeline = vk_device
                        .create_semaphore(&info, None)
                        .map_err(|e| Error::native(format!("Failed to create a shareable timeline: {e}")))?;
                    let loader = self
                        .backend
                        .device()
                        .shared
                        .semaphore_win32()
                        .ok_or_else(|| Error::unsupported("the Vulkan device exports no D3D12 fences"))?;
                    let get = vk::SemaphoreGetWin32HandleInfoKHR::default()
                        .semaphore(self.timeline)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE);
                    let handle = loader
                        .get_semaphore_win32_handle(&get)
                        .map_err(|e| Error::native(format!("Failed to share the timeline as a D3D12 fence: {e}")))?;
                    // The export makes an NT handle, which is the target's.
                    self.shared_fence = Some(crate::win32::OwnedHandle::from_raw(handle));
                }
                SyncKind::VulkanTimelineSemaphore => {
                    let mut type_info = vk::SemaphoreTypeCreateInfo::default()
                        .semaphore_type(vk::SemaphoreType::TIMELINE)
                        .initial_value(0);
                    let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
                    self.timeline = vk_device
                        .create_semaphore(&info, None)
                        .map_err(|e| Error::native(format!("Failed to create a timeline semaphore: {e}")))?;
                }
                SyncKind::SyncFd => {
                    let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
                    let info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
                    self.export_semaphore = vk_device
                        .create_semaphore(&info, None)
                        .map_err(|e| Error::native(format!("Failed to create an exportable semaphore: {e}")))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Has the next frame draw into the image at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.selected = slot;
    }

    /// The host gave the image at `slot` back: the next frame drawn into it
    /// waits for `consumer`, a sync of one of
    /// [`Device::sync_kinds`](crate::Device::sync_kinds)'s consumer kinds. The sync file of the
    /// slot's last frame is closed.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when the target
    /// can't wait for `consumer`, such as for a sync file Vulkan can't
    /// import. The host's GPU may then still read the image.
    #[cfg_attr(not(unix), expect(clippy::needless_pass_by_value, reason = "a Unix release takes the host's sync file"))]
    pub fn release(&mut self, gpu: &mut VulkanGpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        use ash::vk::Handle as _;
        let Some(state) = self.slots.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        #[cfg(unix)]
        {
            state.producer_fd = None;
        }
        forget_wait(gpu, &self.backend, state);
        #[cfg(unix)]
        let ConsumerSync { sync: consumer, sync_file } = consumer;
        #[cfg(not(unix))]
        let ConsumerSync { sync: consumer } = consumer;
        match consumer.kind {
            SyncKind::CpuComplete => Ok(()),
            SyncKind::QueueOrder => {
                // The host's reads went to the queue before the next frame,
                // which waits for them before it clears the image.
                gpu.wait_for_queue()
                    .map_err(|error| Error::native(format!("Waiting for the host's reads failed: {error}")))
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
                let backend = &self.backend;
                let loader = backend
                    .get_semaphore_fd_loader()
                    .ok_or_else(|| Error::native("the Vulkan device imports no sync files"))?;
                let device = backend.get_device();
                let file =
                    fd.try_clone().map_err(|e| Error::native(format!("Failed to keep the host's sync file: {e}")))?;
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
                        state.consumer_wait = Some(ConsumerWait::Imported { semaphore, file });
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
            #[cfg(windows)]
            SyncKind::D3d12Fence => {
                let fence = self.imported_fence(consumer.handle)?;
                self.slots[slot].consumer_wait = Some(ConsumerWait::Fence(fence, consumer.value));
                Ok(())
            }
            kind => Err(Error::unsupported(format!("the Vulkan target waits for no {kind:?}"))),
        }
    }

    /// The host's D3D12 fence whose NT handle is `handle`, imported once.
    #[cfg(windows)]
    fn imported_fence(&mut self, handle: u64) -> Result<Rc<ImportedFence>, Error> {
        if let Some((_, fence)) = self.imported_fences.iter().find(|(known, _)| *known == handle) {
            return Ok(Rc::clone(fence));
        }
        let fence = Rc::new(ImportedFence::import(&self.backend, handle)?);
        self.imported_fences.push((handle, Rc::clone(&fence)));
        Ok(fence)
    }

    /// The host gave up the image at `slot` without saying when its reads
    /// end: the slot's sync file is closed, and the image is never drawn
    /// into again.
    pub fn discard(&mut self, gpu: &mut VulkanGpu, slot: usize) {
        if let Some(state) = self.slots.get_mut(slot) {
            #[cfg(unix)]
            {
                state.producer_fd = None;
            }
            forget_wait(gpu, &self.backend, state);
        }
    }

    /// Takes what the ring's images still wait for, the host's reads of
    /// them, for the ring to wait for before it lets go of the images.
    pub fn take_pending(&mut self, gpu: &mut VulkanGpu) -> PendingReads {
        for slot in &mut self.slots {
            detach(gpu, slot);
        }
        let waits = self.slots.iter_mut().filter_map(|slot| slot.consumer_wait.take()).collect();
        PendingReads { backend: Rc::clone(&self.backend), waits }
    }

    /// Draws into `images` from now on, which must have as many images, and
    /// the format, final layout and owner of the ring's.
    ///
    /// Fails as [`BorrowedImages::check`] does, with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for another
    /// number of images, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another format,
    /// final layout or owner, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    /// The target then keeps its images. Returns the host's reads of the
    /// images that the ring let go of, which the ring waits for before the
    /// host may use them.
    pub fn replace(&mut self, gpu: &mut VulkanGpu, images: BorrowedImages) -> Result<PendingReads, Error> {
        {
            let current = &self.images;
            if images.images.len() != current.images.len() {
                return Err(Error::invalid_argument("a ring keeps its depth"));
            }
            if (images.format, images.final_layout, images.owner)
                != (current.format, current.final_layout, current.owner)
            {
                return Err(Error::unsupported("a ring keeps its format, final layout and owner"));
            }
            let depth = |images: &BorrowedImages| images.depth.as_ref().map(|depth| (depth.format, depth.final_layout));
            if depth(&images) != depth(current) {
                return Err(Error::unsupported("a ring keeps its depth images' format and final layout"));
            }
            images.check(self.backend.device())?;
        }
        let imports = Imports::new(gpu, &images)?;
        // The frames that drew into the old images are done before the
        // `Gpu` forgets them.
        self.backend.wait_idle("borrowed image replacement");
        std::mem::replace(&mut self.imports, imports).tear_down(gpu);
        let old = std::mem::replace(&mut self.images, images);
        Ok(self.reset_slots(&old.images))
    }

    /// Gives each image that the ring had in `old` what it waited for, at
    /// its new slot, and forgets what the slots handed over. Returns what
    /// the images that the ring let go of wait for.
    fn reset_slots(&mut self, old: &[BorrowedImage]) -> PendingReads {
        let slots = &mut self.slots;
        let mut previous = std::mem::take(slots);
        for image in &self.images.images {
            let carried = old
                .iter()
                .position(|old| old.image == image.image)
                .and_then(|index| previous.get_mut(index))
                .and_then(|slot| slot.consumer_wait.take());
            slots.push(Slot { consumer_wait: carried, ..Slot::default() });
        }
        let waits = previous.iter_mut().filter_map(|slot| slot.consumer_wait.take()).collect();
        PendingReads { backend: Rc::clone(&self.backend), waits }
    }

    /// The sync the host waits for, once the frame was submitted.
    #[cfg_attr(not(unix), expect(unused_variables, reason = "sync files are Unix's"))]
    fn producer_sync(&mut self, gpu: &mut VulkanGpu, slot: usize) -> Option<GpuSync> {
        use ash::vk::Handle as _;
        match self.producer {
            SyncKind::CpuComplete => {
                if let Err(error) = gpu.flush(true) {
                    Log::error(Event::Render, format!("Waiting for a borrowed image's frame failed: {error}"));
                    return None;
                }
                Some(GpuSync::of(SyncKind::CpuComplete))
            }
            SyncKind::QueueOrder => Some(GpuSync::of(SyncKind::QueueOrder)),
            SyncKind::VulkanTimelineSemaphore => Some(GpuSync {
                kind: SyncKind::VulkanTimelineSemaphore,
                handle: self.timeline.as_raw(),
                value: self.timeline_value,
            }),
            #[cfg(unix)]
            SyncKind::SyncFd => {
                use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
                let loader = self.backend.get_semaphore_fd_loader()?;
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
                // A semaphore that signaled already exports no descriptor:
                // the frame is complete.
                if fd == -1 {
                    return Some(GpuSync::of(SyncKind::CpuComplete));
                }
                // SAFETY: the export made the descriptor, which is the
                // target's.
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                let raw = fd.as_raw_fd();
                if let Some(state) = self.slots.get_mut(slot) {
                    state.producer_fd = Some(fd);
                }
                Some(GpuSync { kind: SyncKind::SyncFd, handle: raw as u64, value: 0 })
            }
            #[cfg(windows)]
            SyncKind::D3d12Fence => self.shared_fence.as_ref().map(|handle| GpuSync {
                kind: SyncKind::D3d12Fence,
                handle: handle.as_raw() as u64,
                value: self.timeline_value,
            }),
            _ => None,
        }
    }
}

/// Drops what `slot` waits for, destroying a semaphore of the target's that
/// no frame waits on.
fn forget_wait(gpu: &mut VulkanGpu, backend: &BackendImpl, slot: &mut Slot) {
    detach(gpu, slot);
    if let Some(wait) = slot.consumer_wait.take() {
        drop_wait(backend, wait);
    }
}

/// Has no submit of `gpu` wait for what `slot` waits for.
fn detach(gpu: &mut VulkanGpu, slot: &mut Slot) {
    if let Some(texture) = slot.attached.take() {
        gpu.forget_waits(texture);
    }
}

impl FrameTarget for BorrowedTarget {
    type Gpu = VulkanGpu;

    /// The selected slot's image, which the frame's first pass that draws
    /// into it waits for the host's reads before.
    fn begin_frame(&mut self, gpu: &mut VulkanGpu) -> Option<FrameImage> {
        let slot = self.selected;
        let image = self.imports.frame_image(slot)?;
        let stages = vk::PipelineStageFlags::ALL_COMMANDS;
        let Some(state) = self.slots.get_mut(slot) else {
            return Some(image);
        };
        // A frame begun again keeps the wait it attached.
        if state.attached == Some(image.color) {
            return Some(image);
        }
        let waited = match &state.consumer_wait {
            Some(ConsumerWait::Timeline(semaphore, value)) => {
                gpu.wait_before_using(image.color, *semaphore, Some(*value), stages)
            }
            #[cfg(unix)]
            Some(ConsumerWait::Imported { semaphore, .. }) => {
                gpu.wait_before_using(image.color, *semaphore, None, stages)
            }
            #[cfg(windows)]
            Some(ConsumerWait::Fence(fence, value)) => {
                gpu.wait_before_using(image.color, fence.semaphore, Some(*value), stages)
            }
            None => return Some(image),
        };
        state.attached = Some(image.color);
        if let Err(error) = waited {
            Log::error(Event::Render, format!("Waiting for the host's reads failed: {error}"));
        }
        Some(image)
    }

    /// Submits what is left of the frame, signaling the producer's sync
    /// after it.
    fn end_frame(&mut self, gpu: &mut VulkanGpu) -> Handoff {
        let slot = self.selected;
        let Some(color) = self.imports.color.get(slot).copied() else {
            return Handoff::Dropped;
        };
        // What no pass took yet, such as where the frame drew nothing.
        gpu.take_waits_into_next_submit(color);
        match self.producer {
            SyncKind::VulkanTimelineSemaphore | SyncKind::D3d12Fence => {
                self.timeline_value += 1;
                gpu.signal_after_next_submit(self.timeline, Some(self.timeline_value));
            }
            SyncKind::SyncFd => gpu.signal_after_next_submit(self.export_semaphore, None),
            _ => {}
        }
        let submitted = gpu.flush(false);
        // The frame's submits took what the slot waited for.
        let consumed = self.slots.get_mut(slot).and_then(|state| {
            let attached = state.attached.take()?;
            (attached == color).then(|| state.consumer_wait.take()).flatten()
        });
        // A timeline is the host's, and the target keeps a fence until it
        // waits for the queue.
        #[cfg(unix)]
        if let Some(ConsumerWait::Imported { semaphore, file }) = consumed {
            drop(file);
            // SAFETY: destroyed once the frame that waits on it is done.
            gpu.destroy_later(move |device| unsafe { device.destroy_semaphore(semaphore, None) });
        }
        #[cfg(not(unix))]
        drop(consumed);
        if let Err(error) = submitted {
            Log::error(Event::Render, format!("Submitting a borrowed image's frame failed: {error}"));
            return Handoff::Dropped;
        }
        match self.producer_sync(gpu, slot) {
            Some(sync) => Handoff::GivenBack { slot, sync },
            None => Handoff::Dropped,
        }
    }

    /// Forgets what the slots wait for, and destroys the imports.
    fn tear_down(&mut self, gpu: &mut VulkanGpu) {
        self.backend.wait_idle("borrowed target deletion");
        for mut slot in std::mem::take(&mut self.slots) {
            forget_wait(gpu, &self.backend, &mut slot);
        }
        self.imports.tear_down(gpu);
        self.torn_down = true;
    }
}

impl Drop for BorrowedTarget {
    /// Destroys the target's semaphores, and what the slots wait for that a
    /// tear-down didn't, after the backend's work.
    fn drop(&mut self) {
        let backend = &*self.backend;
        if !self.torn_down {
            backend.wait_idle("borrowed target deletion");
        }
        let device = backend.get_device();
        for slot in std::mem::take(&mut self.slots) {
            if let Some(wait) = slot.consumer_wait {
                drop_wait(backend, wait);
            }
        }
        // SAFETY: the backend's work is done, and the semaphores are the
        // target's.
        unsafe {
            if self.timeline != vk::Semaphore::null() {
                device.destroy_semaphore(self.timeline, None);
            }
            if self.export_semaphore != vk::Semaphore::null() {
                device.destroy_semaphore(self.export_semaphore, None);
            }
        }
        #[cfg(windows)]
        self.imported_fences.clear();
    }
}

/// The host's reads of images that a ring let go of, which the ring waits
/// for before the host may use or destroy the images. Dropping it stops
/// waiting.
pub struct PendingReads {
    backend: Rc<BackendImpl>,
    waits: Vec<ConsumerWait>,
}

impl PendingReads {
    /// Whether every read ended, which it checks without blocking. It lets
    /// go of the syncs of the reads that ended.
    pub fn poll(&mut self) -> bool {
        let backend = &*self.backend;
        let now = Instant::now();
        let (ended, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.waits).into_iter().partition(|wait| match wait {
                ConsumerWait::Timeline(semaphore, value) => wait_timeline(backend, *semaphore, *value, now),
                #[cfg(unix)]
                ConsumerWait::Imported { file, .. } => poll_until(file, now),
                #[cfg(windows)]
                ConsumerWait::Fence(fence, value) => wait_timeline(backend, fence.semaphore, *value, now),
            });
        for wait in ended {
            drop_wait(backend, wait);
        }
        self.waits = waiting;
        self.waits.is_empty()
    }
}

impl Drop for PendingReads {
    fn drop(&mut self) {
        for wait in std::mem::take(&mut self.waits) {
            drop_wait(&self.backend, wait);
        }
    }
}

/// Lets go of `wait`, destroying a semaphore of the target's that no frame
/// waits on.
#[cfg_attr(not(unix), expect(unused_variables, clippy::needless_pass_by_value, reason = "sync files are Unix's"))]
fn drop_wait(backend: &BackendImpl, wait: ConsumerWait) {
    match wait {
        ConsumerWait::Timeline(..) => {}
        #[cfg(unix)]
        ConsumerWait::Imported { semaphore, file } => {
            drop(file);
            // SAFETY: no submitted frame waits on the semaphore, which the
            // target made.
            unsafe { backend.get_device().destroy_semaphore(semaphore, None) };
        }
        // The last reference destroys the fence's semaphore.
        #[cfg(windows)]
        ConsumerWait::Fence(..) => {}
    }
}

/// Waits until `deadline` for the host's `semaphore` to reach `value`, and
/// returns whether it did.
fn wait_timeline(backend: &BackendImpl, semaphore: vk::Semaphore, value: u64, deadline: Instant) -> bool {
    let raw = backend.device().raw();
    // Vulkan 1.2's, or VK_KHR_timeline_semaphore's, which the device has when
    // it takes timelines.
    // SAFETY: the device is live, and the names are functions' names.
    let Some(function) = [c"vkWaitSemaphores", c"vkWaitSemaphoresKHR"]
        .into_iter()
        .find_map(|name| unsafe { (raw.get_device_proc_addr)(raw.device, name.as_ptr()) })
    else {
        return false;
    };
    // SAFETY: both functions have vkWaitSemaphores's signature.
    let wait = unsafe { std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkWaitSemaphores>(function) };
    let semaphores = [semaphore];
    let values = [value];
    let info = vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values);
    let timeout = u64::try_from(deadline.saturating_duration_since(Instant::now()).as_nanos()).unwrap_or(u64::MAX);
    // SAFETY: the host's timeline, of the device, which stays valid until
    // the session ends.
    unsafe { wait(raw.device, &raw const info, timeout) == vk::Result::SUCCESS }
}

/// Waits until `deadline` for `file`, a sync file, to signal, and returns
/// whether it did.
#[cfg(unix)]
fn poll_until(file: &std::os::fd::OwnedFd, deadline: Instant) -> bool {
    use std::os::fd::AsRawFd as _;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // Rounded up, so that a wait shorter than a millisecond still waits.
        let timeout = i32::try_from(remaining.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
        let mut poll = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one live descriptor, which the call only reads.
        match unsafe { libc::poll(&raw mut poll, 1, timeout) } {
            0 => return false,
            ready if ready > 0 => return true,
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
            _ => return false,
        }
    }
}
