// The window's renderer backend: Metal by default on macOS, WebGPU (through
// wgpu, the default where there is no Metal) everywhere, Vulkan elsewhere
// than on Apple's platforms and OpenGL ES on Linux, picked with --backend and
// switched in the controls. C++'s GLFW app builds one backend and ignores the
// flag. On macOS both present to the window's one CAMetalLayer
// (metal_layer.rs). Elsewhere each makes its own surface of the window
// (vulkan_surface.rs for Vulkan, EGL for OpenGL ES), which takes one
// backend's frames at a time: the backend that goes gives it up before the
// next one takes it.
//
// OpenGL ES is Linux's only: EGL is no system library on Windows, and macOS
// has neither it nor OpenGL ES.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use mln_gfx::RendererBackend;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use winit::event_loop::EventLoopProxy;

use crate::host::UserEvent;
use crate::overlay::Paint;

/// A backend --backend names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    #[cfg(target_os = "macos")]
    Metal,
    #[cfg(not(target_vendor = "apple"))]
    Vulkan,
    WebGpu,
    #[cfg(target_os = "linux")]
    OpenGl,
}

impl Kind {
    /// The backend of a --backend value.
    pub(crate) fn parse(name: &str) -> Result<Kind, String> {
        match name {
            #[cfg(target_os = "macos")]
            "metal" => Ok(Kind::Metal),
            #[cfg(not(target_vendor = "apple"))]
            "vulkan" => Ok(Kind::Vulkan),
            "webgpu" => Ok(Kind::WebGpu),
            #[cfg(target_os = "linux")]
            "opengl" => Ok(Kind::OpenGl),
            _ => Err(format!("Unknown backend '{name}' (available: {})", Kind::NAMES)),
        }
    }

    #[cfg(target_os = "macos")]
    const NAMES: &str = "metal, webgpu";
    #[cfg(target_os = "linux")]
    const NAMES: &str = "vulkan, webgpu, opengl";
    #[cfg(all(not(target_vendor = "apple"), not(target_os = "linux")))]
    const NAMES: &str = "vulkan, webgpu";
    #[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
    const NAMES: &str = "webgpu";

    /// The backends this platform has.
    pub(crate) const ALL: &[Kind] = &[
        #[cfg(target_os = "macos")]
        Kind::Metal,
        #[cfg(not(target_vendor = "apple"))]
        Kind::Vulkan,
        Kind::WebGpu,
        #[cfg(target_os = "linux")]
        Kind::OpenGl,
    ];

    /// The backend after this one in `ALL`, around to the first.
    pub(crate) fn next(self) -> Kind {
        let index = Kind::ALL.iter().position(|&kind| kind == self).unwrap_or(0);
        Kind::ALL[(index + 1) % Kind::ALL.len()]
    }

    /// The backend's name in the controls.
    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => "Metal",
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => "Vulkan",
            Kind::WebGpu => "WebGPU",
            #[cfg(target_os = "linux")]
            Kind::OpenGl => "OpenGL ES",
        }
    }
}

impl Default for Kind {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        return Kind::Metal;
        #[cfg(not(target_os = "macos"))]
        return Kind::WebGpu;
    }
}

/// The backend presenting to the window.
pub(crate) enum SurfaceBackend {
    #[cfg(target_os = "macos")]
    Metal(Rc<mln_mtl::SurfaceBackend>),
    #[cfg(not(target_vendor = "apple"))]
    Vulkan(Rc<mln_vulkan::SurfaceBackend>),
    WebGpu(Rc<mln_webgpu::SurfaceBackend>),
    #[cfg(target_os = "linux")]
    OpenGl(Rc<mln_gl::SurfaceBackend>),
}

/// The window's thread ended before it answered the render thread.
pub(crate) fn closed() -> Error {
    Error::cancelled("The window closed before it had a surface")
}

/// A wgpu surface of the window for `instance`, which the window's thread
/// makes when asked through `proxy`.
fn make_surface(proxy: &EventLoopProxy<UserEvent>, instance: &wgpu::Instance) -> Result<wgpu::Surface<'static>, Error> {
    let (reply, surface) = mpsc::channel();
    proxy.send_event(UserEvent::MakeSurface(instance.clone(), reply)).map_err(|_| closed())?;
    surface.recv().map_err(|_| closed())?.map_err(Error::native)
}

