// Ports platform/glfw/glfw_vulkan_backend.hpp and
// platform/glfw/glfw_vulkan_backend.cpp: a `vulkan::RendererBackend` that is
// its own default `gfx::Renderable` and presents to a window's surface
// through a swapchain. The window stays with the host (such as
// examples/desktop), which makes the surface (`SurfaceProvider`) and reports
// framebuffer resizes through `set_framebuffer_size`.
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
// - Port-only: `acquire_next_texture` says whether a frame can be drawn,
//   `request_readback` copies a presented frame into an image,
//   `set_overlay` draws over each frame before it is presented, such as an
//   app's controls over the map, and `release_surface` gives the window up
//   until `restore_surface`, as the WebGPU backend's `release_surface` and
//   `set_surface` do.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use ash::vk;
use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderable::{Renderable, RenderableBase};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::context::Context;
use crate::renderable_resource::{
    RenderableResource as _, RenderableResourceBox, SurfaceRenderableResource, downcast_resource,
};
use crate::renderer_backend::{RendererBackend, SurfaceProvider};

/// Port-only: what an overlay draws with (`SurfaceBackend::set_overlay`):
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

/// Port-only: draws over a frame (`SurfaceBackend::set_overlay`).
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The mode a surface that has the modes `available` presents in: without
/// tearing when the frame rate is capped, and otherwise without waiting for
/// the display, tearing (immediate) rather than not (mailbox). Every surface
/// has FIFO.
///
/// A capped surface prefers mailbox to FIFO, as the WebGPU backend does:
/// FIFO blocks acquiring and presenting on Wayland while the window isn't
/// visible, which stalls the thread that renders.
pub fn present_mode(cap_frame_rate: bool, available: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
    let preferred: &[vk::PresentModeKHR] = if cap_frame_rate {
        &[vk::PresentModeKHR::MAILBOX]
    } else {
        &[vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::MAILBOX]
    };
    preferred.iter().copied().find(|mode| available.contains(mode)).unwrap_or(vk::PresentModeKHR::FIFO)
}

/// `GLFWVulkanBackend` without the window: a `vulkan::RendererBackend` and
/// `gfx::Renderable` presenting to a surface.
pub struct SurfaceBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
    /// The backend's context, which `create_context` hands out.
    context: Rc<Context>,
    /// Whether `take_readback` has a frame to wait for or to return.
    readback_requested: Cell<bool>,
    /// Whether the surface is released (`release_surface`).
    released: Cell<bool>,
}

impl SurfaceBackend {
    /// `GLFWVulkanBackend(window, capFrameRate)`: a backend on a device
    /// that can present to the surface `provider` makes, with a swapchain
    /// of `framebuffer_size`. With `cap_frame_rate` off, frames are
    /// presented without waiting for the display where the surface can
    /// (`present_mode`).
    ///
    /// Fails as [`RendererBackend::new`] does, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the window gets no
    /// swapchain.
    pub fn new(provider: Rc<dyn SurfaceProvider>, framebuffer_size: Size, cap_frame_rate: bool) -> Result<Self, Error> {
        let (renderer_backend, surface) = RendererBackend::new(ContextMode::Unique, Some(&*provider))?;
        let context = renderer_backend.create_context();
        let backend_impl = renderer_backend.backend_impl();
        // SAFETY: the surface is the instance's, and the device presents to
        // it.
        let present_modes = surface.and_then(|surface| unsafe {
            backend_impl
                .get_surface_loader()
                .get_physical_device_surface_present_modes(backend_impl.get_physical_device(), surface)
                .ok()
        });
        // The resource owns the surface from here on.
        let resource = SurfaceRenderableResource::new(
            &context,
            framebuffer_size,
            surface,
            Some(provider),
            present_mode(cap_frame_rate, &present_modes.unwrap_or_default()),
            /*read_every_frame=*/ false,
        );
        if !resource.has_platform_surface() {
            return Err(Error::native("The window has no Vulkan surface"));
        }
        // A window without a size yet gets its swapchain with one.
        if resource.get_image_count() == 0 && framebuffer_size.width > 0 && framebuffer_size.height > 0 {
            return Err(Error::native("Failed to create a swapchain for the window"));
        }
        context.set_surface(&resource);
        Ok(SurfaceBackend {
            renderer_backend,
            renderable: RenderableBase::new(framebuffer_size, Some(RenderableResourceBox::new_rc(resource))),
            context,
            readback_requested: Cell::new(false),
            released: Cell::new(false),
        })
    }

