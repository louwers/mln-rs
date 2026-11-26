// A ring of the host's images that frames are drawn into and given back to
// the host, and the ring target's Vulkan part (mln_gfx::ring): the semaphores
// that the host waits on before it reads an image, and the host's syncs that
// the target waits for before it draws into an image the host read.

use ash::vk;
use mln_gfx::gpu::{GpuError, TargetFormats, TextureFormat, TextureId, TextureUsage};
use mln_gfx::ring::{self, RingBackend, RingTextures};
use mln_gfx::{ConsumerSync, GpuSync, SyncKind};
use mln_util::error::Error;
use mln_util::size::Size;

use crate::device::Device;
use crate::gpu::{ImportedImage, VulkanGpu};
#[cfg(unix)]
use crate::log_target;
use crate::renderer_backend::RendererBackend;

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

impl RingTextures for BorrowedImages {
    type Image = BorrowedImage;

    fn images(&self) -> &[BorrowedImage] {
        &self.images
    }

    fn same_image(a: &BorrowedImage, b: &BorrowedImage) -> bool {
        a.image == b.image
    }

    fn size(&self) -> Size {
        self.size
    }

    fn has_depth(&self) -> bool {
        self.depth.is_some()
    }
}

/// What the next frame drawn into a slot's image waits for: the host's reads
/// of it, on `device`.
pub struct ConsumerWait {
    device: Device,
    waiting: Waiting,
}

/// What a [`ConsumerWait`] waits for.
enum Waiting {
    /// The host's timeline, until it reaches the value.
    Timeline(vk::Semaphore, u64),
    /// A semaphore of the target's own that holds an imported sync file.
    #[cfg(unix)]
    Imported(vk::Semaphore),
    /// A timeline semaphore of the target's own that holds the host's
    /// D3D12 fence, imported, until it reaches the value.
    #[cfg(windows)]
    Fence(vk::Semaphore, u64),
}

impl Waiting {
    /// Destroys the semaphore of the ring's own that the wait holds, with
    /// `device`. A timeline is the host's.
    ///
    /// # Safety
    ///
    /// No submit that hasn't finished waits on it.
    unsafe fn destroy(self, device: &ash::Device) {
        // SAFETY: as the caller says.
        unsafe {
            match self {
                Waiting::Timeline(..) => {}
                #[cfg(unix)]
                Waiting::Imported(semaphore) => device.destroy_semaphore(semaphore, None),
                #[cfg(windows)]
                Waiting::Fence(semaphore, _) => device.destroy_semaphore(semaphore, None),
            }
        }
    }
}

/// The host's D3D12 fence whose NT handle is `handle`, imported into a new
/// timeline semaphore, which the caller destroys. The handle stays the
/// host's.
///
/// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when Vulkan can't
/// import it.
#[cfg(windows)]
fn import_fence(device: &Device, handle: u64) -> Result<vk::Semaphore, Error> {
    let loader =
        device.shared.semaphore_win32().ok_or_else(|| Error::native("the Vulkan device imports no D3D12 fences"))?;
    let vk_device = device.shared.device();
    let mut type_info = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    let info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
    // SAFETY: the create info is valid for the device, which has timelines
    // when it imports D3D12 fences.
    let semaphore = unsafe { vk_device.create_semaphore(&info, None) }
        .map_err(|e| Error::native(format!("Failed to create a timeline semaphore: {e}")))?;
    let import = vk::ImportSemaphoreWin32HandleInfoKHR::default()
        .semaphore(semaphore)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE)
        .handle(handle as vk::HANDLE);
    // SAFETY: the semaphore is new, and an import of an NT handle leaves the
    // handle to its owner.
    match unsafe { loader.import_semaphore_win32_handle(&import) } {
        Ok(()) => Ok(semaphore),
        Err(error) => {
            // SAFETY: nothing uses the semaphore.
            unsafe { vk_device.destroy_semaphore(semaphore, None) };
            Err(Error::native(format!("Failed to import the host's D3D12 fence: {error}")))
        }
    }
}

/// Frames drawn into the host's images, each given back to the host.
pub type BorrowedTarget = ring::BorrowedTarget<VulkanRing>;

