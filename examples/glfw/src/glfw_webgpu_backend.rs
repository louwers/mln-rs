// Ports platform/glfw/glfw_webgpu_backend.hpp, glfw_webgpu_backend.cpp and
// the GLFW-facing part of glfw_webgpu_backend_impl.ipp (Linux). The surface
// handling (adapter, device, format choice, configure, acquire, present,
// depth texture, reconfigure) is `mln_webgpu::SurfaceBackend`.
//
// The surface comes from the window's raw handles: Wayland or X11, whichever
// platform GLFW runs on (C++ tries X11 first and Wayland second, but always
// runs GLFW on X11 for Dawn; see glfw_backend.rs).
//
// Deviation: C++ `swap()` ends with `glfwPollEvents()`; GLFWView polls once
// per frame tick anyway, so the port leaves event polling to the view.

use std::rc::Rc;

use mln_gfx::RendererBackend;
use mln_gfx::renderable::Renderable;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_webgpu::SurfaceBackend;

use crate::glfw_backend::GLFWBackend;

/// `GLFWWebGPUBackend`
pub struct GLFWWebGPUBackend {
    backend: Rc<SurfaceBackend>,
}

/// Creates the wgpu surface for `window`.
// FFI: the window's raw display and window handles.
#[allow(unsafe_code)]
fn create_surface(instance: &wgpu::Instance, window: &glfw::PWindow) -> Result<wgpu::Surface<'static>, String> {
    // SAFETY (FFI): the raw Wayland/X11 handles stay valid while the window
    // lives, and the window outlives the surface: GLFWView owns both and
    // drops its backend (the only owner of the surface once the renderer is
    // gone) before the window.
    let target = unsafe { wgpu::SurfaceTargetUnsafe::from_display_and_window(window, window) }
        .map_err(|e| format!("Failed to get the window handles from GLFW: {e}"))?;
    // SAFETY: as above.
    unsafe { instance.create_surface_unsafe(target) }
        .map_err(|e| format!("Failed to get X11 or Wayland window surface from GLFW: {e}"))
}

impl GLFWWebGPUBackend {
    /// `GLFWWebGPUBackend(GLFWwindow*, bool capFrameRate)`
    pub fn new(window: &glfw::PWindow, cap_frame_rate: bool) -> Result<Self, String> {
        // Add small delay to let previous resources clean up
        std::thread::sleep(std::time::Duration::from_millis(100));

        let (fb_width, fb_height) = window.get_framebuffer_size();
        let framebuffer_size = Size::new(fb_width.max(0) as u32, fb_height.max(0) as u32);

        let backend =
            SurfaceBackend::new(|instance| create_surface(instance, window), framebuffer_size, cap_frame_rate)?;

        // Final delay to ensure everything is ready
        std::thread::sleep(std::time::Duration::from_millis(50));

        Ok(GLFWWebGPUBackend { backend: Rc::new(backend) })
    }

    /// The surface backend (adapter info, readback).
    pub fn surface_backend(&self) -> &Rc<SurfaceBackend> {
        &self.backend
    }
}

impl GLFWBackend for GLFWWebGPUBackend {
    fn get_renderer_backend(&self) -> Rc<dyn RendererBackend> {
        // We inherit from webgpu::RendererBackend which is a gfx::RendererBackend
        self.backend.clone()
    }

    fn get_framebuffer_size(&self) -> Size {
        self.backend.get_size()
    }

    fn set_framebuffer_size(&self, size: Size) {
        self.backend.set_framebuffer_size(size);
    }

    fn begin_frame(&self) -> bool {
        self.backend.acquire_next_texture()
    }

    fn describe(&self) -> String {
        let info = self.backend.get_adapter_info();
        format!(
            "{} ({:?}, driver {} {}), surface format {:?}",
            info.name,
            info.backend,
            info.driver,
            info.driver_info,
            self.backend.get_swap_chain_format()
        )
    }

    fn request_readback(&self) {
        self.backend.request_readback();
    }

    fn take_readback(&self) -> Option<PremultipliedImage> {
        self.backend.take_readback()
    }
}
