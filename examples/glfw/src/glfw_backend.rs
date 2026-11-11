// Ports platform/glfw/glfw_backend.hpp, plus GLFW initialization from
// glfw_view.cpp (`glfwSetErrorCallback(glfwError)`, the platform init hint,
// `glfwInit`).
//
// Deviation: C++ forces the X11 platform for WebGPU ("Dawn doesn't support
// Wayland yet"). wgpu presents to Wayland surfaces, so the port tries
// Wayland first and falls back to X11 (`MLN_GLFW_PLATFORM=x11|wayland`
// forces one).

use std::rc::Rc;

use mln_gfx::RendererBackend;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::glfw_webgpu_backend::GLFWWebGPUBackend;

/// `GLFWBackend`: the rendering backend bound to a GLFW window.
pub trait GLFWBackend {
    fn get_renderer_backend(&self) -> Rc<dyn RendererBackend>;
    fn get_framebuffer_size(&self) -> Size;
    fn set_framebuffer_size(&self, size: Size);

    /// Port-only: false when no frame can be presented right now (see
    /// `SurfaceBackend::acquire_next_texture`).
    fn begin_frame(&self) -> bool;
    /// Port-only: the adapter in use, for the log.
    fn describe(&self) -> String;
    /// Port-only: copy the next presented frame (see `take_readback`).
    fn request_readback(&self);
    /// Port-only: the frame copied after `request_readback`.
    fn take_readback(&self) -> Option<PremultipliedImage>;
}

/// `GLFWBackend::Create(window, capFrameRate)`: the WebGPU backend, the only
/// one in this port.
pub fn create(window: &glfw::PWindow, cap_frame_rate: bool) -> Result<Box<dyn GLFWBackend>, String> {
    Ok(Box::new(GLFWWebGPUBackend::new(window, cap_frame_rate)?))
}

/// `glfwError`
fn glfw_error(error: glfw::Error, description: String) {
    Log::error(Event::GraphicsBackend, format!("GLFW error ({}): {description}", error.as_raw()));
}

fn platform_name(platform: glfw::Platform) -> &'static str {
    match platform {
        glfw::Platform::Wayland => "Wayland",
        glfw::Platform::X11 => "X11",
        glfw::Platform::Win32 => "Win32",
        glfw::Platform::MacOS => "Cocoa",
        glfw::Platform::Null => "Null",
        glfw::Platform::Any => "Any",
    }
}

/// `glfwSetErrorCallback(glfwError); glfwInitHint(GLFW_PLATFORM, ...);
/// glfwInit()`. On Linux it tries Wayland, then X11; elsewhere C++ sets no
/// platform hint (`#ifdef __linux__`), so glfwInit() picks the native
/// platform (Cocoa on macOS).
pub fn init_glfw() -> Result<glfw::Glfw, String> {
    let platforms: &[glfw::Platform] = if cfg!(target_os = "linux") {
        match std::env::var("MLN_GLFW_PLATFORM").as_deref() {
            Ok("x11") => &[glfw::Platform::X11],
            Ok("wayland") => &[glfw::Platform::Wayland],
            _ => &[glfw::Platform::Wayland, glfw::Platform::X11],
        }
    } else {
        &[glfw::Platform::Any]
    };
    for &platform in platforms {
        if platform != glfw::Platform::Any && !platform.is_supported() {
            continue;
        }
        glfw::init_hint(glfw::InitHint::Platform(platform));
        match glfw::init(glfw_error) {
            Ok(glfw) => {
                Log::info(Event::GraphicsBackend, format!("GLFW platform: {}", platform_name(glfw.get_platform())));
                return Ok(glfw);
            }
            Err(_) => Log::warning(
                Event::GraphicsBackend,
                format!("GLFW: the {} platform is not available", platform_name(platform)),
            ),
        }
    }
    Err("failed to initialize glfw".to_owned())
}
