// What a `SurfaceTarget` presents to: a swapchain of a platform surface,
// whose images are textures of the backend's `VulkanGpu`, and a depth and
// stencil texture of the `Gpu`'s.
//
// - The swapchain is made with the identity transform, and acquiring an
//   image waits for one.
// - A swapchain's images can be copied from when the surface allows it, so
//   a read needs no new swapchain; when it doesn't, the read is dropped
//   with a warning.
// - No swapchain is made for an empty extent, which a minimized window can
//   have: the resource then has no images until it is made again.
//   `current_extent` says whether the surface has one now.
// - A failed wait for a frame, or a surface that stays out of date, is
//   logged, and the frame is not drawn.
// - The overlay (`set_overlay`) draws over each frame in a render pass of
//   its own, after the frame's passes and before the frame is read back and
//   presented.

use ash::vk;
use mln_gfx::frame::FrameImage;
use mln_gfx::gpu::{
    Gpu as _, ReadbackId, ResourceId, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion,
    TextureUsage, destroy_unused,
};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::device::Device;
use crate::gpu::{ImportedImage, VulkanGpu};
use crate::renderer_backend::SurfaceProvider;
use crate::surface_target::{Overlay, OverlayTarget};

const MAXIMUM_SURFACE_UPDATE_RETRIES: u8 = 3;

/// The format of the depth and stencil attachment.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The extent that `surface`, a surface of `device`'s instance,
/// has now, which is empty for a minimized window's. `None` when the
/// swapchain sets the surface's extent, as on Wayland, or the surface can't
/// say, such as once it is lost.
pub(crate) fn surface_extent(device: &Device, surface: vk::SurfaceKHR) -> Option<vk::Extent2D> {
    let loader = device.shared.surface()?;
    // SAFETY: the surface is the instance's.
    let capabilities =
        unsafe { loader.get_physical_device_surface_capabilities(device.shared.physical_device(), surface) }.ok()?;
    let extent = capabilities.current_extent;
    (extent.width != u32::MAX || extent.height != u32::MAX).then_some(extent)
}

/// A swapchain image, its view, and the texture of the `Gpu` it is.
struct SwapchainImage {
    view: vk::ImageView,
    texture: TextureId,
    /// Signaled by the frame drawn into the image, and waited for by its
    /// presentation.
    present_semaphore: vk::Semaphore,
}

/// A frame whose image is acquired.
#[derive(Clone, Copy)]
struct Acquired {
    image: u32,
    slot: usize,
}

struct SurfaceState {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    present_mode: vk::PresentModeKHR,
    extent: vk::Extent2D,
    color_format: vk::Format,
    /// Whether the images can be copied from (`TRANSFER_SRC`).
    readable: bool,
    images: Vec<SwapchainImage>,
    depth_stencil: Option<TextureId>,

    /// The semaphore that each of the frames in flight acquires its image
    /// with, and the serial of the last submit of the frame that last used
    /// it, which has waited for it once finished.
    acquire_semaphores: Vec<(vk::Semaphore, u64)>,
    /// The frame in flight that acquires next.
    next_slot: usize,
    acquired: Option<Acquired>,

    update_requested: bool,
    update_latency: i32,
    /// The platform surface must be made again before the swapchain is.
    lost: bool,
}

/// What a frame target presents to: a swapchain of a platform surface, its
/// images and a depth and stencil attachment, all textures of the backend's
/// `Gpu`, which each call that reaches them takes.
pub(crate) struct SurfaceResource {
    device: Device,
    /// The backend's frames in flight, each of which acquires an image with
    /// a semaphore of its own.
    max_frames: u32,
    provider: Option<Box<dyn SurfaceProvider>>,
    /// Whether the resource destroys its surface: one that `provider` made,
    /// and not a host's.
    owns_surface: bool,
    state: SurfaceState,
    overlay: Option<Box<dyn Overlay>>,
    /// Whether the next frame presented is read back.
    read_requested: bool,
    /// The readback of the last frame read, and its size.
    readback: Option<(ReadbackId, Size)>,
    /// Whether a tear-down destroyed the resource's objects, after the
    /// device's work.
    torn_down: bool,
}

