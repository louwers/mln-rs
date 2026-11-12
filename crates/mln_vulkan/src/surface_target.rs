// Ports the presenting part of platform/glfw/glfw_vulkan_backend.hpp and
// platform/glfw/glfw_vulkan_backend.cpp: frames presented to a window's
// surface through a swapchain. The window stays with the host, which hands
// over its handles and reports framebuffer resizes through `resize`.
//
// Differences from C++:
// - `USE_SHARED_VK_CONTEXT`, the example of an instance and a device that
//   the application made, is not ported.
// - Without `cap_frame_rate`, C++ asks for the immediate present mode and
//   falls back to FIFO; the port takes mailbox before FIFO
//   (`present_mode`).
// - A resize rebuilds the swapchain before the next frame, where C++ waits
//   a few frames (`requestSurfaceUpdate`'s delay): a Wayland surface has the
//   size of its swapchain's images, so a late rebuild shows frames of the
//   old size scaled. An empty framebuffer, which a minimized window has,
//   keeps the swapchain as it is.
// - Port-only: `request_readback` copies a presented frame into an image,
//   and `set_overlay` draws over each frame before it is presented, such as
//   an app's controls over the map.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use mln_gfx::Window;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::context::Context;
use crate::device::Device;
use crate::draw_target::{DrawTarget, TargetResource as _};
use crate::renderer_backend::{RendererBackend, SurfaceProvider};
use crate::surface_resource::SurfaceResource;
use crate::window::WindowSurface;

/// The host's `VkSurfaceKHR`, which stays the host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostSurface(vk::SurfaceKHR);

impl HostSurface {
    /// # Safety
    ///
    /// `surface` is a live surface of the instance of the device that the
    /// targets on it render on, and it outlives them.
    #[must_use]
    pub unsafe fn from_raw(surface: u64) -> HostSurface {
        use ash::vk::Handle as _;
        HostSurface(vk::SurfaceKHR::from_raw(surface))
    }
}

/// Port-only: what an overlay draws with (`SurfaceTarget::set_overlay`):
/// the frame's command buffer, inside a render pass over the frame's image
/// that keeps what the map drew.
pub struct OverlayTarget<'a> {
    /// The backend's context, for the overlay's buffers and textures. The
    /// frame is in a render pass, so a texture uploaded now is uploaded
    /// before the frame's commands run.
    pub context: &'a Context,
    pub device: &'a ash::Device,
    /// The frame's command buffer, which the overlay records its draws
    /// into. What they use must live until the frame is done
    /// (`Context::enqueue_deletion`).
    pub command_buffer: vk::CommandBuffer,
    /// The render pass the command buffer is in, for the overlay's
    /// pipelines: one color attachment of the surface's format, no depth or
    /// stencil. It is the same for every frame of a surface format.
    pub render_pass: vk::RenderPass,
    /// The size of the frame's image, in physical pixels.
    pub size: Size,
}

/// Port-only: draws over a frame (`SurfaceTarget::set_overlay`).
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The mode a surface that has the modes `available` presents in: without
/// tearing when the frame rate is capped, and otherwise without waiting for
/// the display, tearing (immediate) rather than not (mailbox). Every surface
/// has FIFO.
///
/// A capped surface prefers mailbox to FIFO, as the WebGPU backend does:
/// FIFO blocks acquiring and presenting on Wayland while the window isn't
/// visible, which stalls the thread that renders.
#[must_use]
pub fn present_mode(cap_frame_rate: bool, available: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
    let preferred: &[vk::PresentModeKHR] = if cap_frame_rate {
        &[vk::PresentModeKHR::MAILBOX]
    } else {
        &[vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::MAILBOX]
    };
    preferred.iter().copied().find(|mode| available.contains(mode)).unwrap_or(vk::PresentModeKHR::FIFO)
}

