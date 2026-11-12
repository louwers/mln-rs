// The window's renderer backend and the target it presents with: Metal by
// default on macOS, WebGPU (through wgpu, the default where there is no
// Metal) everywhere, Vulkan elsewhere than on Apple's platforms and OpenGL ES
// on Linux, picked with --backend and switched in the controls. C++'s GLFW
// app builds one backend and ignores the flag. On macOS both present to the
// window's one CAMetalLayer (metal_layer.rs). Elsewhere each makes its own
// surface of the window, which takes one backend's frames at a time: the
// backend that goes gives it up before the next one takes it.
//
// OpenGL ES is Linux's only: EGL is no system library on Windows, and macOS
// has neither it nor OpenGL ES.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use mln_gfx::{BackendScope, FrameTarget, RendererBackend, ScopeType};
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

/// The backend presenting to the window, and its target.
pub(crate) enum WindowTarget {
    #[cfg(target_os = "macos")]
    Metal(Rc<mln_mtl::RendererBackend>, mln_mtl::SurfaceTarget),
    #[cfg(not(target_vendor = "apple"))]
    Vulkan(Rc<mln_vulkan::RendererBackend>, mln_vulkan::SurfaceTarget),
    WebGpu(Rc<mln_webgpu::RendererBackend>, mln_webgpu::SurfaceTarget),
    #[cfg(target_os = "linux")]
    OpenGl(Rc<mln_gl::RendererBackend>, mln_gl::SurfaceTarget, String),
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

/// The window, from the window's thread.
#[cfg(not(target_vendor = "apple"))]
fn window(proxy: &EventLoopProxy<UserEvent>) -> Result<std::sync::Arc<winit::window::Window>, Error> {
    let (reply, window) = mpsc::channel();
    proxy.send_event(UserEvent::Window(reply)).map_err(|_| closed())?;
    window.recv().map_err(|_| closed())
}

impl WindowTarget {
    /// A backend of `kind` on a device of its own, and a target that
    /// presents to the window, whose surface or layer the window's thread
    /// makes when asked through `proxy`.
    pub(crate) fn new(
        kind: Kind,
        proxy: &EventLoopProxy<UserEvent>,
        framebuffer_size: Size,
        cap_frame_rate: bool,
    ) -> Result<WindowTarget, Error> {
        match kind {
            #[cfg(target_os = "macos")]
            Kind::Metal => {
                let (reply, layer) = mpsc::channel();
                proxy.send_event(UserEvent::MakeLayer(reply)).map_err(|_| closed())?;
                let layer = layer.recv().map_err(|_| closed())?.map_err(Error::native)?;
                let backend = Rc::new(mln_mtl::RendererBackend::new(&mln_mtl::Device::new(None)?));
                let layer = mln_mtl::Layer::new(layer.into_layer());
                let target = mln_mtl::SurfaceTarget::new(&backend, layer, framebuffer_size, cap_frame_rate);
                Ok(WindowTarget::Metal(backend, target))
            }
            #[cfg(not(target_vendor = "apple"))]
            Kind::Vulkan => {
                let device = mln_vulkan::Device::new(None)?;
                let (backend, target) =
                    mln_vulkan::SurfaceTarget::with_backend(&device, window(proxy)?, framebuffer_size, cap_frame_rate)?;
                Ok(WindowTarget::Vulkan(backend, target))
            }
            Kind::WebGpu => {
                let device = mln_webgpu::Device::new(None)?;
                let instance = device.instance().cloned().ok_or_else(|| Error::native("The device has no instance"))?;
                let backend = Rc::new(mln_webgpu::RendererBackend::new(&device));
                let options = mln_webgpu::SurfaceOptions { cap_frame_rate, ..Default::default() };
                let surface = make_surface(proxy, &instance)?;
                let target = mln_webgpu::SurfaceTarget::new(backend.clone(), surface, framebuffer_size, options)?;
                Ok(WindowTarget::WebGpu(backend, target))
            }
            #[cfg(target_os = "linux")]
            Kind::OpenGl => {
                let (backend, target) =
                    mln_gl::SurfaceTarget::with_backend(window(proxy)?, framebuffer_size, cap_frame_rate)?;
                let name = {
                    let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
                    backend.get_renderer_name()
                };
                Ok(WindowTarget::OpenGl(backend, target, name))
            }
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(..) => Kind::Metal,
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(..) => Kind::Vulkan,
            WindowTarget::WebGpu(..) => Kind::WebGpu,
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(..) => Kind::OpenGl,
        }
    }