/// A [`BorrowedTarget`]'s part on Vulkan: the semaphores that its frames
/// signal. A tear-down destroys them once the frames are done, and dropping
/// it without one once the target waited for them.
pub struct VulkanRing {
    device: Device,
    producer: SyncKind,
    /// The timeline that frames signal, for a timeline or D3D12 fence
    /// producer.
    timeline: vk::Semaphore,
    timeline_value: u64,
    /// The binary semaphore that frames signal, for a sync file producer.
    export_semaphore: vk::Semaphore,
    /// The NT handle that shares `timeline` as a D3D12 fence, for a D3D12
    /// fence producer.
    #[cfg(windows)]
    shared_fence: Option<std::os::windows::io::OwnedHandle>,
}

impl VulkanRing {
    /// Makes what frames signal for the target's producer kind, which the
    /// device has.
    fn create_sync(&mut self) -> Result<(), Error> {
        let producer = self.producer;
        let vk_device = self.device.shared.device().clone();
        // SAFETY: the create infos are valid for the device, which has the
        // features of the producer kind (checked above).
        unsafe {
            match producer {
                #[cfg(windows)]
                SyncKind::D3d12Fence => {
                    use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
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
                        .device
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
                    self.shared_fence = Some(OwnedHandle::from_raw_handle(handle as _));
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

    /// Imports the host's sync file `fd` into a semaphore of the target's
    /// own.
    #[cfg(unix)]
    fn import_sync_file(&self, fd: std::os::fd::OwnedFd) -> Result<Waiting, Error> {
        use std::os::fd::{FromRawFd as _, IntoRawFd as _, OwnedFd};
        let loader = self
            .device
            .shared
            .semaphore_fd()
            .ok_or_else(|| Error::native("the Vulkan device imports no sync files"))?;
        let device = self.device.shared.device();
        // SAFETY: the create info is valid for the device.
        let semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
            .map_err(|e| Error::native(format!("Failed to create a semaphore: {e}")))?;
        let raw = fd.into_raw_fd();
        let info = vk::ImportSemaphoreFdInfoKHR::default()
            .semaphore(semaphore)
            .flags(vk::SemaphoreImportFlags::TEMPORARY)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
            .fd(raw);
        // SAFETY: the semaphore is new; Vulkan owns the descriptor once the
        // import succeeds.
        match unsafe { loader.import_semaphore_fd(&info) } {
            Ok(()) => Ok(Waiting::Imported(semaphore)),
            Err(error) => {
                // SAFETY: the import failed, so the descriptor is still the
                // target's, and nothing uses the semaphore.
                unsafe {
                    drop(OwnedFd::from_raw_fd(raw));
                    device.destroy_semaphore(semaphore, None);
                }
                Err(Error::native(format!("Failed to import the host's sync file: {error}")))
            }
        }
    }

    /// Exports the sync file that the submitted frame signals.
    #[cfg(unix)]
    fn export_sync_file(&self) -> Option<(GpuSync, Option<std::os::fd::OwnedFd>)> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
        let loader = self.device.shared.semaphore_fd()?;
        let info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(self.export_semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        // SAFETY: the submitted frame signals the semaphore, which the
        // export unsignals for the next frame.
        let fd = match unsafe { loader.get_semaphore_fd(&info) } {
            Ok(fd) => fd,
            Err(error) => {
                log::error!(target: log_target::RENDER, "Exporting a frame's sync file failed: {error}");
                return None;
            }
        };
        // A semaphore that signaled already exports no descriptor: the frame
        // is complete.
        if fd == -1 {
            return Some((GpuSync::of(SyncKind::CpuComplete), None));
        }
        // SAFETY: the export made the descriptor, which is the target's.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let sync = GpuSync { kind: SyncKind::SyncFd, handle: fd.as_raw_fd() as u64, value: 0 };
        Some((sync, Some(fd)))
    }
}

impl Drop for VulkanRing {
    /// Destroys the semaphores that no tear-down took, which the target's
    /// frames are done with.
    fn drop(&mut self) {
        let device = self.device.shared.device();
        // SAFETY: the target waited for its frames, and the semaphores are
        // the ring's.
        unsafe {
            if self.timeline != vk::Semaphore::null() {
                device.destroy_semaphore(self.timeline, None);
            }
            if self.export_semaphore != vk::Semaphore::null() {
                device.destroy_semaphore(self.export_semaphore, None);
            }
        }
    }
}

/// What a slot's last frame handed the host: the frame's sync file, which
/// the host may read until it gives the slot back.
#[cfg(unix)]
type Handed = std::os::fd::OwnedFd;
#[cfg(not(unix))]
type Handed = std::convert::Infallible;

impl RingBackend for VulkanRing {
    type Gpu = VulkanGpu;
    type Renderer = RendererBackend;
    type Textures = BorrowedImages;
    type Wait = ConsumerWait;
    type Handed = Handed;

    /// Fails as [`BorrowedImages::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// that isn't one of [`Device::sync_kinds`](crate::Device::sync_kinds)'s,
    /// and with [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan
    /// call fails.
    fn new(backend: &mut RendererBackend, images: &BorrowedImages, producer: SyncKind) -> Result<Self, Error> {
        let device = backend.device().clone();
        images.check(&device)?;
        if !device.sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("the Vulkan device can't hand frames over with {producer:?}")));
        }
        let mut ring = VulkanRing {
            device,
            producer,
            timeline: vk::Semaphore::null(),
            timeline_value: 0,
            export_semaphore: vk::Semaphore::null(),
            #[cfg(windows)]
            shared_fence: None,
        };
        ring.create_sync()?;
        Ok(ring)
    }

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another format, final layout or owner, or depth images of another
    /// format or final layout or where the ring has none, and as
    /// [`BorrowedImages::check`] does.
    fn check_replacement(&self, current: &BorrowedImages, next: &BorrowedImages) -> Result<(), Error> {
        if (next.format, next.final_layout, next.owner) != (current.format, current.final_layout, current.owner) {
            return Err(Error::unsupported("a ring keeps its format, final layout and owner"));
        }
        let depth = |images: &BorrowedImages| images.depth.as_ref().map(|depth| (depth.format, depth.final_layout));
        if depth(next) != depth(current) {
            return Err(Error::unsupported("a ring keeps its depth images' format and final layout"));
        }
        next.check(&self.device)
    }

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// format that `VulkanGpu` has no texture of.
    fn targets(gpu: &VulkanGpu, images: &BorrowedImages) -> Result<TargetFormats, Error> {
        let color = gpu
            .texture_format(images.format)
            .ok_or_else(|| Error::unsupported(format!("the Vulkan target draws into no {:?}", images.format)))?;
        let depth_stencil = match &images.depth {
            Some(depth) => gpu.texture_format(depth.format),
            None => Some(TextureFormat::Depth24PlusStencil8),
        };
        Ok(TargetFormats { color: Some(color), depth_stencil })
    }

    fn import(gpu: &mut VulkanGpu, images: &BorrowedImages, slot: usize, depth: bool) -> Result<TextureId, GpuError> {
        let release_to = match images.owner {
            ImageOwner::GraphicsQueue => None,
            ImageOwner::External => Some(vk::QUEUE_FAMILY_EXTERNAL),
            ImageOwner::Foreign => Some(vk::QUEUE_FAMILY_FOREIGN_EXT),
        };
        let (ring, format, layout) = match (&images.depth, depth) {
            (Some(depth), true) => (&depth.images, depth.format, depth.final_layout),
            (None, true) => return Err(GpuError::Invalid("the ring has no depth images".to_owned())),
            (_, false) => (&images.images, images.format, images.final_layout),
        };
        let image = ring.get(slot).ok_or_else(|| GpuError::Invalid(format!("the ring has no slot {slot}")))?;
        gpu.import_image(&ImportedImage {
            image: image.image,
            view: image.view,
            format,
            size: images.size,
            usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
            layout,
            release_to,
        })
    }

    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when the target
    /// can't wait for `consumer`, such as for a sync file Vulkan can't
    /// import, and with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind than [`Device::sync_kinds`](crate::Device::sync_kinds)'s
    /// consumer kinds. The host's GPU may then still read the image.
    fn wait_for(&mut self, gpu: &mut VulkanGpu, consumer: ConsumerSync) -> Result<Option<ConsumerWait>, Error> {
        use ash::vk::Handle as _;
        #[cfg(unix)]
        let ConsumerSync { sync: consumer, sync_file } = consumer;
        #[cfg(not(unix))]
        let ConsumerSync { sync: consumer } = consumer;
        let waiting = match consumer.kind {
            SyncKind::CpuComplete => return Ok(None),
            SyncKind::QueueOrder => {
                // The host's reads went to the queue before the next frame,
                // which waits for them before it clears the image.
                gpu.wait_for_queue()
                    .map_err(|error| Error::native(format!("Waiting for the host's reads failed: {error}")))?;
                return Ok(None);
            }
            SyncKind::VulkanTimelineSemaphore => {
                Waiting::Timeline(vk::Semaphore::from_raw(consumer.handle), consumer.value)
            }
            #[cfg(unix)]
            SyncKind::SyncFd => {
                let fd = sync_file.ok_or_else(|| Error::invalid_argument("a sync file release names no file"))?;
                self.import_sync_file(fd)?
            }
            #[cfg(windows)]
            SyncKind::D3d12Fence => Waiting::Fence(import_fence(&self.device, consumer.handle)?, consumer.value),
            kind => return Err(Error::unsupported(format!("the Vulkan target waits for no {kind:?}"))),
        };
        Ok(Some(ConsumerWait { device: self.device.clone(), waiting }))
    }

    fn wait_before_using(gpu: &mut VulkanGpu, texture: TextureId, wait: &ConsumerWait) -> Result<(), GpuError> {
        let stages = vk::PipelineStageFlags::ALL_COMMANDS;
        match &wait.waiting {
            Waiting::Timeline(semaphore, value) => gpu.wait_before_using(texture, *semaphore, Some(*value), stages),
            #[cfg(unix)]
            Waiting::Imported(semaphore) => gpu.wait_before_using(texture, *semaphore, None, stages),
            #[cfg(windows)]
            Waiting::Fence(semaphore, value) => gpu.wait_before_using(texture, *semaphore, Some(*value), stages),
        }
    }

    fn take_waits_into_next_submit(gpu: &mut VulkanGpu, texture: TextureId) {
        gpu.take_waits_into_next_submit(texture);
    }

    fn forget_waits(gpu: &mut VulkanGpu, texture: TextureId) -> bool {
        gpu.forget_waits(texture)
    }

    /// A semaphore of the ring's own goes once the submit is done.
    fn waited(gpu: &mut VulkanGpu, wait: ConsumerWait) {
        let waiting = wait.waiting;
        // SAFETY: destroyed once the frame that waits on it is done.
        gpu.destroy_later(move |device| unsafe { waiting.destroy(device) });
    }

    fn signal_after_next_submit(&mut self, gpu: &mut VulkanGpu) {
        match self.producer {
            SyncKind::VulkanTimelineSemaphore | SyncKind::D3d12Fence => {
                self.timeline_value += 1;
                gpu.signal_after_next_submit(self.timeline, Some(self.timeline_value));
            }
            SyncKind::SyncFd => gpu.signal_after_next_submit(self.export_semaphore, None),
            _ => {}
        }
    }

    fn producer_sync(&mut self, _gpu: &mut VulkanGpu) -> Option<(GpuSync, Option<Handed>)> {
        use ash::vk::Handle as _;
        match self.producer {
            SyncKind::VulkanTimelineSemaphore => Some((
                GpuSync {
                    kind: SyncKind::VulkanTimelineSemaphore,
                    handle: self.timeline.as_raw(),
                    value: self.timeline_value,
                },
                None,
            )),
            #[cfg(unix)]
            SyncKind::SyncFd => self.export_sync_file(),
            #[cfg(windows)]
            SyncKind::D3d12Fence => self.shared_fence.as_ref().map(|handle| {
                use std::os::windows::io::AsRawHandle as _;
                let handle = handle.as_raw_handle() as usize as u64;
                (GpuSync { kind: SyncKind::D3d12Fence, handle, value: self.timeline_value }, None)
            }),
            _ => None,
        }
    }

    /// The semaphores go once the frames that signal them are done.
    fn tear_down(&mut self, gpu: &mut VulkanGpu) {
        let semaphores = [
            std::mem::replace(&mut self.timeline, vk::Semaphore::null()),
            std::mem::replace(&mut self.export_semaphore, vk::Semaphore::null()),
        ];
        // SAFETY: destroyed once the frames that signal them are done, and
        // they are the ring's.
        gpu.destroy_later(move |device| unsafe {
            for semaphore in semaphores.into_iter().filter(|semaphore| *semaphore != vk::Semaphore::null()) {
                device.destroy_semaphore(semaphore, None);
            }
        });
    }

    /// Waits for the work submitted to the device's queue.
    fn wait_for_frames(&self, what: &str) {
        self.device.wait_idle(what);
    }

    fn forget(wait: ConsumerWait) {
        // SAFETY: no submit waits on the semaphore, which the ring made.
        unsafe { wait.waiting.destroy(wait.device.shared.device()) };
    }
}