/// `GLFWVulkanBackend`'s swapchain without the window: frames presented to
/// a window's surface.
pub struct SurfaceTarget {
    backend: Rc<RendererBackend>,
    resource: Rc<SurfaceResource>,
    image: Rc<DrawTarget>,
    size: Cell<Size>,
    /// Whether the swapchain is rebuilt before the next frame.
    rebuild: Cell<bool>,
    /// Whether `take_readback` has a frame to wait for or to return.
    readback_requested: Cell<bool>,
}

impl SurfaceTarget {
    /// `GLFWVulkanBackend(window, capFrameRate)`: a presenting backend on
    /// `device` and a target that presents to `window` through a swapchain
    /// of `framebuffer_size` physical pixels. With `cap_frame_rate` off,
    /// frames are presented without waiting for the display where the
    /// surface can (`present_mode`).
    ///
    /// Fails as [`RendererBackend::presenting`] and [`new`](Self::new) do.
    pub fn with_backend(
        device: &Device,
        window: Arc<dyn Window>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<(Rc<RendererBackend>, SurfaceTarget), Error> {
        let provider = Rc::new(WindowSurface::new(window)?);
        Self::with_backend_from_provider(device, provider, framebuffer_size, cap_frame_rate)
    }

    /// [`with_backend`](Self::with_backend) for the surfaces `provider`
    /// makes, which may be of no window, such as `VK_EXT_headless_surface`'s.
    pub fn with_backend_from_provider(
        device: &Device,
        provider: Rc<dyn SurfaceProvider>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<(Rc<RendererBackend>, SurfaceTarget), Error> {
        let backend = Rc::new(RendererBackend::presenting(device)?);
        let target = Self::from_provider(backend.clone(), provider, framebuffer_size, cap_frame_rate)?;
        Ok((backend, target))
    }

    /// A target on `backend`, a presenting backend, that presents to
    /// `window`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// backend can't present to the window, or Vulkan has no surface for it,
    /// and with [`ErrorKind::Native`](mln_util::ErrorKind) when the window
    /// gets no surface or swapchain.
    pub fn new(
        backend: Rc<RendererBackend>,
        window: Arc<dyn Window>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<SurfaceTarget, Error> {
        let provider = Rc::new(WindowSurface::new(window)?);
        Self::from_provider(backend, provider, framebuffer_size, cap_frame_rate)
    }

    /// [`new`](Self::new) for the surfaces `provider` makes.
    pub fn from_provider(
        backend: Rc<RendererBackend>,
        provider: Rc<dyn SurfaceProvider>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<SurfaceTarget, Error> {
        let backend_impl = backend.backend_impl();
        let shared = &backend_impl.device().shared;
        let missing = provider.instance_extensions().into_iter().find(|name| !shared.has_instance_extension(name));
        if let Some(name) = missing {
            return Err(Error::unsupported(format!("The Vulkan instance lacks {}", name.to_string_lossy())));
        }
        let surface = provider.create_surface(backend_impl.entry(), backend_impl.get_instance())?;
        Self::on_surface(backend, surface, Some(provider), framebuffer_size, cap_frame_rate)
    }

    /// A target on `backend`, a presenting backend, that presents to the
    /// host's `surface`, which stays the host's: the target destroys the
    /// swapchain it makes, but not the surface, and can't make the surface
    /// again when it is lost.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device's queue can't present to the surface, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the surface gets no
    /// swapchain.
    pub fn on_host_surface(
        backend: Rc<RendererBackend>,
        surface: HostSurface,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<SurfaceTarget, Error> {
        Self::on_surface(backend, surface.0, None, framebuffer_size, cap_frame_rate)
    }

    /// A target that presents to `surface`, which the target owns when
    /// `provider` made it, and which `provider` makes again when it is lost.
    /// A surface the queue can't present to fails with `Unsupported`, and is
    /// destroyed when the target would own it.
    fn on_surface(
        backend: Rc<RendererBackend>,
        surface: vk::SurfaceKHR,
        provider: Option<Rc<dyn SurfaceProvider>>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<SurfaceTarget, Error> {
        let backend_impl = backend.backend_impl();
        let (Some(surface_loader), Some(_)) = (backend_impl.get_surface_loader(), backend_impl.get_swapchain_loader())
        else {
            return Err(Error::unsupported("The Vulkan device presents to no windows"));
        };
        // SAFETY: the surface is the instance's, and the queue family the
        // physical device's.
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                backend_impl.get_physical_device(),
                backend_impl.get_graphics_queue_index(),
                surface,
            )
        };
        if supported != Ok(true) {
            if provider.is_some() {
                // SAFETY: nothing was made on the surface, which is the
                // target's.
                unsafe { surface_loader.destroy_surface(surface, None) };
            }
            return Err(Error::unsupported("The Vulkan device's queue can't present to the surface"));
        }
        let context: &Rc<Context> = backend.vulkan_context();
        // SAFETY: the surface is the instance's, and the device presents to
        // it.
        let present_modes = unsafe {
            surface_loader
                .get_physical_device_surface_present_modes(backend_impl.get_physical_device(), surface)
                .unwrap_or_default()
        };
        let resource = SurfaceResource::new(
            context,
            framebuffer_size,
            Some(surface),
            provider,
            present_mode(cap_frame_rate, &present_modes),
            /*read_every_frame=*/ false,
        );
        if !resource.has_platform_surface() {
            return Err(Error::native("The window has no Vulkan surface"));
        }
        // A window without a size yet gets its swapchain with one.
        if resource.get_image_count() == 0 && framebuffer_size.width > 0 && framebuffer_size.height > 0 {
            return Err(Error::native("Failed to create a swapchain for the window"));
        }
        let image = Rc::new(DrawTarget::new(framebuffer_size, resource.clone()));
        Ok(SurfaceTarget {
            backend,
            resource,
            image,
            size: Cell::new(framebuffer_size),
            rebuild: Cell::new(false),
            readback_requested: Cell::new(false),
        })
    }

    /// The backend the target draws on.
    pub fn backend(&self) -> &Rc<RendererBackend> {
        &self.backend
    }

    /// The format of the swapchain's images.
    pub fn get_color_format(&self) -> vk::Format {
        self.resource.get_color_format()
    }

    /// The size of the frames, in physical pixels.
    pub fn size(&self) -> Size {
        self.size.get()
    }

    /// `setFramebufferSize(Size)`: the size of the window's framebuffer, in
    /// pixels. The next frame is drawn into a swapchain of that size, or of
    /// the size the surface has where the window system gives it one.
    pub fn resize(&self, size: Size) {
        self.size.set(size);
        self.image.set_size(size);
        if size.width == 0 || size.height == 0 {
            return;
        }
        let extent = self.resource.get_extent();
        let changed = extent.width != size.width || extent.height != size.height;
        if changed {
            self.resource.set_extent(size);
        }
        if changed || self.resource.get_image_count() == 0 {
            self.rebuild.set(true);
        }
    }

    /// Port-only: copy the frame being drawn, or else the next one, when it
    /// is presented (see `take_readback`).
    pub fn request_readback(&self) {
        self.resource.queue_surface_read();
        self.readback_requested.set(true);
    }

    /// Port-only: the frame copied after `request_readback`, once
    /// presented, with what the overlay drew over it.
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        if !self.readback_requested.get() || self.resource.is_surface_read_queued() {
            return None;
        }
        self.readback_requested.set(false);
        self.resource.read_image()
    }

    /// Port-only: draws `overlay` over each frame from now on, after the
    /// map and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        self.resource.set_overlay(overlay);
    }
}

impl FrameTarget for SurfaceTarget {
    /// The frame's image is acquired when the frame begins
    /// (`Context::begin_frame`). No frame is drawn into an empty
    /// framebuffer.
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let context = self.backend.vulkan_context();
        context.set_surface(&self.resource);
        if self.rebuild.take() {
            context.request_surface_update(false);
        }
        Some(self.image.clone())
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.resource.swap() { Handoff::Presented } else { Handoff::Dropped }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        // The overlay's resources go through the context's deletion queues,
        // and the swapchain and the surface with the resource.
        self.set_overlay(None);
    }
}