impl SurfaceResource {
    /// A resource of `size` on `surface`, whose textures are of `gpu`.
    /// `provider` made the surface, which the resource then destroys, and
    /// makes it again when it is lost; without one, the surface is the
    /// host's. `present_mode` is used when the surface has it, FIFO
    /// otherwise.
    pub(crate) fn new(
        gpu: &mut VulkanGpu,
        size: Size,
        surface: vk::SurfaceKHR,
        provider: Option<Box<dyn SurfaceProvider>>,
        present_mode: vk::PresentModeKHR,
    ) -> Self {
        let mut resource = SurfaceResource {
            device: gpu.device().clone(),
            max_frames: gpu.max_frames(),
            owns_surface: provider.is_some(),
            provider,
            state: SurfaceState {
                surface,
                swapchain: vk::SwapchainKHR::null(),
                present_mode,
                extent: vk::Extent2D::default(),
                color_format: vk::Format::UNDEFINED,
                readable: false,
                images: Vec::new(),
                depth_stencil: None,
                acquire_semaphores: Vec::new(),
                next_slot: 0,
                acquired: None,
                update_requested: false,
                update_latency: 0,
                lost: false,
            },
            overlay: None,
            read_requested: false,
            readback: None,
            torn_down: false,
        };
        resource.init(gpu, size.width, size.height);
        resource
    }

    /// The extent of the resource's surface now, or `None` when
    /// it has no surface or its surface doesn't say ([`surface_extent`]).
    pub(crate) fn current_extent(&self) -> Option<vk::Extent2D> {
        let surface = self.state.surface;
        if surface == vk::SurfaceKHR::null() {
            return None;
        }
        surface_extent(&self.device, surface)
    }

    /// Whether the resource presents to a surface: a lost one that couldn't
    /// be made again is gone.
    pub(crate) fn has_platform_surface(&self) -> bool {
        self.state.surface != vk::SurfaceKHR::null()
    }

    pub(crate) fn get_color_format(&self) -> vk::Format {
        self.state.color_format
    }

    pub(crate) fn get_image_count(&self) -> u32 {
        self.state.images.len() as u32
    }

    pub(crate) fn get_extent(&self) -> vk::Extent2D {
        self.state.extent
    }

    fn init_swapchain(device: &Device, state: &mut SurfaceState, w: u32, h: u32) -> Result<Vec<vk::Image>, String> {
        let physical_device = device.shared.physical_device();
        let surface_loader = device.shared.surface().ok_or("The instance has no surfaces")?;
        let swapchain_loader = device.shared.swapchain().ok_or("The device has no swapchains")?;
        let surface = state.surface;

        // SAFETY: the surface is the instance's, and the device can present
        // to it.
        let (formats, present_modes, capabilities) = unsafe {
            (
                surface_loader
                    .get_physical_device_surface_formats(physical_device, surface)
                    .map_err(|e| format!("Surface formats: {e}"))?,
                surface_loader
                    .get_physical_device_surface_present_modes(physical_device, surface)
                    .map_err(|e| format!("Surface present modes: {e}"))?,
                surface_loader
                    .get_physical_device_surface_capabilities(physical_device, surface)
                    .map_err(|e| format!("Surface capabilities: {e}"))?,
            )
        };
        let format = formats
            .iter()
            .find(|format| {
                (format.format == vk::Format::B8G8R8A8_UNORM || format.format == vk::Format::R8G8B8A8_UNORM)
                    && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .ok_or("No suitable swapchain format found")?;

        // Every surface has FIFO.
        if state.present_mode != vk::PresentModeKHR::FIFO && !present_modes.contains(&state.present_mode) {
            Log::error(
                Event::Render,
                format!("Requested PresentModeKHR not available ({})", state.present_mode.as_raw()),
            );
            state.present_mode = vk::PresentModeKHR::FIFO;
        }

        // Readbacks copy from the images where the surface allows it.
        let mut image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        state.readable = capabilities.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_SRC);
        if state.readable {
            image_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }

        // The surface's extent, or the requested size within its limits.
        let extent = if capabilities.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: w.max(capabilities.min_image_extent.width).min(capabilities.max_image_extent.width),
                height: h.max(capabilities.min_image_extent.height).min(capabilities.max_image_extent.height),
            }
        } else {
            capabilities.current_extent
        };
        if extent.width == 0 || extent.height == 0 {
            return Err("The surface is empty".to_owned());
        }

