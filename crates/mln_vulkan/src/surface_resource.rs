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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ash::vk;
use mln_gfx::frame::FrameImage;
use mln_gfx::gpu::{
    Gpu as _, ReadbackId, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage,
};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gpu::ImportedImage;
use crate::renderer_backend::{BackendImpl, RendererBackend, SurfaceProvider};
use crate::surface_target::{Overlay, OverlayTarget};

const MAXIMUM_SURFACE_UPDATE_RETRIES: u8 = 3;

/// The format of the depth and stencil attachment.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The extent that `surface`, a surface of `backend`'s instance,
/// has now, which is empty for a minimized window's. `None` when the
/// swapchain sets the surface's extent, as on Wayland, or the surface can't
/// say, such as once it is lost.
pub(crate) fn surface_extent(backend: &BackendImpl, surface: vk::SurfaceKHR) -> Option<vk::Extent2D> {
    let loader = backend.get_surface_loader()?;
    // SAFETY: the surface is the instance's.
    let capabilities =
        unsafe { loader.get_physical_device_surface_capabilities(backend.get_physical_device(), surface) }.ok()?;
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
/// `Gpu`.
pub(crate) struct SurfaceResource {
    renderer: Rc<RendererBackend>,
    provider: Option<Rc<dyn SurfaceProvider>>,
    /// Whether the resource destroys its surface: one that `provider` made,
    /// and not a host's.
    owns_surface: bool,
    state: RefCell<SurfaceState>,
    overlay: RefCell<Option<Box<dyn Overlay>>>,
    /// Whether the next frame presented is read back.
    read_requested: Cell<bool>,
    /// The readback of the last frame read, and its size.
    readback: Cell<Option<(ReadbackId, Size)>>,
}

impl SurfaceResource {
    /// A resource of `size` on `surface`. `provider` made the surface, which
    /// the resource then destroys, and makes it again when it is lost;
    /// without one, the surface is the host's. `present_mode` is used when
    /// the surface has it, FIFO otherwise.
    pub(crate) fn new(
        renderer: Rc<RendererBackend>,
        size: Size,
        surface: vk::SurfaceKHR,
        provider: Option<Rc<dyn SurfaceProvider>>,
        present_mode: vk::PresentModeKHR,
    ) -> Self {
        let resource = SurfaceResource {
            renderer,
            owns_surface: provider.is_some(),
            provider,
            state: RefCell::new(SurfaceState {
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
            }),
            overlay: RefCell::new(None),
            read_requested: Cell::new(false),
            readback: Cell::new(None),
        };
        resource.init(size.width, size.height);
        resource
    }

    fn backend(&self) -> &BackendImpl {
        self.renderer.backend_impl()
    }

    /// The extent of the resource's surface now, or `None` when
    /// it has no surface or its surface doesn't say ([`surface_extent`]).
    pub(crate) fn current_extent(&self) -> Option<vk::Extent2D> {
        let surface = self.state.borrow().surface;
        if surface == vk::SurfaceKHR::null() {
            return None;
        }
        surface_extent(self.backend(), surface)
    }

    /// Whether the resource presents to a surface: a lost one that couldn't
    /// be made again is gone.
    pub(crate) fn has_platform_surface(&self) -> bool {
        self.state.borrow().surface != vk::SurfaceKHR::null()
    }

    pub(crate) fn get_color_format(&self) -> vk::Format {
        self.state.borrow().color_format
    }

    pub(crate) fn get_image_count(&self) -> u32 {
        self.state.borrow().images.len() as u32
    }

    pub(crate) fn get_extent(&self) -> vk::Extent2D {
        self.state.borrow().extent
    }

    fn init_swapchain(&self, state: &mut SurfaceState, w: u32, h: u32) -> Result<Vec<vk::Image>, String> {
        let physical_device = self.backend().get_physical_device();
        let surface_loader = self.backend().get_surface_loader().ok_or("The instance has no surfaces")?;
        let swapchain_loader = self.backend().get_swapchain_loader().ok_or("The device has no swapchains")?;
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

    /// Makes the swapchain, its images and their textures, the depth and
    /// stencil attachment, and the semaphores.
    fn init(&self, w: u32, h: u32) -> bool {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        match self.init_swapchain(state, w, h).and_then(|images| self.init_images(state, &images)) {
            Ok(()) => true,
            Err(error) => {
                Log::error(Event::Render, error);
                self.destroy_images(state);
                false
            }
        }
    }

    fn init_images(&self, state: &mut SurfaceState, images: &[vk::Image]) -> Result<(), String> {
        let device = self.backend().get_device();
        let size = Size::new(state.extent.width, state.extent.height);
        let mut gpu = self.renderer.gpu();
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
            self.backend().set_debug_name(image, &format!("SwapchainImage_{index}"));
            self.backend().set_debug_name(view, &format!("SwapchainImageView_{index}"));
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
            self.backend().set_debug_name(semaphore, &format!("PresentSemaphore_{index}"));
            if let Some(image) = state.images.last_mut() {
                image.present_semaphore = semaphore;
            }
        }
        for index in 0..self.backend().get_max_frames() {
            // SAFETY: the create info is valid for the device.
            let semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                .map_err(|e| format!("Failed to create a semaphore: {e}"))?;
            self.backend().set_debug_name(semaphore, &format!("AcquireSemaphore_{index}"));
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

    /// Destroys what `init` made, but the swapchain and the surface. The
    /// device is idle.
    fn destroy_images(&self, state: &mut SurfaceState) {
        let device = self.backend().get_device();
        let mut gpu = self.renderer.gpu();
        if let Some((readback, _)) = self.readback.take() {
            gpu.destroy_readback(readback);
        }
        for image in state.images.drain(..) {
            gpu.destroy_texture(image.texture);
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
            gpu.destroy_texture(depth_stencil);
        }
        state.next_slot = 0;
        state.acquired = None;
    }

    /// Destroys the swapchain and the surface, whose images are gone
    /// (`destroy_images`).
    fn destroy_surface(&self, state: &mut SurfaceState) {
        // SAFETY: the device is idle, and the swapchain and the surface are
        // this resource's.
        unsafe {
            if state.swapchain != vk::SwapchainKHR::null()
                && let Some(loader) = self.backend().get_swapchain_loader()
            {
                loader.destroy_swapchain(state.swapchain, None);
            }
            if state.surface != vk::SurfaceKHR::null()
                && self.owns_surface
                && let Some(loader) = self.backend().get_surface_loader()
            {
                loader.destroy_surface(state.surface, None);
            }
        }
        state.swapchain = vk::SwapchainKHR::null();
        state.surface = vk::SurfaceKHR::null();
    }

    /// Makes the swapchain, and the surface if it was lost, again. Returns
    /// whether it did.
    fn recreate_swapchain(&self, recreate_surface: bool) -> bool {
        let (width, height) = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            // Without a surface there is no swapchain, unless the surface
            // is made again.
            if state.surface == vk::SurfaceKHR::null() && !recreate_surface {
                return true;
            }
            self.backend().wait_idle("swapchain recreation");
            self.destroy_images(state);

            if recreate_surface {
                self.destroy_surface(state);
                let Some(provider) = &self.provider else {
                    return false;
                };
                match provider.create_surface(self.backend().entry(), self.backend().get_instance()) {
                    Ok(surface) => state.surface = surface,
                    Err(error) => {
                        Log::error(Event::Render, error.message);
                        return false;
                    }
                }
            }
            (state.extent.width, state.extent.height)
        };
        self.init(width, height)
    }

    /// Sets the size the swapchain is made with when the surface doesn't give
    /// one.
    pub(crate) fn set_extent(&self, size: Size) {
        self.state.borrow_mut().extent = vk::Extent2D { width: size.width, height: size.height };
    }

    /// Has the swapchain rebuilt, a few frames from now or, without
    /// `use_delay`, before the next one.
    pub(crate) fn request_surface_update(&self, use_delay: bool) {
        let mut state = self.state.borrow_mut();
        if state.update_requested {
            if !use_delay {
                state.update_latency = 0;
            }
            return;
        }
        state.update_requested = true;
        state.update_latency = if use_delay { self.backend().get_max_frames() as i32 * 3 } else { 0 };
    }

    fn update_surface(&self) {
        let lost = self.state.borrow().lost;
        // The request stays pending if this fails, so the next frame
        // retries it.
        if !self.recreate_swapchain(lost) {
            return;
        }
        let mut state = self.state.borrow_mut();
        state.lost = false;
        state.update_requested = false;
    }

    /// Has the next frame presented read back, with what the overlay drew
    /// over it.
    pub(crate) fn request_read(&self) {
        self.read_requested.set(true);
    }

    /// Whether a frame is still to be read back.
    pub(crate) fn is_read_requested(&self) -> bool {
        self.read_requested.get()
    }

    /// Draws `overlay` over each frame from now on, or stops with `None`
    /// (`SurfaceTarget::set_overlay`). The overlay it replaces destroys what
    /// it made.
    pub(crate) fn set_overlay(&self, overlay: Option<Box<dyn Overlay>>) {
        let old = self.overlay.replace(overlay);
        if let Some(mut old) = old {
            old.destroy(&mut self.renderer.gpu());
        }
    }

    /// Acquires the next image, after the frame that last used its frame in
    /// flight's semaphore, and makes the swapchain again first where a
    /// frame asked for it. `None` where no image could be acquired. Until
    /// [`end_frame`](Self::end_frame), returns the image acquired.
    pub(crate) fn begin_frame(&self) -> Option<FrameImage> {
        if let Some(acquired) = self.state.borrow().acquired {
            return self.frame_image(acquired);
        }
        {
            let mut state = self.state.borrow_mut();
            // A lost surface may have been destroyed by a failed recreation.
            let platform_surface = state.surface != vk::SurfaceKHR::null();
            if (platform_surface || state.lost) && state.update_requested {
                state.update_latency -= 1;
                if state.update_latency <= 0 {
                    drop(state);
                    self.update_surface();
                }
            }
        }
        let acquired = match self.acquire() {
            Ok(acquired) => acquired,
            Err(error) => {
                Log::error(Event::Render, format!("Acquiring a swapchain image failed: {error}"));
                return None;
            }
        };
        self.state.borrow_mut().acquired = Some(acquired);
        let image = self.frame_image(acquired)?;
        let semaphore = self.state.borrow().acquire_semaphores.get(acquired.slot)?.0;
        let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT;
        if let Err(error) = self.renderer.gpu().wait_before_using(image.color, semaphore, None, stages) {
            Log::error(Event::Render, format!("Waiting for the acquired image failed: {error}"));
        }
        Some(image)
    }

    /// The textures that the frame of `acquired` draws into.
    fn frame_image(&self, acquired: Acquired) -> Option<FrameImage> {
        let state = self.state.borrow();
        let color = state.images.get(acquired.image as usize)?.texture;
        let targets = TargetFormats {
            color: self.renderer.gpu().texture_format(state.color_format),
            depth_stencil: Some(DEPTH_STENCIL),
        };
        Some(FrameImage {
            size: Size::new(state.extent.width, state.extent.height),
            color,
            depth_stencil: state.depth_stencil,
            targets,
        })
    }

    /// Acquires an image with the next frame in flight's semaphore, making
    /// the swapchain again where it is out of date.
    fn acquire(&self) -> Result<Acquired, String> {
        let loader = self.backend().get_swapchain_loader().ok_or("the device has no swapchains")?;
        for _ in 0..MAXIMUM_SURFACE_UPDATE_RETRIES {
            let (swapchain, slot, semaphore, serial) = {
                let mut state = self.state.borrow_mut();
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
            self.renderer.gpu().wait_for(serial).map_err(|error| format!("waiting for a frame: {error}"))?;
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
                    self.update_surface();
                }
                Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                    self.state.borrow_mut().lost = true;
                    self.request_surface_update(false);
                    self.update_surface();
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("the swapchain stays out of date".to_owned())
    }

    /// Has the overlay draw over the acquired image in a submit of its own.
    fn draw_overlay(&self, texture: TextureId) {
        let mut overlay = self.overlay.borrow_mut();
        let Some(overlay) = overlay.as_mut() else {
            return;
        };
        let mut gpu = self.renderer.gpu();
        let begun = gpu.load_pass(texture).and_then(|pass| Ok((pass, gpu.begin_recording()?)));
        let ((render_pass, framebuffer, extent), command_buffer) = match begun {
            Ok(begun) => begun,
            Err(error) => {
                Log::error(Event::Render, format!("Beginning the overlay failed: {error}"));
                return;
            }
        };
        let backend = self.backend();
        let device = backend.get_device();
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent });
        backend.begin_debug_label(command_buffer, "overlay", [0.0; 4]);
        // SAFETY: the command buffer is recording outside a render pass, and
        // the framebuffer is the render pass's, over the acquired image in
        // the layout the frame's passes left it in.
        unsafe { device.cmd_begin_render_pass(command_buffer, &begin_info, vk::SubpassContents::INLINE) };
        overlay.draw(&mut OverlayTarget {
            gpu: &mut gpu,
            device,
            command_buffer,
            render_pass,
            size: Size::new(extent.width, extent.height),
        });
        // SAFETY: the command buffer is in the overlay's render pass.
        unsafe { device.cmd_end_render_pass(command_buffer) };
        backend.end_debug_label(command_buffer);
        gpu.take_waits_into_next_submit(texture);
        if let Err(error) = gpu.submit_recording(command_buffer) {
            Log::error(Event::Render, format!("Submitting the overlay failed: {error}"));
        }
    }

    /// Draws the overlay over the acquired image, reads it back if asked,
    /// and presents it. Returns whether the frame had an image to present.
    pub(crate) fn end_frame(&self) -> bool {
        let Some(acquired) = self.state.borrow_mut().acquired.take() else {
            return false;
        };
        let (texture, present_semaphore, swapchain, readable) = {
            let state = self.state.borrow();
            let Some(image) = state.images.get(acquired.image as usize) else {
                return false;
            };
            (image.texture, image.present_semaphore, state.swapchain, state.readable)
        };
        self.draw_overlay(texture);
        let mut gpu = self.renderer.gpu();
        if self.read_requested.take() {
            if readable {
                let extent = self.get_extent();
                let size = Size::new(extent.width, extent.height);
                match gpu.read_texture(texture, TextureRegion::whole(size)) {
                    Ok(readback) => {
                        if let Some((previous, _)) = self.readback.replace(Some((readback, size))) {
                            gpu.destroy_readback(previous);
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
        let submitted = gpu.submit(&[]);
        let serial = gpu.last_serial();
        drop(gpu);
        if let Some(entry) = self.state.borrow_mut().acquire_semaphores.get_mut(acquired.slot) {
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
        match self.backend().present(&present_info) {
            Ok(true) => self.request_surface_update(true),
            Ok(false) => {}
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.request_surface_update(false),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                // The frame was submitted, recreate the surface before the
                // next one.
                self.state.borrow_mut().lost = true;
                self.request_surface_update(false);
            }
            Err(error) => Log::error(Event::Render, format!("queuePresentKHR result: {error}")),
        }
        true
    }

    /// The last frame read back, once its readback has finished, if any.
    pub(crate) fn read_image(&self) -> Option<PremultipliedImage> {
        let (readback, size) = self.readback.take()?;
        let data = match self.renderer.gpu().take_readback(readback, true) {
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
}

impl Drop for SurfaceResource {
    fn drop(&mut self) {
        self.backend().wait_idle("surface deletion");
        if let Some(mut overlay) = self.overlay.take() {
            overlay.destroy(&mut self.renderer.gpu());
        }
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        self.destroy_images(state);
        self.destroy_surface(state);
    }
}
