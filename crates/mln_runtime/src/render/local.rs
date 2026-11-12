//! A render target built on the thread that renders: a [`RenderTarget`]'s
//! device and surface made into the backend's objects, which belong to that
//! thread.
//!
//! [`RenderTarget`]: super::RenderTarget

// The browser build has no backend for sessions.
#![cfg_attr(target_family = "wasm", allow(unused_macros, unused_variables))]

use std::rc::Rc;

use mln_gfx::{FrameTarget, RendererBackend};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::target::{Device, DeviceOptions, Surface, SurfaceKind};

/// Frames read back: an offscreen target.
trait Offscreen: FrameTarget {
    fn resize(&self, size: Size);
    fn read_image(&self) -> Option<PremultipliedImage>;
}

/// Frames presented: a surface target.
trait Presenting: FrameTarget {
    fn resize(&self, size: Size);
}

macro_rules! offscreen {
    ($type:ty) => {
        impl Offscreen for $type {
            fn resize(&self, size: Size) {
                <$type>::resize(self, size);
            }
            fn read_image(&self) -> Option<PremultipliedImage> {
                <$type>::read_image(self)
            }
        }
    };
}

macro_rules! presenting {
    ($type:ty) => {
        impl Presenting for $type {
            fn resize(&self, size: Size) {
                <$type>::resize(self, size);
            }
        }
    };
}

#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
offscreen!(mln_webgpu::OffscreenTarget);
#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
presenting!(mln_webgpu::SurfaceTarget);
#[cfg(all(feature = "metal", target_vendor = "apple"))]
offscreen!(mln_mtl::OffscreenTarget);
#[cfg(all(feature = "metal", target_vendor = "apple"))]
presenting!(mln_mtl::SurfaceTarget);
#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
offscreen!(mln_vulkan::OffscreenTarget);
#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
presenting!(mln_vulkan::SurfaceTarget);
#[cfg(all(feature = "gl", not(target_family = "wasm")))]
offscreen!(mln_gl::OffscreenTarget);
#[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
presenting!(mln_gl::SurfaceTarget);

/// A backend, by its type.
#[derive(Clone)]
enum Backend {
    #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
    WebGpu(Rc<mln_webgpu::RendererBackend>),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(Rc<mln_mtl::RendererBackend>),
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(Rc<mln_vulkan::RendererBackend>),
    #[cfg(all(feature = "gl", not(target_family = "wasm")))]
    Gl(Rc<mln_gl::RendererBackend>),
}

impl Backend {
    fn as_dyn(&self) -> Rc<dyn RendererBackend> {
        match *self {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            Backend::WebGpu(ref backend) => backend.clone(),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Backend::Metal(ref backend) => backend.clone(),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Backend::Vulkan(ref backend) => backend.clone(),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Backend::Gl(ref backend) => backend.clone(),
        }
    }

    /// A backend on `device`, for offscreen targets and the surfaces that
    /// the device's backend makes on it.
    fn new(device: &Device) -> Result<Backend, Error> {
        match *device {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            Device::WebGpu(ref device) => Ok(Backend::WebGpu(Rc::new(mln_webgpu::RendererBackend::new(device)))),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(ref device) => Ok(Backend::Metal(Rc::new(mln_mtl::RendererBackend::new(device)))),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(ref device) => Ok(Backend::Vulkan(Rc::new(mln_vulkan::RendererBackend::new(device)?))),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Device::Gl(ref device) => Ok(Backend::Gl(Rc::new(mln_gl::RendererBackend::new(device)?))),
        }
    }

    /// An offscreen target of `size` pixels on the backend.
    fn offscreen(&self, size: Size) -> Box<dyn Offscreen> {
        match *self {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            Backend::WebGpu(ref backend) => Box::new(mln_webgpu::OffscreenTarget::new(backend.clone(), size)),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Backend::Metal(ref backend) => Box::new(mln_mtl::OffscreenTarget::new(backend.clone(), size)),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Backend::Vulkan(ref backend) => Box::new(mln_vulkan::OffscreenTarget::new(backend.clone(), size)),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Backend::Gl(ref backend) => Box::new(mln_gl::OffscreenTarget::new(backend.clone(), size)),
        }
    }