        let mut swapchain_image_count = capabilities.min_image_count + 1;
        // A maximum of 0 is unlimited.
        if capabilities.max_image_count > 0 {
            swapchain_image_count = swapchain_image_count.min(capabilities.max_image_count);
        }

        let composite_alpha = if capabilities.supported_composite_alpha.contains(vk::CompositeAlphaFlagsKHR::INHERIT) {
            vk::CompositeAlphaFlagsKHR::INHERIT
        } else {
            vk::CompositeAlphaFlagsKHR::OPAQUE
        };
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(swapchain_image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .present_mode(state.present_mode)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(image_usage)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .clipped(true)
            .composite_alpha(composite_alpha)
            // The device's one queue renders and presents.
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .old_swapchain(state.swapchain);

        // SAFETY: the create info is valid for the surface, and the old
        // swapchain's images aren't in use (the device is idle when a
        // swapchain is replaced).
        let images = unsafe {
            let swapchain = swapchain_loader
                .create_swapchain(&create_info, None)
                .map_err(|e| format!("Failed to create a swapchain: {e}"))?;
            if state.swapchain != vk::SwapchainKHR::null() {
                swapchain_loader.destroy_swapchain(state.swapchain, None);
            }
            state.swapchain = swapchain;
            swapchain_loader.get_swapchain_images(swapchain).map_err(|e| format!("Swapchain images: {e}"))?
        };
        state.color_format = format.format;
        state.extent = extent;
        Ok(images)
    }

    /// Makes the swapchain, its images and their textures in `gpu`, the
    /// depth and stencil attachment, and the semaphores.
    fn init(&mut self, gpu: &mut VulkanGpu, w: u32, h: u32) -> bool {
        match Self::init_swapchain(&self.device, &mut self.state, w, h)
            .and_then(|images| self.init_images(gpu, &images))
        {
            Ok(()) => true,
            Err(error) => {
                Log::error(Event::Render, error);
                self.destroy_images(Some(gpu));
                false
            }
        }
    }