impl SurfaceBackend {
    /// A backend of `kind` presenting to the window, whose surface the
    /// window's thread makes when asked through `proxy`.
    pub(crate) fn new(
        kind: Kind,
        proxy: &EventLoopProxy<UserEvent>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<SurfaceBackend, Error> {
        match kind {
            #[cfg(target_os = "macos")]
            Kind::Metal => {
                let (reply, layer) = mpsc::channel();
                proxy.send_event(UserEvent::MakeLayer(reply)).map_err(|_| closed())?;
                let layer = layer.recv().map_err(|_| closed())?.map_err(Error::native)?;
                let backend = mln_mtl::SurfaceBackend::new(layer.into_layer(), framebuffer_size, cap_frame_rate)?;
                Ok(SurfaceBackend::Metal(Rc::new(backend)))
            }
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => {
                let provider = Rc::new(crate::vulkan_surface::WindowSurface::new(proxy)?);
                let backend = mln_vulkan::SurfaceBackend::new(provider, framebuffer_size, cap_frame_rate)?;
                Ok(SurfaceBackend::Vulkan(Rc::new(backend)))
            }
            Kind::WebGpu => {
                let make_surface = |instance: &wgpu::Instance| make_surface(proxy, instance);
                let backend = mln_webgpu::SurfaceBackend::new(make_surface, framebuffer_size, cap_frame_rate)?;
                Ok(SurfaceBackend::WebGpu(Rc::new(backend)))
            }
            #[cfg(target_os = "linux")]
            Kind::OpenGl => {
                let (reply, window) = mpsc::channel();
                proxy.send_event(UserEvent::Window(reply)).map_err(|_| closed())?;
                let window = window.recv().map_err(|_| closed())?;
                let backend = mln_gl::SurfaceBackend::new(window, framebuffer_size, cap_frame_rate)?;
                Ok(SurfaceBackend::OpenGl(Rc::new(backend)))
            }
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(_) => Kind::Metal,
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(_) => Kind::Vulkan,
            SurfaceBackend::WebGpu(_) => Kind::WebGpu,
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(_) => Kind::OpenGl,
        }
    }

    /// The size of the frames, in physical pixels.
    pub(crate) fn framebuffer_size(&self) -> Size {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.get_framebuffer_size(),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.get_framebuffer_size(),
            SurfaceBackend::WebGpu(backend) => backend.get_framebuffer_size(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.get_framebuffer_size(),
        }
    }

    /// What renders and where, for the log.
    pub(crate) fn describe(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => {
                format!(
                    "Rendering with {} (Metal), surface format {:?}",
                    backend.get_device_name(),
                    backend.get_pixel_format()
                )
            }
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => {
                format!(
                    "Rendering with {} (Vulkan), surface format {:?}",
                    backend.get_device_name(),
                    backend.get_color_format()
                )
            }
            SurfaceBackend::WebGpu(backend) => {
                let info = backend.get_adapter_info();
                format!(
                    "Rendering with {} (wgpu {:?}, driver {} {}), surface format {:?}",
                    info.name,
                    info.backend,
                    info.driver,
                    info.driver_info,
                    backend.get_swap_chain_format()
                )
            }
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => {
                format!("Rendering with {} (OpenGL ES through EGL)", backend.get_renderer_name())
            }
        }
    }

    /// The GPU and how the map renders on it, for the status line.
    pub(crate) fn adapter(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => format!("{} (Metal)", backend.get_device_name()),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => format!("{} (Vulkan)", backend.get_device_name()),
            SurfaceBackend::WebGpu(backend) => {
                let info = backend.get_adapter_info();
                format!("{} (wgpu {:?})", info.name, info.backend)
            }
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => format!("{} (OpenGL ES)", backend.get_renderer_name()),
        }
    }

    /// Paints the controls' `paint` over each frame of the map.
    pub(crate) fn set_controls_overlay(&self, paint: Rc<RefCell<Paint>>) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.set_overlay(Some(crate::metal_overlay::metal_overlay(paint))),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.set_overlay(Some(crate::vulkan_overlay::vulkan_overlay(paint))),
            SurfaceBackend::WebGpu(backend) => backend.set_overlay(Some(crate::overlay::webgpu_overlay(paint))),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.set_overlay(Some(crate::gl_overlay::gl_overlay(paint))),
        }
    }

    pub(crate) fn renderer_backend(&self) -> Rc<dyn RendererBackend> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.clone(),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.clone(),
            SurfaceBackend::WebGpu(backend) => backend.clone(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.clone(),
        }
    }

    /// Takes the surface's next texture ahead of a frame, where the backend
    /// has it then; false when there is none to draw into right now.
    pub(crate) fn acquire_next_texture(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.acquire_next_texture(),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.acquire_next_texture(),
            SurfaceBackend::WebGpu(backend) => backend.acquire_next_texture(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.acquire_next_texture(),
        }
    }

    pub(crate) fn set_framebuffer_size(&self, size: Size) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.set_framebuffer_size(size),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.set_framebuffer_size(size),
            SurfaceBackend::WebGpu(backend) => backend.set_framebuffer_size(size),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.set_framebuffer_size(size),
        }
    }

    /// Gives the window up for the backend made next, which makes its own
    /// surface of it and couldn't otherwise: a window presents one
    /// swapchain at a time. On macOS there is nothing to give up: the
    /// backends present to the one layer.
    pub(crate) fn release_surface(&self) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(_) | SurfaceBackend::WebGpu(_) => {}
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.release_surface(),
            #[cfg(not(target_os = "macos"))]
            SurfaceBackend::WebGpu(backend) => backend.release_surface(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.release_surface(),
        }
    }

    /// Takes the window back after `release_surface`, when no other backend
    /// took it. OpenGL ES does so by itself with its next frame.
    pub(crate) fn restore_surface(&self, proxy: &EventLoopProxy<UserEvent>) -> Result<(), Error> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(_) => Ok(()),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => {
                backend.restore_surface();
                Ok(())
            }
            SurfaceBackend::WebGpu(backend) => {
                if backend.has_surface() {
                    return Ok(());
                }
                let instance = backend
                    .webgpu_backend()
                    .get_instance()
                    .ok_or_else(|| Error::invalid_state("The backend has no instance"))?;
                backend.set_surface(make_surface(proxy, &instance)?, backend.get_framebuffer_size())
            }
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(_) => Ok(()),
        }
    }

    /// Copy a presented frame (see `take_readback`).
    pub(crate) fn request_readback(&self) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.request_readback(),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.request_readback(),
            SurfaceBackend::WebGpu(backend) => backend.request_readback(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.request_readback(),
        }
    }

    /// The frame copied after `request_readback`, once presented.
    pub(crate) fn take_readback(&self) -> Option<PremultipliedImage> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.take_readback(),
            #[cfg(not(target_vendor = "apple"))]
            SurfaceBackend::Vulkan(backend) => backend.take_readback(),
            SurfaceBackend::WebGpu(backend) => backend.take_readback(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.take_readback(),
        }
    }
}
