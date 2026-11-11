// The window's renderer backend: Metal by default on macOS, WebGPU (through
// wgpu) everywhere, and OpenGL ES on Linux, picked with --backend and
// switched in the controls. C++'s GLFW app builds one backend and ignores the
// flag. On macOS both present to the window's one CAMetalLayer
// (metal_layer.rs). On Linux OpenGL ES presents through EGL to the window's
// Wayland surface or X11 window, which takes one API's frames at a time: the
// backend that goes gives it up before the next one takes it.
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
            "webgpu" => Ok(Kind::WebGpu),
            #[cfg(target_os = "linux")]
            "opengl" => Ok(Kind::OpenGl),
            _ => Err(format!("Unknown backend '{name}' (available: {})", Kind::NAMES)),
        }
    }

    #[cfg(target_os = "macos")]
    const NAMES: &str = "metal, webgpu";
    #[cfg(target_os = "linux")]
    const NAMES: &str = "webgpu, opengl";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    const NAMES: &str = "webgpu";

    /// The backends this platform has.
    pub(crate) const ALL: &[Kind] = &[
        #[cfg(target_os = "macos")]
        Kind::Metal,
        Kind::WebGpu,
        #[cfg(target_os = "linux")]
        Kind::OpenGl,
    ];

    /// The backend's name in the controls.
    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => "Metal",
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
    WebGpu(Rc<mln_webgpu::SurfaceBackend>),
    #[cfg(target_os = "linux")]
    OpenGl(Rc<mln_gl::SurfaceBackend>),
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
        let closed = || Error::cancelled("The window closed before it had a surface");
        match kind {
            #[cfg(target_os = "macos")]
            Kind::Metal => {
                let (reply, layer) = mpsc::channel();
                proxy.send_event(UserEvent::MakeLayer(reply)).map_err(|_| closed())?;
                let layer = layer.recv().map_err(|_| closed())?.map_err(Error::native)?;
                let backend = mln_mtl::SurfaceBackend::new(layer.into_layer(), framebuffer_size, cap_frame_rate)?;
                Ok(SurfaceBackend::Metal(Rc::new(backend)))
            }
            Kind::WebGpu => {
                let proxy = proxy.clone();
                let make_surface = move |instance: &wgpu::Instance| {
                    let (reply, surface) = mpsc::channel();
                    proxy.send_event(UserEvent::MakeSurface(instance.clone(), reply)).map_err(|_| closed())?;
                    surface.recv().map_err(|_| closed())?.map_err(Error::native)
                };
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
            SurfaceBackend::WebGpu(backend) => backend.set_overlay(Some(crate::overlay::webgpu_overlay(paint))),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.set_overlay(Some(crate::gl_overlay::gl_overlay(paint))),
        }
    }

    pub(crate) fn renderer_backend(&self) -> Rc<dyn RendererBackend> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.clone(),
            SurfaceBackend::WebGpu(backend) => backend.clone(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.clone(),
        }
    }

    /// Gives the window up for the backend made next, which couldn't take it
    /// otherwise. This one takes it back with its next frame.
    pub(crate) fn release_surface(&self) {
        #[cfg(target_os = "linux")]
        if let SurfaceBackend::OpenGl(backend) = self {
            backend.release_surface();
        }
    }

    /// Takes the surface's next texture ahead of a frame; false when there
    /// is none right now.
    pub(crate) fn acquire_next_texture(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.acquire_next_texture(),
            SurfaceBackend::WebGpu(backend) => backend.acquire_next_texture(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.acquire_next_texture(),
        }
    }

    pub(crate) fn set_framebuffer_size(&self, size: Size) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.set_framebuffer_size(size),
            SurfaceBackend::WebGpu(backend) => backend.set_framebuffer_size(size),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.set_framebuffer_size(size),
        }
    }

    /// Copy a presented frame (see `take_readback`).
    pub(crate) fn request_readback(&self) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.request_readback(),
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
            SurfaceBackend::WebGpu(backend) => backend.take_readback(),
            #[cfg(target_os = "linux")]
            SurfaceBackend::OpenGl(backend) => backend.take_readback(),
        }
    }
}