    fn init_images(&mut self, gpu: &mut VulkanGpu, images: &[vk::Image]) -> Result<(), String> {
        let state = &mut self.state;
        let device = self.device.shared.device();
        let size = Size::new(state.extent.width, state.extent.height);
        for (index, &image) in images.iter().enumerate() {
            let create_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(state.color_format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            // SAFETY: the image is live and of the color format.
            let view = unsafe { device.create_image_view(&create_info, None) }
                .map_err(|e| format!("Vulkan color view creation failed: {e}"))?;
            self.device.set_debug_name(image, &format!("SwapchainImage_{index}"));
            self.device.set_debug_name(view, &format!("SwapchainImageView_{index}"));
            let imported = gpu.import_image(&ImportedImage {
                image,
                view,
                format: state.color_format,
                size,
                usage: TextureUsage { sampled: false, render_target: true, copy_src: state.readable },
                layout: vk::ImageLayout::PRESENT_SRC_KHR,
                release_to: None,
            });
            let texture = match imported {
                Ok(texture) => texture,
                Err(error) => {
                    // SAFETY: nothing uses the view yet.
                    unsafe { device.destroy_image_view(view, None) };
                    return Err(format!("Importing a swapchain image failed: {error}"));
                }
            };
            state.images.push(SwapchainImage { view, texture, present_semaphore: vk::Semaphore::null() });
            // SAFETY: the create info is valid for the device.
            let semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                .map_err(|e| format!("Failed to create a semaphore: {e}"))?;
            self.device.set_debug_name(semaphore, &format!("PresentSemaphore_{index}"));
            if let Some(image) = state.images.last_mut() {
                image.present_semaphore = semaphore;
            }
        }
        for index in 0..self.max_frames {
            // SAFETY: the create info is valid for the device.
            let semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                .map_err(|e| format!("Failed to create a semaphore: {e}"))?;
            self.device.set_debug_name(semaphore, &format!("AcquireSemaphore_{index}"));
            state.acquire_semaphores.push((semaphore, 0));
        }
        let depth_stencil = gpu
            .create_texture(&TextureDesc {
                label: "SwapchainDepthImage",
                size,
                format: DEPTH_STENCIL,
                usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
            })
            .map_err(|error| format!("Vulkan depth texture allocation failed: {error}"))?;
        state.depth_stencil = Some(depth_stencil);
        Ok(())
    }

    /// Destroys what `init` made, but the swapchain and the surface: the
    /// textures in `gpu`, or, without it, only the native objects, which
    /// leaves the textures to the `Gpu`. The device is idle.
    fn destroy_images(&mut self, mut gpu: Option<&mut VulkanGpu>) {
        let device = self.device.shared.device();
        let state = &mut self.state;
        let mut destroy = |resource| {
            if let Some(gpu) = gpu.as_deref_mut() {
                destroy_unused(gpu, [resource]);
            }
        };
        if let Some((readback, _)) = self.readback.take() {
            destroy(ResourceId::Readback(readback));
        }
        for image in state.images.drain(..) {
            destroy(ResourceId::Texture(image.texture));
            // SAFETY: the device is idle, and the view and the semaphore are
            // this resource's.
            unsafe {
                device.destroy_image_view(image.view, None);
                device.destroy_semaphore(image.present_semaphore, None);
            }
        }
        for (semaphore, _) in state.acquire_semaphores.drain(..) {
            // SAFETY: the device is idle, and the semaphore is this
            // resource's.
            unsafe { device.destroy_semaphore(semaphore, None) };
        }
        if let Some(depth_stencil) = state.depth_stencil.take() {
            destroy(ResourceId::Texture(depth_stencil));
        }
        state.next_slot = 0;
        state.acquired = None;
    }

    /// Destroys the swapchain and the surface, whose images are gone
    /// (`destroy_images`).
    fn destroy_surface(&mut self) {
        let shared = &self.device.shared;
        let state = &mut self.state;
        // SAFETY: the device is idle, and the swapchain and the surface are
        // this resource's.
        unsafe {
            if state.swapchain != vk::SwapchainKHR::null()
                && let Some(loader) = shared.swapchain()
            {
                loader.destroy_swapchain(state.swapchain, None);
            }
            if state.surface != vk::SurfaceKHR::null()
                && self.owns_surface
                && let Some(loader) = shared.surface()
            {
                loader.destroy_surface(state.surface, None);
            }
        }
        state.swapchain = vk::SwapchainKHR::null();
        state.surface = vk::SurfaceKHR::null();
    }

    /// Makes the swapchain, and the surface if it was lost, again. Returns
    /// whether it did.
    fn recreate_swapchain(&mut self, gpu: &mut VulkanGpu, recreate_surface: bool) -> bool {
        // Without a surface there is no swapchain, unless the surface is
        // made again.
        if self.state.surface == vk::SurfaceKHR::null() && !recreate_surface {
            return true;
        }
        self.device.wait_idle("swapchain recreation");
        self.destroy_images(Some(gpu));

        if recreate_surface {
            self.destroy_surface();
            let Some(provider) = &self.provider else {
                return false;
            };
            match provider.create_surface(self.device.shared.entry(), self.device.shared.instance()) {
                Ok(surface) => self.state.surface = surface,
                Err(error) => {
                    Log::error(Event::Render, error.message);
                    return false;
                }
            }
        }
        let (width, height) = (self.state.extent.width, self.state.extent.height);
        self.init(gpu, width, height)
    }

    /// Sets the size the swapchain is made with when the surface doesn't give
    /// one.
    pub(crate) fn set_extent(&mut self, size: Size) {
        self.state.extent = vk::Extent2D { width: size.width, height: size.height };
    }

    /// Has the swapchain rebuilt, a few frames from now or, without
    /// `use_delay`, before the next one.
    pub(crate) fn request_surface_update(&mut self, use_delay: bool) {
        let max_frames = self.max_frames;
        let state = &mut self.state;
        if state.update_requested {
            if !use_delay {
                state.update_latency = 0;
            }
            return;
        }
        state.update_requested = true;
        state.update_latency = if use_delay { max_frames as i32 * 3 } else { 0 };
    }

    fn update_surface(&mut self, gpu: &mut VulkanGpu) {
        // The request stays pending if this fails, so the next frame
        // retries it.
        if !self.recreate_swapchain(gpu, self.state.lost) {
            return;
        }
        self.state.lost = false;
        self.state.update_requested = false;
    }

    /// Has the next frame presented read back, with what the overlay drew
    /// over it.
    pub(crate) fn request_read(&mut self) {
        self.read_requested = true;
    }

    /// Whether a frame is still to be read back.
    pub(crate) fn is_read_requested(&self) -> bool {
        self.read_requested
    }

    /// Draws `overlay` over each frame from now on, or stops with `None`
    /// (`SurfaceTarget::set_overlay`). The overlay it replaces destroys what
    /// it made on `gpu`.
    pub(crate) fn set_overlay(&mut self, gpu: &mut VulkanGpu, overlay: Option<Box<dyn Overlay>>) {
        if let Some(mut old) = std::mem::replace(&mut self.overlay, overlay) {
            old.destroy(gpu);
        }
    }

    /// Acquires the next image, after the frame that last used its frame in
    /// flight's semaphore, and makes the swapchain again first where a
    /// frame asked for it. `None` where no image could be acquired. Until
    /// [`end_frame`](Self::end_frame), returns the image acquired.
    pub(crate) fn begin_frame(&mut self, gpu: &mut VulkanGpu) -> Option<FrameImage> {
        if let Some(acquired) = self.state.acquired {
            return self.frame_image(gpu, acquired);
        }
        // A lost surface may have been destroyed by a failed recreation.
        let platform_surface = self.state.surface != vk::SurfaceKHR::null();
        if (platform_surface || self.state.lost) && self.state.update_requested {
            self.state.update_latency -= 1;
            if self.state.update_latency <= 0 {
                self.update_surface(gpu);
            }
        }
        let acquired = match self.acquire(gpu) {
            Ok(acquired) => acquired,
            Err(error) => {
                Log::error(Event::Render, format!("Acquiring a swapchain image failed: {error}"));
                return None;
            }
        };
        self.state.acquired = Some(acquired);
        let image = self.frame_image(gpu, acquired)?;
        let semaphore = self.state.acquire_semaphores.get(acquired.slot)?.0;
        let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT;
        if let Err(error) = gpu.wait_before_using(image.color, semaphore, None, stages) {
            Log::error(Event::Render, format!("Waiting for the acquired image failed: {error}"));
        }
        Some(image)
    }

    /// The textures that the frame of `acquired` draws into.
    fn frame_image(&self, gpu: &VulkanGpu, acquired: Acquired) -> Option<FrameImage> {
        let state = &self.state;
        let color = state.images.get(acquired.image as usize)?.texture;
        let targets =
            TargetFormats { color: gpu.texture_format(state.color_format), depth_stencil: Some(DEPTH_STENCIL) };
        Some(FrameImage {
            size: Size::new(state.extent.width, state.extent.height),
            color,
            depth_stencil: state.depth_stencil,
            targets,
        })
    }

    /// Acquires an image with the next frame in flight's semaphore, making
    /// the swapchain again where it is out of date.
    fn acquire(&mut self, gpu: &mut VulkanGpu) -> Result<Acquired, String> {
        let device = self.device.clone();
        let loader = device.shared.swapchain().ok_or("the device has no swapchains")?;
        for _ in 0..MAXIMUM_SURFACE_UPDATE_RETRIES {
            let (swapchain, slot, semaphore, serial) = {
                let state = &mut self.state;
                // A swapchain that couldn't be rebuilt has no images, and a
                // lost surface that couldn't be made again none either.
                if state.surface == vk::SurfaceKHR::null() || state.images.is_empty() {
                    return Err("the surface has no swapchain".to_owned());
                }
                let slot = state.next_slot;
                state.next_slot = (slot + 1) % state.acquire_semaphores.len();
                let (semaphore, serial) = state.acquire_semaphores[slot];
                (state.swapchain, slot, semaphore, serial)
            };
            // The frame that last acquired with the semaphore has waited for
            // it.
            gpu.wait_for(serial).map_err(|error| format!("waiting for a frame: {error}"))?;
            // SAFETY: the swapchain and the semaphore are the surface's, and
            // the semaphore has no signal pending.
            let result = unsafe { loader.acquire_next_image(swapchain, u64::MAX, semaphore, vk::Fence::null()) };
            match result {
                Ok((image, suboptimal)) => {
                    if suboptimal {
                        self.request_surface_update(true);
                    }
                    return Ok(Acquired { image, slot });
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.request_surface_update(false);
                    self.update_surface(gpu);
                }
                Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                    self.state.lost = true;
                    self.request_surface_update(false);
                    self.update_surface(gpu);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("the swapchain stays out of date".to_owned())
    }

    /// Has the overlay draw over the acquired image in a submit of its own.
    fn draw_overlay(&mut self, gpu: &mut VulkanGpu, texture: TextureId) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let begun = gpu.load_pass(texture).and_then(|pass| Ok((pass, gpu.begin_recording()?)));
        let ((render_pass, framebuffer, extent), command_buffer) = match begun {
            Ok(begun) => begun,
            Err(error) => {
                Log::error(Event::Render, format!("Beginning the overlay failed: {error}"));
                return;
            }
        };
        let device = self.device.shared.device();
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent });
        self.device.begin_debug_label(command_buffer, "overlay", [0.0; 4]);
        // SAFETY: the command buffer is recording outside a render pass, and
        // the framebuffer is the render pass's, over the acquired image in
        // the layout the frame's passes left it in.
        unsafe { device.cmd_begin_render_pass(command_buffer, &begin_info, vk::SubpassContents::INLINE) };
        overlay.draw(&mut OverlayTarget {
            gpu,
            device,
            command_buffer,
            render_pass,
            size: Size::new(extent.width, extent.height),
        });
        // SAFETY: the command buffer is in the overlay's render pass.
        unsafe { device.cmd_end_render_pass(command_buffer) };
        self.device.end_debug_label(command_buffer);
        gpu.take_waits_into_next_submit(texture);
        if let Err(error) = gpu.submit_recording(command_buffer) {
            Log::error(Event::Render, format!("Submitting the overlay failed: {error}"));
        }
    }