    pub fn vulkan_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// Runs `f` with the surface's renderable resource.
    fn with_resource<R>(&self, f: impl FnOnce(&SurfaceRenderableResource) -> R) -> Option<R> {
        let resource = self.get_resource()?;
        downcast_resource::<SurfaceRenderableResource>(&*resource).map(f)
    }

    /// The name of the GPU.
    pub fn get_device_name(&self) -> String {
        self.renderer_backend
            .backend_impl()
            .get_device_properties()
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The format of the swapchain's images.
    pub fn get_color_format(&self) -> vk::Format {
        self.with_resource(SurfaceRenderableResource::get_color_format).unwrap_or_default()
    }

    /// `getFramebufferSize()`
    pub fn get_framebuffer_size(&self) -> Size {
        self.get_size()
    }

    /// `setFramebufferSize(Size)`: the size of the window's framebuffer, in
    /// pixels. The next frame is drawn into a swapchain of that size, or of
    /// the size the surface has where the window system gives it one.
    pub fn set_framebuffer_size(&self, size: Size) {
        self.set_renderable_size(size);
        if size.width == 0 || size.height == 0 {
            return;
        }
        let rebuild = self.with_resource(|resource| {
            let extent = resource.get_extent();
            let changed = extent.width != size.width || extent.height != size.height;
            if changed {
                resource.set_extent(size);
            }
            changed || resource.get_image_count() == 0
        });
        // A released surface gets its swapchain when it is restored.
        if rebuild == Some(true) && !self.released.get() {
            self.context.request_surface_update(false);
        }
    }

    /// Port-only: whether a frame can be drawn now: not into an empty
    /// framebuffer, and not without the surface. The frame acquires its
    /// image when it begins (`Context::begin_frame`).
    pub fn acquire_next_texture(&self) -> bool {
        let size = self.get_size();
        size.width > 0 && size.height > 0 && !self.released.get()
    }

    /// Port-only: destroys the swapchain and the surface, so that the host
    /// can destroy the window or give it to another backend, and keeps the
    /// device and everything built on it. Until `restore_surface`,
    /// `acquire_next_texture` returns false, and no frame may be drawn.
    pub fn release_surface(&self) {
        if self.with_resource(SurfaceRenderableResource::release_surface).is_some() {
            self.released.set(true);
        }
    }

    /// Port-only: presents to the window again after `release_surface`: the
    /// next frame has the provider make the surface, and builds the
    /// swapchain at the framebuffer's size.
    pub fn restore_surface(&self) {
        if self.released.replace(false) {
            self.context.request_surface_recreation();
        }
    }

    /// Port-only: copy the frame being drawn, or else the next one, when it
    /// is presented (see `take_readback`).
    pub fn request_readback(&self) {
        if self.with_resource(SurfaceRenderableResource::queue_surface_read).is_some() {
            self.readback_requested.set(true);
        }
    }

    /// Port-only: the frame copied after `request_readback`, once
    /// presented, with what the overlay drew over it.
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        if !self.readback_requested.get() {
            return None;
        }
        let image = self.with_resource(|resource| {
            if resource.is_surface_read_queued() {
                return None;
            }
            self.readback_requested.set(false);
            resource.read_image()
        });
        image.flatten()
    }

    /// Port-only: draws `overlay` over each frame from now on, after the
    /// map and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        self.with_resource(|resource| resource.set_overlay(overlay));
    }
}

impl Drop for SurfaceBackend {
    fn drop(&mut self) {
        // The overlay's resources go through the context's deletion queues,
        // and the swapchain's with the resource, before the context.
        self.set_overlay(None);
        self.set_resource(None);
        self.context.destroy_resources();
        self.renderer_backend.reset_context();
    }
}

impl Renderable for SurfaceBackend {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for SurfaceBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.renderer_backend.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_default_renderable(&self) -> &dyn Renderable {
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.renderer_backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        self.context.clone()
    }

    fn activate(&self) {}

    fn deactivate(&self) {}
}