    /// The size of the frames, in physical pixels.
    pub(crate) fn framebuffer_size(&self) -> Size {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target.size(),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target.size(),
            WindowTarget::WebGpu(_, target) => target.size(),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target.size(),
        }
    }

    /// What renders and where, for the log.
    pub(crate) fn describe(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(backend, target) => {
                format!(
                    "Rendering with {} (Metal), surface format {:?}",
                    backend.device().name(),
                    target.get_pixel_format()
                )
            }
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(backend, target) => {
                format!(
                    "Rendering with {} (Vulkan), surface format {:?}",
                    backend.get_device_name(),
                    target.get_color_format()
                )
            }
            WindowTarget::WebGpu(backend, target) => {
                let info = backend.device().adapter_info();
                format!(
                    "Rendering with {} (wgpu {:?}, driver {} {}), surface format {:?}",
                    info.name,
                    info.backend,
                    info.driver,
                    info.driver_info,
                    target.format()
                )
            }
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, _, name) => format!("Rendering with {name} (OpenGL ES through EGL)"),
        }
    }

    /// The GPU and how the map renders on it, for the status line.
    pub(crate) fn adapter(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(backend, _) => format!("{} (Metal)", backend.device().name()),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(backend, _) => format!("{} (Vulkan)", backend.get_device_name()),
            WindowTarget::WebGpu(backend, _) => {
                let info = backend.device().adapter_info();
                format!("{} (wgpu {:?})", info.name, info.backend)
            }
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, _, name) => format!("{name} (OpenGL ES)"),
        }
    }

    /// Paints the controls' `paint` over each frame of the map.
    pub(crate) fn set_controls_overlay(&self, paint: Rc<RefCell<Paint>>) {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target.set_overlay(Some(crate::metal_overlay::metal_overlay(paint))),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target.set_overlay(Some(crate::vulkan_overlay::vulkan_overlay(paint))),
            WindowTarget::WebGpu(_, target) => target.set_overlay(Some(crate::overlay::webgpu_overlay(paint))),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target.set_overlay(Some(crate::gl_overlay::gl_overlay(paint))),
        }
    }

    pub(crate) fn renderer_backend(&self) -> Rc<dyn RendererBackend> {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(backend, _) => backend.clone(),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(backend, _) => backend.clone(),
            WindowTarget::WebGpu(backend, _) => backend.clone(),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(backend, ..) => backend.clone(),
        }
    }

    /// The frames the renderer draws into.
    pub(crate) fn frames(&self) -> &dyn FrameTarget {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target,
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target,
            WindowTarget::WebGpu(_, target) => target,
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target,
        }
    }

    /// Takes the surface's next texture ahead of a frame, where the backend
    /// has it then; false when there is none to draw into right now.
    pub(crate) fn acquire_next_texture(&self) -> bool {
        let backend = self.renderer_backend();
        let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
        self.frames().begin_frame().is_some()
    }

    pub(crate) fn set_framebuffer_size(&self, size: Size) {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target.resize(size),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target.resize(size),
            WindowTarget::WebGpu(_, target) => target.resize(size),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target.resize(size),
        }
    }

    /// Copy a presented frame (see `take_readback`).
    pub(crate) fn request_readback(&self) {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target.request_readback(),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target.request_readback(),
            WindowTarget::WebGpu(_, target) => target.request_readback(),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target.request_readback(),
        }
    }

    /// The frame copied after `request_readback`, once presented.
    pub(crate) fn take_readback(&self) -> Option<PremultipliedImage> {
        match self {
            #[cfg(target_os = "macos")]
            WindowTarget::Metal(_, target) => target.take_readback(),
            #[cfg(not(target_vendor = "apple"))]
            WindowTarget::Vulkan(_, target) => target.take_readback(),
            WindowTarget::WebGpu(_, target) => target.take_readback(),
            #[cfg(target_os = "linux")]
            WindowTarget::OpenGl(_, target, _) => target.take_readback(),
        }
    }
}