    /// Draws the overlay over the acquired image, reads it back if asked,
    /// and presents it. Returns whether the frame had an image to present.
    pub(crate) fn end_frame(&mut self, gpu: &mut VulkanGpu) -> bool {
        let Some(acquired) = self.state.acquired.take() else {
            return false;
        };
        let (texture, present_semaphore, swapchain, readable) = {
            let state = &self.state;
            let Some(image) = state.images.get(acquired.image as usize) else {
                return false;
            };
            (image.texture, image.present_semaphore, state.swapchain, state.readable)
        };
        self.draw_overlay(gpu, texture);
        if std::mem::take(&mut self.read_requested) {
            if readable {
                let extent = self.get_extent();
                let size = Size::new(extent.width, extent.height);
                match gpu.read_texture(texture, TextureRegion::whole(size)) {
                    Ok(readback) => {
                        if let Some((previous, _)) = self.readback.replace((readback, size)) {
                            destroy_unused(gpu, [ResourceId::Readback(previous)]);
                        }
                    }
                    Err(error) => Log::error(Event::Render, format!("Reading the frame back failed: {error}")),
                }
            } else {
                Log::warning(Event::Render, "The surface's images can't be read");
            }
        }
        gpu.take_waits_into_next_submit(texture);
        gpu.signal_after_next_submit(present_semaphore, None);
        let submitted = gpu.flush(false);
        let serial = gpu.last_serial();
        if let Some(entry) = self.state.acquire_semaphores.get_mut(acquired.slot) {
            entry.1 = serial;
        }
        if let Err(error) = submitted {
            // The image can't be presented without the semaphore: a new
            // swapchain takes it back.
            Log::error(Event::Render, format!("Submitting the frame failed: {error}"));
            self.request_surface_update(false);
            return false;
        }

        let swapchains = [swapchain];
        let image_indices = [acquired.image];
        let wait_semaphores = [present_semaphore];
        let present_info = vk::PresentInfoKHR::default()
            .swapchains(&swapchains)
            .wait_semaphores(&wait_semaphores)
            .image_indices(&image_indices);
        // The image was acquired from the swapchain, and the submit signals
        // the semaphore.
        match self.device.present(&present_info) {
            Ok(true) => self.request_surface_update(true),
            Ok(false) => {}
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.request_surface_update(false),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                // The frame was submitted, recreate the surface before the
                // next one.
                self.state.lost = true;
                self.request_surface_update(false);
            }
            Err(error) => Log::error(Event::Render, format!("queuePresentKHR result: {error}")),
        }
        true
    }

    /// The last frame read back, once its readback has finished, if any.
    pub(crate) fn read_image(&mut self, gpu: &mut VulkanGpu) -> Option<PremultipliedImage> {
        let (readback, size) = self.readback.take()?;
        let data = match gpu.take_readback(readback, true) {
            Ok(data) => data?,
            Err(error) => {
                Log::error(Event::Render, format!("Reading the frame back failed: {error}"));
                return None;
            }
        };
        let mut image = PremultipliedImage::from_vec(size, data);
        // Swizzled to RGBA.
        if self.get_color_format() == vk::Format::B8G8R8A8_UNORM {
            for pixel in image.data.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        Some(image)
    }

    /// Destroys the overlay's objects and the resource's textures in `gpu`,
    /// and the swapchain and the surface, after the device's work.
    pub(crate) fn tear_down(&mut self, gpu: &mut VulkanGpu) {
        self.device.wait_idle("surface deletion");
        if let Some(mut overlay) = self.overlay.take() {
            overlay.destroy(gpu);
        }
        self.destroy_images(Some(gpu));
        self.destroy_surface();
        self.torn_down = true;
    }
}

impl Drop for SurfaceResource {
    /// Destroys the native objects that a tear-down didn't, after the
    /// device's work, and leaves their textures to the `Gpu`.
    fn drop(&mut self) {
        if self.torn_down {
            return;
        }
        self.device.wait_idle("surface deletion");
        self.destroy_images(None);
        self.destroy_surface();
    }
}