    /// A target on the backend that presents to `surface`, at `size`
    /// pixels. Fails with `Unsupported` when only a backend of the target's
    /// own can present to it: see [`surface_backend`].
    fn surface(&self, surface: Surface, size: Size) -> Result<Box<dyn Presenting>, Error> {
        #[allow(unreachable_patterns)]
        match (self, surface.kind) {
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            (Backend::WebGpu(backend), kind) => {
                let target = webgpu_surface(backend, kind)?;
                let options = mln_webgpu::SurfaceOptions::default();
                Ok(Box::new(mln_webgpu::SurfaceTarget::new(backend.clone(), target, size, options)?))
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            (Backend::Metal(backend), SurfaceKind::MetalLayer(layer)) => {
                Ok(Box::new(mln_mtl::SurfaceTarget::new(backend, layer, size, true)))
            }
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (Backend::Vulkan(backend), SurfaceKind::Window(window)) => {
                Ok(Box::new(mln_vulkan::SurfaceTarget::new(backend.clone(), window, size, true)?))
            }
            _ => {
                let _ = size;
                Err(Error::unsupported("the session's backend presents to no surface of this kind"))
            }
        }
    }
}

/// A wgpu surface of `kind`, made with the instance of `backend`'s device
/// where the host didn't make it.
#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
fn webgpu_surface(backend: &mln_webgpu::RendererBackend, kind: SurfaceKind) -> Result<wgpu::Surface<'static>, Error> {
    let instance = || {
        backend
            .device()
            .instance()
            .ok_or_else(|| Error::invalid_argument("the session's device has no instance to make a surface with"))
    };
    let made = match kind {
        SurfaceKind::Wgpu(surface) => return Ok(surface),
        SurfaceKind::Window(window) => instance()?.create_surface(window),
        #[cfg(all(feature = "metal", target_vendor = "apple"))]
        SurfaceKind::MetalLayer(_) => return Err(Error::unsupported("a Metal layer presents on a Metal device")),
    };
    made.map_err(|error| Error::native(format!("can't make a surface on the window: {error}")))
}

/// A backend of the target's own on `device` and a target that presents to
/// `surface`, for the backends whose context or device is made for the
/// window: Vulkan's, whose instance and device take the window's surface
/// extensions, and OpenGL ES's, whose context is of the window's display.
fn surface_backend(device: &Device, surface: Surface, size: Size) -> Result<(Backend, Box<dyn Presenting>), Error> {
    #[allow(unreachable_patterns)]
    match (device, &surface.kind) {
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), SurfaceKind::Window(window)) => {
            let (backend, target) = mln_vulkan::SurfaceTarget::with_backend(device, window.clone(), size, true)?;
            Ok((Backend::Vulkan(backend), Box::new(target)))
        }
        #[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
        (Device::Gl(_), SurfaceKind::Window(window)) => {
            let (backend, target) = mln_gl::SurfaceTarget::with_backend(window.clone(), size, true)?;
            Ok((Backend::Gl(backend), Box::new(target)))
        }
        _ => {
            let backend = Backend::new(device)?;
            let target = backend.surface(surface, size)?;
            Ok((backend, target))
        }
    }
}

enum Frames {
    Offscreen(Box<dyn Offscreen>),
    /// `None` once the surface is released.
    Presenting(Option<Box<dyn Presenting>>),
}

/// A render target built on this thread: the backend that a renderer draws
/// with, on the target's device, and the frames it draws into.
pub struct LocalTarget {
    device: Device,
    backend: Backend,
    frames: Frames,
}

impl LocalTarget {
    /// An offscreen target of `size` pixels on `device`, or on a device that
    /// [`Device::new`] makes when that is `None`.
    ///
    /// Fails as [`Device::new`] does, and with the backend's error when it
    /// can't render on the device.
    pub fn offscreen(device: Option<Device>, size: Size) -> Result<LocalTarget, Error> {
        let device = match device {
            Some(device) => device,
            None => Device::new(DeviceOptions::default())?,
        };
        let backend = Backend::new(&device)?;
        let frames = Frames::Offscreen(backend.offscreen(size));
        Ok(LocalTarget { device, backend, frames })
    }

