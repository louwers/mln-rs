//! A render target built on the thread that renders: a [`RenderTarget`]'s
//! device and surface made into the backend's objects, which belong to that
//! thread.
//!
//! [`RenderTarget`]: super::RenderTarget

// The browser build has no backend for sessions.
#![cfg_attr(target_family = "wasm", allow(unused_macros, unused_variables, unused_imports))]

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::{ConsumerSync, ExportKind, ExportedImage, ExportedRing, FrameTarget, RendererBackend, SyncKind, Window};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::target::{
    BorrowedRing, Device, DeviceOptions, FrameTexture, FrameTextureKind, RingKind, Surface, SurfaceKind,
};

/// Frames read back: an offscreen target.
trait Offscreen: FrameTarget {
    fn resize(&self, size: Size);
    fn read_image(&self) -> Option<PremultipliedImage>;
}

/// Frames presented: a surface target.
trait Presenting: FrameTarget {
    fn resize(&self, size: Size);
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
impl Borrowed for mln_mtl::BorrowedTarget {
    fn select(&self, slot: usize) {
        mln_mtl::BorrowedTarget::select(self, slot);
    }
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_mtl::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&self, slot: usize) {
        mln_mtl::BorrowedTarget::discard(self, slot);
    }
    fn replace(&self, ring: BorrowedRing) -> Result<Pending, Error> {
        #[allow(irrefutable_let_patterns)]
        let RingKind::Metal(textures) = ring.kind else {
            return Err(Error::unsupported("a ring of Metal textures takes Metal textures only"));
        };
        mln_mtl::BorrowedTarget::replace(self, textures).map(pending)
    }
    fn take_pending(&self) -> Pending {
        pending(mln_mtl::BorrowedTarget::take_pending(self))
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
impl PendingReads for mln_mtl::PendingReads {
    fn poll(&mut self) -> bool {
        mln_mtl::PendingReads::poll(self)
    }
}

/// Frames given back to the host: a ring of its textures.
trait Borrowed: FrameTarget {
    fn select(&self, slot: usize);
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error>;
    fn discard(&self, slot: usize);
    /// Renders into `ring` from now on, and returns the host's reads of the
    /// textures that it let go of.
    fn replace(&self, ring: BorrowedRing) -> Result<Pending, Error>;
    /// Takes the host's reads of the textures that they still wait for, for
    /// the ring to wait for before it lets go of them. A target that waits
    /// for the host's reads as it takes each texture back has none.
    fn take_pending(&self) -> Pending {
        None
    }
}

/// The host's reads of textures that a ring let go of, which the session
/// waits for before the release or replacement completes. Dropping it stops
/// waiting.
pub(crate) trait PendingReads {
    /// Whether every read ended, which it checks without blocking.
    fn poll(&mut self) -> bool;
}

/// The host's reads that a ring waits for, if any.
pub(crate) type Pending = Option<Box<dyn PendingReads>>;

/// `reads`, unless they ended already.
#[cfg(any(
    all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))),
    all(feature = "metal", target_vendor = "apple")
))]
fn pending(mut reads: impl PendingReads + 'static) -> Pending {
    (!reads.poll()).then(|| Box::new(reads) as Box<dyn PendingReads>)
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
impl Borrowed for mln_vulkan::BorrowedTarget {
    fn select(&self, slot: usize) {
        mln_vulkan::BorrowedTarget::select(self, slot);
    }
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_vulkan::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&self, slot: usize) {
        mln_vulkan::BorrowedTarget::discard(self, slot);
    }
    fn replace(&self, ring: BorrowedRing) -> Result<Pending, Error> {
        #[allow(irrefutable_let_patterns)]
        let RingKind::Vulkan(images) = ring.kind else {
            return Err(Error::unsupported("a ring of Vulkan images takes Vulkan images only"));
        };
        mln_vulkan::BorrowedTarget::replace(self, images).map(pending)
    }
    fn take_pending(&self) -> Pending {
        pending(mln_vulkan::BorrowedTarget::take_pending(self))
    }
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
impl PendingReads for mln_vulkan::PendingReads {
    fn poll(&mut self) -> bool {
        mln_vulkan::PendingReads::poll(self)
    }
}

/// Frames queued for the host: a ring of the target's own images, which it
/// exports.
trait Exported: FrameTarget {
    fn select(&self, slot: usize);
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error>;
    fn discard(&self, slot: usize);
    fn resize(&self, size: Size, held: &[usize]) -> Result<bool, Error>;
    fn size(&self) -> Size;
    fn image(&self, slot: usize) -> Option<(Size, ExportedImage)>;
}

macro_rules! exported {
    ($type:ty) => {
        impl Exported for $type {
            fn select(&self, slot: usize) {
                <$type>::select(self, slot);
            }
            fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
                <$type>::release(self, slot, consumer)
            }
            fn discard(&self, slot: usize) {
                <$type>::discard(self, slot);
            }
            fn resize(&self, size: Size, held: &[usize]) -> Result<bool, Error> {
                <$type>::resize(self, size, held)
            }
            fn size(&self) -> Size {
                <$type>::size(self)
            }
            fn image(&self, slot: usize) -> Option<(Size, ExportedImage)> {
                <$type>::image(self, slot)
            }
        }
    };
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
exported!(mln_vulkan::ExportedTarget);
#[cfg(all(feature = "metal", target_vendor = "apple"))]
exported!(mln_mtl::ExportedTarget);
#[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
exported!(mln_gl::ExportedTarget);

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
#[cfg(all(feature = "gl", unix, not(target_vendor = "apple")))]
presenting!(mln_gl::SurfaceTarget);
#[cfg(all(feature = "gl", not(target_family = "wasm")))]
presenting!(mln_gl::HostSurfaceTarget);

#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
impl Borrowed for mln_webgpu::BorrowedTarget {
    fn select(&self, slot: usize) {
        mln_webgpu::BorrowedTarget::select(self, slot);
    }
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_webgpu::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&self, slot: usize) {
        mln_webgpu::BorrowedTarget::discard(self, slot);
    }
    fn replace(&self, ring: BorrowedRing) -> Result<Pending, Error> {
        #[allow(irrefutable_let_patterns)]
        let RingKind::WebGpu(textures) = ring.kind else {
            return Err(Error::unsupported("a ring of WebGPU textures takes WebGPU textures only"));
        };
        mln_webgpu::BorrowedTarget::replace(self, textures).map(|()| None)
    }
}

