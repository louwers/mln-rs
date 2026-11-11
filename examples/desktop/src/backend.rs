// The window's renderer backend: Metal by default on macOS, WebGPU (through
// wgpu) everywhere, picked with --backend and switched in the controls.
// C++'s GLFW app builds one backend and ignores the flag. On macOS both
// present to the window's one CAMetalLayer (metal_layer.rs).

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
}

impl Kind {
    /// The backend of a --backend value.
    pub(crate) fn parse(name: &str) -> Result<Kind, String> {
        match name {
            #[cfg(target_os = "macos")]
            "metal" => Ok(Kind::Metal),
            "webgpu" => Ok(Kind::WebGpu),
            _ => Err(format!("Unknown backend '{name}' (available: {})", Kind::NAMES)),
        }
    }

    #[cfg(target_os = "macos")]
    const NAMES: &str = "metal, webgpu";
    #[cfg(not(target_os = "macos"))]
    const NAMES: &str = "webgpu";

    /// The backends this platform has.
    pub(crate) const ALL: &[Kind] = &[
        #[cfg(target_os = "macos")]
        Kind::Metal,
        Kind::WebGpu,
    ];

    /// The backend's name in the controls.
    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Kind::Metal => "Metal",
            Kind::WebGpu => "WebGPU",
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
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(_) => Kind::Metal,
            SurfaceBackend::WebGpu(_) => Kind::WebGpu,
        }
    }

    /// The size of the frames, in physical pixels.
    pub(crate) fn framebuffer_size(&self) -> Size {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.get_framebuffer_size(),
            SurfaceBackend::WebGpu(backend) => backend.get_framebuffer_size(),
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
        }
    }

    /// Paints the controls' `paint` over each frame of the map.
    pub(crate) fn set_controls_overlay(&self, paint: Rc<RefCell<Paint>>) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.set_overlay(Some(crate::metal_overlay::metal_overlay(paint))),
            SurfaceBackend::WebGpu(backend) => backend.set_overlay(Some(crate::overlay::webgpu_overlay(paint))),
        }
    }

    pub(crate) fn renderer_backend(&self) -> Rc<dyn RendererBackend> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.clone(),
            SurfaceBackend::WebGpu(backend) => backend.clone(),
        }
    }

    /// Takes the surface's next texture ahead of a frame; false when there
    /// is none right now.
    pub(crate) fn acquire_next_texture(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.acquire_next_texture(),
            SurfaceBackend::WebGpu(backend) => backend.acquire_next_texture(),
        }
    }

    pub(crate) fn set_framebuffer_size(&self, size: Size) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.set_framebuffer_size(size),
            SurfaceBackend::WebGpu(backend) => backend.set_framebuffer_size(size),
        }
    }

    /// Copy a presented frame (see `take_readback`).
    pub(crate) fn request_readback(&self) {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.request_readback(),
            SurfaceBackend::WebGpu(backend) => backend.request_readback(),
        }
    }

    /// The frame copied after `request_readback`, once presented.
    pub(crate) fn take_readback(&self) -> Option<PremultipliedImage> {
        match self {
            #[cfg(target_os = "macos")]
            SurfaceBackend::Metal(backend) => backend.take_readback(),
            SurfaceBackend::WebGpu(backend) => backend.take_readback(),
        }
    }
}