    /// A target of `size` pixels that presents to `surface`, on `device` or,
    /// when that is `None`, on the device the surface's kind takes
    /// ([`RenderTarget::Surface`](super::RenderTarget::Surface)).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// surface that the host made without the host's device, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the device can't
    /// present to the surface, and with the error of the device or the
    /// window's surface.
    pub fn surface(surface: Surface, device: Option<Device>, size: Size) -> Result<LocalTarget, Error> {
        let device = match device {
            Some(device) => device,
            None => default_surface_device(&surface)?,
        };
        let (backend, target) = surface_backend(&device, surface, size)?;
        Ok(LocalTarget { device, backend, frames: Frames::Presenting(Some(target)) })
    }

    /// The device the target renders on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The backend that renderers drawing into the target draw with.
    pub fn backend(&self) -> Rc<dyn RendererBackend> {
        self.backend.as_dyn()
    }

    /// The frames to draw into, or `None` while a surface target has no
    /// surface.
    pub fn frames(&self) -> Option<&dyn FrameTarget> {
        match &self.frames {
            Frames::Offscreen(target) => Some(&**target),
            Frames::Presenting(target) => target.as_deref().map(|target| target as &dyn FrameTarget),
        }
    }

    /// Whether frames are read back rather than presented.
    pub fn is_offscreen(&self) -> bool {
        matches!(self.frames, Frames::Offscreen(_))
    }

    /// Draws the next frame at `size` pixels.
    pub fn resize(&self, size: Size) {
        match &self.frames {
            Frames::Offscreen(target) => target.resize(size),
            Frames::Presenting(Some(target)) => target.resize(size),
            Frames::Presenting(None) => {}
        }
    }

    /// The last frame's pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that presents its frames, and with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) before a frame.
    pub fn read_image(&self) -> Result<PremultipliedImage, Error> {
        match &self.frames {
            Frames::Offscreen(target) => target
                .read_image()
                .ok_or_else(|| Error::invalid_state("the target has no frame since it was made or resized")),
            Frames::Presenting(_) => Err(Error::unsupported("a surface session can't read its frames back")),
        }
    }

    /// Lets go of a surface target's surface. The backend stays.
    pub fn release_surface(&mut self) {
        if let Frames::Presenting(target) = &mut self.frames {
            target.take();
        }
    }

    /// Presents to `surface` at `size` pixels from now on, in place of the
    /// surface the target has, if any: on the same backend where it can
    /// present to `surface`, and on a new one of the device otherwise, which
    /// the target then returns true for. A renderer on the old backend has
    /// to be made again on the new one.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for an
    /// offscreen target or when the device can't present to `surface`, and
    /// as [`surface`](Self::surface) does. The target then keeps what it
    /// had.
    pub fn set_surface(&mut self, surface: Surface, size: Size) -> Result<bool, Error> {
        if self.is_offscreen() {
            return Err(Error::unsupported("an offscreen target keeps its frames"));
        }
        let retry = surface.clone_window();
        let (new_backend, target) = match self.backend.surface(surface, size) {
            Ok(target) => (false, target),
            Err(error) if error.kind == mln_util::ErrorKind::Unsupported => {
                let surface = retry.ok_or(error)?;
                let (backend, target) = surface_backend(&self.device, surface, size)?;
                self.backend = backend;
                (true, target)
            }
            Err(error) => return Err(error),
        };
        self.frames = Frames::Presenting(Some(target));
        Ok(new_backend)
    }
}

/// The device a surface renders on without the host's: the native one where
/// its backend takes the surface's kind.
fn default_surface_device(surface: &Surface) -> Result<Device, Error> {
    match surface.kind {
        SurfaceKind::Window(_) if cfg!(target_vendor = "apple") => {
            Device::with_backend(super::target::BackendType::WebGPU, DeviceOptions::default())
        }
        SurfaceKind::Window(_) => Device::new(DeviceOptions::default()),
        #[cfg(all(feature = "metal", target_vendor = "apple"))]
        SurfaceKind::MetalLayer(_) => Device::with_backend(super::target::BackendType::Metal, DeviceOptions::default()),
        #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
        SurfaceKind::Wgpu(_) => Err(Error::invalid_argument("a surface that the host made needs the host's device")),
    }
}