#[cfg(all(feature = "gl", not(target_family = "wasm")))]
impl Borrowed for mln_gl::BorrowedTarget {
    fn select(&self, slot: usize) {
        mln_gl::BorrowedTarget::select(self, slot);
    }
    fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_gl::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&self, slot: usize) {
        mln_gl::BorrowedTarget::discard(self, slot);
    }
    fn replace(&self, ring: BorrowedRing) -> Result<Pending, Error> {
        #[allow(irrefutable_let_patterns)]
        let RingKind::Gl(textures) = ring.kind else {
            return Err(Error::unsupported("a ring of OpenGL textures takes OpenGL textures only"));
        };
        mln_gl::BorrowedTarget::replace(self, textures).map(|()| None)
    }
}

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

    /// A target on the backend that renders into `ring`, handing frames over
    /// with sync of the kind `producer`.
    fn borrowed(&self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed>, Error> {
        #[allow(unreachable_patterns)]
        match (self, ring.kind) {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (Backend::Vulkan(backend), RingKind::Vulkan(images)) => {
                Ok(Box::new(mln_vulkan::BorrowedTarget::new(backend.clone(), images, producer)?))
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            (Backend::Metal(backend), RingKind::Metal(textures)) => {
                Ok(Box::new(mln_mtl::BorrowedTarget::new(backend.clone(), textures, producer)?))
            }
            #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
            (Backend::WebGpu(backend), RingKind::WebGpu(textures)) => {
                Ok(Box::new(mln_webgpu::BorrowedTarget::new(backend.clone(), textures, producer)?))
            }
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            (Backend::Gl(backend), RingKind::Gl(textures)) => {
                Ok(Box::new(mln_gl::BorrowedTarget::new(backend.clone(), textures, producer)?))
            }
            _ => {
                let _ = producer;
                Err(Error::unsupported("the session's backend renders into no ring of this kind"))
            }
        }
    }

    /// A target on the backend that renders into a ring of `ring.depth`
    /// images of `size` pixels, which it exports as `kind`, and hands frames
    /// over with sync of the kind `producer`.
    fn exported(
        &self,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported>, Error> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Backend::Vulkan(backend) => {
                Ok(Box::new(mln_vulkan::ExportedTarget::new(backend.clone(), ring, kind, size, producer)?))
            }
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Backend::Metal(backend) => {
                Ok(Box::new(mln_mtl::ExportedTarget::new(backend.clone(), ring, kind, size, producer)?))
            }
            #[cfg(all(feature = "gl", unix, not(target_vendor = "apple"), not(target_os = "android")))]
            Backend::Gl(backend) => {
                let Device::Gl(device) = device else {
                    return Err(Error::unsupported("an OpenGL backend renders on an OpenGL device"));
                };
                Ok(Box::new(mln_gl::ExportedTarget::new(backend.clone(), device, ring, kind, size, producer)?))
            }
            _ => {
                let _ = (device, ring, kind, size, producer);
                Err(Error::unsupported("the session's backend exports no ring"))
            }
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
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            (Backend::Vulkan(backend), SurfaceKind::Vulkan(surface)) => {
                Ok(Box::new(mln_vulkan::SurfaceTarget::on_host_surface(backend.clone(), surface, size, true)?))
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
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        SurfaceKind::Vulkan(_) => return Err(Error::unsupported("a Vulkan surface presents on a Vulkan device")),
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        SurfaceKind::Gl(_) => return Err(Error::unsupported("an EGL surface presents on an OpenGL device")),
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
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), SurfaceKind::Vulkan(_)) => {
            let backend = Backend::Vulkan(Rc::new(mln_vulkan::RendererBackend::presenting(device)?));
            let target = backend.surface(surface, size)?;
            Ok((backend, target))
        }
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        (Device::Gl(device), SurfaceKind::Gl(host)) => {
            let (backend, target) = mln_gl::HostSurfaceTarget::with_backend(device, *host, size)?;
            Ok((Backend::Gl(backend), Box::new(target)))
        }
        #[cfg(all(feature = "gl", unix, not(target_vendor = "apple")))]
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
    /// `target` is `None` once the ring is released.
    Borrowed {
        target: Option<Box<dyn Borrowed>>,
        producer: SyncKind,
    },
    Exported(Box<dyn Exported>),
}

/// A render target built on this thread: the backend that a renderer draws
/// with, on the target's device, and the frames it draws into.
pub struct LocalTarget {
    device: Device,
    backend: Backend,
    frames: Frames,
    /// The window that a surface target presents to, if it presents to one.
    window: Option<Arc<dyn Window>>,
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
        Ok(LocalTarget { device, backend, frames, window: None })
    }

    /// A target of `size` pixels that presents to `surface`, on `device` or,
    /// when that is `None`, on the device the surface's kind takes
    /// ([`RenderTarget::Surface`](super::RenderTarget::Surface)).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// surface that the host made without the host's device, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a window or
    /// surface without an area to present to, such as a minimized window,
    /// with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the device
    /// can't present to the surface, and with the error of the device or the
    /// window's surface.
    pub fn surface(surface: Surface, device: Option<Device>, size: Size) -> Result<LocalTarget, Error> {
        surface.check_area()?;
        let window = surface.host_window();
        let device = match device {
            Some(device) => device,
            None => default_surface_device(&surface)?,
        };
        let (backend, target) = surface_backend(&device, surface, size)?;
        Ok(LocalTarget { device, backend, frames: Frames::Presenting(Some(target)), window })
    }

    /// A target of `size` pixels on `device` that renders into the host's
    /// `ring`, and hands each frame over with sync of the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't render into the ring with `producer`, and with the
    /// backend's error.
    pub fn borrowed(device: Device, ring: BorrowedRing, producer: SyncKind) -> Result<LocalTarget, Error> {
        let backend = Backend::new(&device)?;
        let frames = Frames::Borrowed { target: Some(backend.borrowed(ring, producer)?), producer };
        Ok(LocalTarget { device, backend, frames, window: None })
    }

    /// A target on `device` that renders into a ring of `ring.depth` images
    /// of `size` pixels, which it exports as `kind`, one that
    /// [`Device::export_plan`] picked, and hands each frame over with sync of
    /// the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't export `kind` or hand frames over with `producer`, and
    /// with the backend's error.
    pub fn exported(
        device: Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<LocalTarget, Error> {
        let backend = Backend::new(&device)?;
        let frames = {
            let scoped = backend.as_dyn();
            let _scope = mln_gfx::BackendScope::new(&*scoped, mln_gfx::ScopeType::Explicit);
            Frames::Exported(backend.exported(&device, ring, kind, size, producer)?)
        };
        Ok(LocalTarget { device, backend, frames, window: None })
    }

    /// The device the target renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The backend that renderers drawing into the target draw with.
    #[must_use]
    pub fn backend(&self) -> Rc<dyn RendererBackend> {
        self.backend.as_dyn()
    }

    /// The frames to draw into, or `None` while a surface target has no
    /// surface.
    #[must_use]
    pub fn frames(&self) -> Option<&dyn FrameTarget> {
        match &self.frames {
            Frames::Offscreen(target) => Some(&**target),
            Frames::Presenting(target) => target.as_deref().map(|target| target as &dyn FrameTarget),
            Frames::Borrowed { target, .. } => target.as_deref().map(|target| target as &dyn FrameTarget),
            Frames::Exported(target) => Some(&**target),
        }
    }

    /// Whether the target can take a frame as far as its window says: it
    /// presents to no window, or to one with an area to present to
    /// ([`mln_gfx::window::has_area`]).
    #[must_use]
    pub fn window_has_area(&self) -> bool {
        self.window.as_deref().is_none_or(mln_gfx::window::has_area)
    }

    /// Whether frames are presented to a window or surface.
    #[must_use]
    pub fn is_presenting(&self) -> bool {
        matches!(self.frames, Frames::Presenting(_))
    }

    /// Whether frames go to the host in images of the target's own.
    #[must_use]
    pub fn is_exported(&self) -> bool {
        matches!(self.frames, Frames::Exported(_))
    }

    /// Whether frames go to the host's ring.
    #[must_use]
    pub fn is_borrowed(&self) -> bool {
        matches!(self.frames, Frames::Borrowed { .. })
    }

    /// Whether the target is a ring that let go of its textures.
    #[must_use]
    pub fn ring_released(&self) -> bool {
        matches!(self.frames, Frames::Borrowed { target: None, .. })
    }

    fn ring(&self) -> Result<&dyn Borrowed, Error> {
        match &self.frames {
            Frames::Borrowed { target: Some(target), .. } => Ok(&**target),
            Frames::Borrowed { target: None, .. } => Err(Error::invalid_state("the ring was released")),
            _ => Err(Error::unsupported("the target has no ring")),
        }
    }

    /// Has the next frame render into the ring's texture at `slot`.
    pub fn select_slot(&self, slot: usize) -> Result<(), Error> {
        if let Frames::Exported(target) = &self.frames {
            target.select(slot);
            return Ok(());
        }
        self.ring()?.select(slot);
        Ok(())
    }

    /// The host gave the ring's texture at `slot` back: the next frame
    /// rendered into it waits for `consumer`.
    pub fn release_slot(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        if let Frames::Exported(target) = &self.frames {
            return target.release(slot, consumer);
        }
        self.ring()?.release(slot, consumer)
    }

    /// The host gave up the ring's texture at `slot`, which the target
    /// renders into no more.
    pub fn discard_slot(&self, slot: usize) -> Result<(), Error> {
        if let Frames::Exported(target) = &self.frames {
            target.discard(slot);
            return Ok(());
        }
        self.ring()?.discard(slot);
        Ok(())
    }

    /// The size in pixels of an exported ring's images, or `None` for
    /// another target.
    #[must_use]
    pub fn exported_size(&self) -> Option<Size> {
        match &self.frames {
            Frames::Exported(target) => Some(target.size()),
            _ => None,
        }
    }

    /// Renders an exported ring's next frames into images of `size` pixels.
    /// The frames that the host holds, at `held` slots, keep their images.
    /// Returns whether the ring has new images, which it doesn't when they
    /// have `size` already.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that isn't an exported ring, and with the backend's error, when
    /// the ring keeps its images.
    pub fn resize_exported(&self, size: Size, held: &[usize]) -> Result<bool, Error> {
        match &self.frames {
            Frames::Exported(target) => target.resize(size, held),
            _ => Err(Error::unsupported("the target exports no ring")),
        }
    }

    /// The exported image of the frame at `slot`, of the size it has.
    #[must_use]
    pub fn exported_texture(&self, slot: usize) -> Option<FrameTexture> {
        match &self.frames {
            Frames::Exported(target) => {
                target.image(slot).map(|(size, image)| FrameTexture { size, kind: FrameTextureKind::Exported(image) })
            }
            _ => None,
        }
    }

    /// Renders into `ring` from now on, in place of the ring's textures, and
    /// returns the host's reads of the textures that it let go of, which the
    /// host may use once they end.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that isn't a ring, or a ring of another format, and with the
    /// backend's error.
    pub(crate) fn set_ring(&mut self, ring: BorrowedRing) -> Result<Pending, Error> {
        match &mut self.frames {
            Frames::Borrowed { target: Some(target), .. } => target.replace(ring),
            Frames::Borrowed { target, producer } => {
                *target = Some(self.backend.borrowed(ring, *producer)?);
                Ok(None)
            }
            _ => Err(Error::unsupported("the target has no ring")),
        }
    }

    /// Whether frames are read back rather than presented.
    #[must_use]
    pub fn is_offscreen(&self) -> bool {
        matches!(self.frames, Frames::Offscreen(_))
    }

    /// Draws the next frame at `size` pixels.
    pub fn resize(&self, size: Size) {
        match &self.frames {
            Frames::Offscreen(target) => target.resize(size),
            Frames::Presenting(Some(target)) => target.resize(size),
            Frames::Presenting(None) | Frames::Borrowed { .. } | Frames::Exported(_) => {}
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
            Frames::Borrowed { .. } | Frames::Exported(_) => {
                Err(Error::unsupported("a ring's frames are the host's to read"))
            }
        }
    }

    /// Lets go of a surface target's surface, or of a ring's textures, and
    /// returns the host's reads of the textures, which the host may use once
    /// they end. The backend stays.
    pub(crate) fn release_surface(&mut self) -> Pending {
        match &mut self.frames {
            Frames::Presenting(target) => {
                drop(target.take());
                self.window = None;
                None
            }
            Frames::Borrowed { target, .. } => target.take().and_then(|target| target.take_pending()),
            Frames::Offscreen(_) | Frames::Exported(_) => None,
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
        if !matches!(self.frames, Frames::Presenting(_)) {
            return Err(Error::unsupported("only a surface target presents to another surface"));
        }
        surface.check_area()?;
        let window = surface.host_window();
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
        self.window = window;
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
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        SurfaceKind::Vulkan(_) => Err(Error::invalid_argument("a surface that the host made needs the host's device")),
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        SurfaceKind::Gl(_) => Err(Error::invalid_argument("a surface that the host made needs the host's device")),
    }
}
