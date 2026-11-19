//! A render target built on the thread that renders: a [`RenderTarget`]'s
//! device and surface made into the backend's objects, which belong to that
//! thread.
//!
//! [`RenderTarget`]: super::RenderTarget

// The matches over the backends' enums list a variant per backend that the
// build's features and target have, so whether their `_` arm is unreachable,
// and whether a `let` of one variant is irrefutable, depends on the build.
#![allow(
    unreachable_patterns,
    irrefutable_let_patterns,
    reason = "the backends' variants depend on the build's features and target"
)]
#![cfg_attr(
    target_family = "wasm",
    expect(
        unused_macros,
        unused_imports,
        reason = "the browser build has only canvases' backends, and none of the targets that the others make"
    )
)]
#![cfg_attr(
    all(target_family = "wasm", not(feature = "webgpu")),
    expect(dead_code, reason = "WebGL2 renders only into a canvas, whose target makes its backend")
)]

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::ring::{self, Exporter, RingBackend};
use mln_gfx::{
    ConsumerSync, ExportKind, ExportedFrame, ExportedRing, FrameImage, FrameTarget, Handoff, RendererBackend, SyncKind,
    Window,
};
use mln_renderer::{Renderer, UpdateParameters};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::target::{
    BorrowedRing, Device, DeviceOptions, FrameTexture, RingKind, Surface, SurfaceKind, SurfaceOverlay,
};

/// Frames presented: a surface target on the backend `B`.
trait Presenting<B: RendererBackend>: FrameTarget<Gpu = B::Gpu> {
    fn resize(&mut self, size: Size);
}

/// Frames given back to the host: a ring of its textures, on the backend
/// `B`.
trait Borrowed<B: RendererBackend>: FrameTarget<Gpu = B::Gpu> {
    fn select(&mut self, slot: usize);
    fn release(&mut self, gpu: &mut B::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error>;
    fn discard(&mut self, gpu: &mut B::Gpu, slot: usize);
    /// Renders into `ring` from now on, and returns the host's reads of the
    /// textures that it let go of.
    fn replace(&mut self, gpu: &mut B::Gpu, ring: BorrowedRing) -> Result<Pending, Error>;
    /// Takes the host's reads of the textures that they still wait for, for
    /// the ring to wait for before it lets go of them. A target that waits
    /// for the host's reads as it takes each texture back has none.
    fn take_pending(&mut self, _gpu: &mut B::Gpu) -> Pending {
        None
    }
}

/// Frames queued for the host: a ring of the target's own images, which it
/// exports, on the backend `B`.
trait Exported<B: RendererBackend>: FrameTarget<Gpu = B::Gpu> {
    fn select(&mut self, slot: usize);
    fn release(&mut self, gpu: &mut B::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error>;
    fn discard(&mut self, gpu: &mut B::Gpu, slot: usize);
    fn resize(&mut self, backend: &mut B, size: Size, held: &[usize]) -> Result<bool, Error>;
    fn size(&self) -> Size;
    fn image(&self, slot: usize) -> Option<ExportedFrame>;
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
    all(feature = "metal", target_vendor = "apple"),
    all(feature = "d3d12", windows)
))]
fn pending(mut reads: impl PendingReads + 'static) -> Pending {
    (!reads.poll()).then(|| -> Box<dyn PendingReads> { Box::new(reads) })
}

/// A ring target of the backend `$backend` of the type `$type`, whose
/// textures come in `RingKind::$kind`, and whose replacement's result
/// `$pending` makes the host's reads that the ring waits for.
macro_rules! borrowed {
    ($backend:ty, $type:ty, $kind:ident, $what:literal, $pending:expr) => {
        impl Borrowed<$backend> for $type {
            fn select(&mut self, slot: usize) {
                <$type>::select(self, slot);
            }
            fn release(
                &mut self,
                gpu: &mut <$backend as RendererBackend>::Gpu,
                slot: usize,
                consumer: ConsumerSync,
            ) -> Result<(), Error> {
                <$type>::release(self, gpu, slot, consumer)
            }
            fn discard(&mut self, gpu: &mut <$backend as RendererBackend>::Gpu, slot: usize) {
                <$type>::discard(self, gpu, slot);
            }
            fn replace(
                &mut self,
                gpu: &mut <$backend as RendererBackend>::Gpu,
                ring: BorrowedRing,
            ) -> Result<Pending, Error> {
                let RingKind::$kind(textures) = ring.kind else {
                    return Err(Error::unsupported(concat!("a ring of ", $what, " takes ", $what, " only")));
                };
                <$type>::replace(self, gpu, textures).map($pending)
            }
        }
    };
}

/// A ring target written once in `mln_gfx`, on the backend `$backend`,
/// whose ring waits for the host's reads as it lets go of its textures.
macro_rules! borrowed_ring {
    ($backend:ty, $ring:ty, $kind:ident, $what:literal) => {
        impl Borrowed<$backend> for ring::BorrowedTarget<$ring> {
            fn select(&mut self, slot: usize) {
                ring::BorrowedTarget::select(self, slot);
            }
            fn release(
                &mut self,
                gpu: &mut <$backend as RendererBackend>::Gpu,
                slot: usize,
                consumer: ConsumerSync,
            ) -> Result<(), Error> {
                ring::BorrowedTarget::release(self, gpu, slot, consumer)
            }
            fn discard(&mut self, gpu: &mut <$backend as RendererBackend>::Gpu, slot: usize) {
                ring::BorrowedTarget::discard(self, gpu, slot);
            }
            fn replace(
                &mut self,
                gpu: &mut <$backend as RendererBackend>::Gpu,
                ring: BorrowedRing,
            ) -> Result<Pending, Error> {
                let RingKind::$kind(textures) = ring.kind else {
                    return Err(Error::unsupported(concat!("a ring of ", $what, " takes ", $what, " only")));
                };
                ring::BorrowedTarget::replace(self, gpu, textures).map(pending)
            }
            fn take_pending(&mut self, gpu: &mut <$backend as RendererBackend>::Gpu) -> Pending {
                pending(ring::BorrowedTarget::take_pending(self, gpu))
            }
        }
    };
}

impl<R: RingBackend> PendingReads for ring::PendingReads<R> {
    fn poll(&mut self) -> bool {
        ring::PendingReads::poll(self)
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
borrowed_ring!(mln_mtl::RendererBackend, mln_mtl::MetalRing, Metal, "Metal textures");
#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
borrowed_ring!(mln_vulkan::RendererBackend, mln_vulkan::VulkanRing, Vulkan, "Vulkan images");
#[cfg(all(feature = "d3d12", windows))]
borrowed_ring!(mln_d3d12::RendererBackend, mln_d3d12::D3d12Ring, D3d12, "D3D12 textures");

#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
impl Borrowed<mln_webgpu::RendererBackend> for mln_webgpu::BorrowedTarget {
    fn select(&mut self, slot: usize) {
        mln_webgpu::BorrowedTarget::select(self, slot);
    }
    fn release(&mut self, _gpu: &mut mln_webgpu::WebGpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_webgpu::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&mut self, _gpu: &mut mln_webgpu::WebGpu, slot: usize) {
        mln_webgpu::BorrowedTarget::discard(self, slot);
    }
    fn replace(&mut self, gpu: &mut mln_webgpu::WebGpu, ring: BorrowedRing) -> Result<Pending, Error> {
        let RingKind::WebGpu(textures) = ring.kind else {
            return Err(Error::unsupported("a ring of WebGPU textures takes WebGPU textures only"));
        };
        mln_webgpu::BorrowedTarget::replace(self, gpu, textures).map(|()| None)
    }
}

#[cfg(all(feature = "gl", not(target_family = "wasm")))]
borrowed!(mln_gl::RendererBackend, mln_gl::BorrowedTarget, Gl, "OpenGL textures", |()| None);

impl<E: Exporter> Exported<<E::Ring as RingBackend>::Renderer> for ring::ExportedTarget<E> {
    fn select(&mut self, slot: usize) {
        ring::ExportedTarget::select(self, slot);
    }
    fn release(
        &mut self,
        gpu: &mut <E::Ring as RingBackend>::Gpu,
        slot: usize,
        consumer: ConsumerSync,
    ) -> Result<(), Error> {
        ring::ExportedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut <E::Ring as RingBackend>::Gpu, slot: usize) {
        ring::ExportedTarget::discard(self, gpu, slot);
    }
    fn resize(
        &mut self,
        backend: &mut <E::Ring as RingBackend>::Renderer,
        size: Size,
        held: &[usize],
    ) -> Result<bool, Error> {
        ring::ExportedTarget::resize(self, backend, size, held)
    }
    fn size(&self) -> Size {
        ring::ExportedTarget::size(self)
    }
    fn image(&self, slot: usize) -> Option<ExportedFrame> {
        ring::ExportedTarget::image(self, slot)
    }
}

#[cfg(all(feature = "gl", unix, not(target_vendor = "apple")))]
impl Exported<mln_gl::RendererBackend> for mln_gl::ExportedTarget {
    fn select(&mut self, slot: usize) {
        mln_gl::ExportedTarget::select(self, slot);
    }
    fn release(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        mln_gl::ExportedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize) {
        mln_gl::ExportedTarget::discard(self, gpu, slot);
    }
    fn resize(&mut self, backend: &mut mln_gl::RendererBackend, size: Size, held: &[usize]) -> Result<bool, Error> {
        mln_gl::ExportedTarget::resize(self, &mut backend.gpu(), size, held)
    }
    fn size(&self) -> Size {
        mln_gl::ExportedTarget::size(self)
    }
    fn image(&self, slot: usize) -> Option<ExportedFrame> {
        mln_gl::ExportedTarget::image(self, slot)
    }
}

/// A surface target of the backend `$backend` of the type `$type`.
macro_rules! presenting {
    ($backend:ty, $type:ty) => {
        impl Presenting<$backend> for $type {
            fn resize(&mut self, size: Size) {
                <$type>::resize(self, size);
            }
        }
    };
}

#[cfg(feature = "webgpu")]
presenting!(mln_webgpu::RendererBackend, mln_webgpu::SurfaceTarget);
#[cfg(all(feature = "metal", target_vendor = "apple"))]
presenting!(mln_mtl::RendererBackend, mln_mtl::SurfaceTarget);
#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
presenting!(mln_vulkan::RendererBackend, mln_vulkan::SurfaceTarget);
#[cfg(all(feature = "d3d12", windows))]
presenting!(mln_d3d12::RendererBackend, mln_d3d12::SurfaceTarget);
#[cfg(all(feature = "gl", any(windows, all(unix, not(target_vendor = "apple")))))]
presenting!(mln_gl::RendererBackend, mln_gl::SurfaceTarget);
#[cfg(all(feature = "gl", not(target_family = "wasm")))]
presenting!(mln_gl::RendererBackend, mln_gl::HostSurfaceTarget);
#[cfg(all(feature = "gl", target_family = "wasm"))]
presenting!(mln_gl::RendererBackend, mln_gl::CanvasTarget);

/// What a [`BackendGfx`] draws into, which it makes on its backend.
enum FramesInit<'a> {
    Offscreen(Size),
    Borrowed(BorrowedRing, SyncKind),
    Exported { device: &'a Device, ring: &'a ExportedRing, kind: ExportKind, size: Size, producer: SyncKind },
    Surface(Surface, Option<SurfaceOverlay>, Size),
}

/// A backend of the build, and the targets that the session makes on it.
/// A kind of target that a backend has none of fails with `Unsupported`.
trait LocalBackend: RendererBackend + Sized {
    /// `gfx` as a session's.
    fn wrap(gfx: BackendGfx<Self>) -> Gfx;

    /// An offscreen target of `size` pixels on the backend: none in the
    /// browser, which renders into canvases.
    fn offscreen(&mut self, size: Size) -> Result<OffscreenTarget<Self::Gpu>, Error> {
        if cfg!(target_family = "wasm") {
            let _ = size;
            return Err(Error::unsupported("the browser build renders into canvases only"));
        }
        Ok(OffscreenTarget::new(size))
    }

    /// A target on the backend that renders into `ring`, handing frames
    /// over with sync of the kind `producer`.
    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let _ = (ring, producer);
        Err(Error::unsupported("the session's backend renders into no ring of this kind"))
    }

    /// A target on the backend that renders into a ring of `ring.depth`
    /// images of `size` pixels, which it exports as `kind`, and hands frames
    /// over with sync of the kind `producer`.
    fn exported(
        &mut self,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        let _ = (device, ring, kind, size, producer);
        Err(Error::unsupported("the session's backend exports no ring"))
    }

    /// A target on the backend that presents to `surface`, at `size`
    /// pixels, with `overlay` drawn over its frames. Fails with
    /// `Unsupported` when only a backend of the target's own can present to
    /// it: see [`surface_backend`].
    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<Box<dyn Presenting<Self>>, Error> {
        let _ = (surface, overlay, size);
        Err(Error::unsupported("the session's backend presents to no surface of this kind"))
    }
}

#[cfg(feature = "webgpu")]
impl LocalBackend for mln_webgpu::RendererBackend {
    fn wrap(gfx: BackendGfx<Self>) -> Gfx {
        Gfx::WebGpu(Box::new(gfx))
    }

    #[cfg(not(target_family = "wasm"))]
    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let RingKind::WebGpu(textures) = ring.kind else {
            return Err(Error::unsupported("the session's backend renders into no ring of this kind"));
        };
        Ok(Box::new(mln_webgpu::BorrowedTarget::new(self, textures, producer)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<Box<dyn Presenting<Self>>, Error> {
        let kind = surface.kind;
        #[cfg(target_family = "wasm")]
        let format = match &kind {
            SurfaceKind::WebCanvas { format: Some(format), .. } => Some(match format {
                super::target::CanvasFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
                super::target::CanvasFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
                super::target::CanvasFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            }),
            _ => None,
        };
        #[cfg(not(target_family = "wasm"))]
        let format = None;
        let overlay = overlay_of(overlay, |overlay| {
            if let SurfaceOverlay::WebGpu(overlay) = overlay { Ok(overlay) } else { Err(overlay) }
        })?;
        let target = webgpu_surface(self, kind)?;
        let options = mln_webgpu::SurfaceOptions {
            cap_frame_rate: surface.cap_frame_rate,
            format,
            ..mln_webgpu::SurfaceOptions::default()
        };
        let mut target = mln_webgpu::SurfaceTarget::new(self, target, size, options)?;
        target.set_overlay(overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
impl LocalBackend for mln_mtl::RendererBackend {
    fn wrap(gfx: BackendGfx<Self>) -> Gfx {
        Gfx::Metal(Box::new(gfx))
    }

    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let RingKind::Metal(textures) = ring.kind else {
            return Err(Error::unsupported("the session's backend renders into no ring of this kind"));
        };
        Ok(Box::new(mln_mtl::BorrowedTarget::new(self, textures, producer)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_mtl::ExportedTarget::new(self, ring, kind, size, producer)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<Box<dyn Presenting<Self>>, Error> {
        let SurfaceKind::MetalLayer(layer) = surface.kind else {
            return Err(Error::unsupported("the session's backend presents to no surface of this kind"));
        };
        let overlay = metal_overlay(overlay)?;
        let mut target = mln_mtl::SurfaceTarget::new(self, layer, size, surface.cap_frame_rate);
        target.set_overlay(overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
impl LocalBackend for mln_vulkan::RendererBackend {
    fn wrap(gfx: BackendGfx<Self>) -> Gfx {
        Gfx::Vulkan(Box::new(gfx))
    }

    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let RingKind::Vulkan(images) = ring.kind else {
            return Err(Error::unsupported("the session's backend renders into no ring of this kind"));
        };
        Ok(Box::new(mln_vulkan::BorrowedTarget::new(self, images, producer)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_vulkan::ExportedTarget::new(self, ring, kind, size, producer)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<Box<dyn Presenting<Self>>, Error> {
        let overlay = vulkan_overlay(overlay)?;
        let mut target = match surface.kind {
            SurfaceKind::Window(window) => mln_vulkan::SurfaceTarget::new(self, window, size, surface.cap_frame_rate)?,
            SurfaceKind::Vulkan(host) => {
                mln_vulkan::SurfaceTarget::on_host_surface(self, host, size, surface.cap_frame_rate)?
            }
            _ => return Err(Error::unsupported("the session's backend presents to no surface of this kind")),
        };
        target.set_overlay(self.gpu(), overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "d3d12", windows))]
impl LocalBackend for mln_d3d12::RendererBackend {
    fn wrap(gfx: BackendGfx<Self>) -> Gfx {
        Gfx::D3d12(Box::new(gfx))
    }

    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let RingKind::D3d12(textures) = ring.kind else {
            return Err(Error::unsupported("the session's backend renders into no ring of this kind"));
        };
        Ok(Box::new(mln_d3d12::BorrowedTarget::new(self, textures, producer)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_d3d12::ExportedTarget::new(self, ring, kind, size, producer)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<Box<dyn Presenting<Self>>, Error> {
        let target = match surface.kind {
            SurfaceKind::Window(window) => {
                let overlay = d3d12_overlay(overlay)?;
                let mut target = mln_d3d12::SurfaceTarget::new(self, window, size)?;
                target.set_overlay(self.gpu(), overlay);
                target
            }
            SurfaceKind::D3d12Composition { composition, thread } => {
                if composition.is_swap_chain_panel() && thread != std::thread::current().id() {
                    return Err(Error::unsupported(
                        "a SwapChainPanel takes its swap chain on its UI thread, which made the surface: attach it \
                         with Driver::CallerThread, serviced there",
                    ));
                }
                let overlay = d3d12_overlay(overlay)?;
                let mut target = mln_d3d12::SurfaceTarget::for_composition(self, composition, size)?;
                target.set_overlay(self.gpu(), overlay);
                target
            }
            _ => return Err(Error::unsupported("the session's backend presents to no surface of this kind")),
        };
        Ok(Box::new(target))
    }
}

#[cfg(feature = "gl")]
impl LocalBackend for mln_gl::RendererBackend {
    fn wrap(gfx: BackendGfx<Self>) -> Gfx {
        Gfx::Gl(Box::new(gfx))
    }

    #[cfg(not(target_family = "wasm"))]
    fn borrowed(&mut self, ring: BorrowedRing, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let RingKind::Gl(textures) = ring.kind else {
            return Err(Error::unsupported("the session's backend renders into no ring of this kind"));
        };
        Ok(Box::new(mln_gl::BorrowedTarget::new(textures, producer)?))
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    fn exported(
        &mut self,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        // Irrefutable in a build with OpenGL alone, which has no other kind
        // of device, and refutable in one with other backends.
        let Device::Gl(device) = device else {
            return Err(Error::unsupported("an OpenGL backend renders on an OpenGL device"));
        };
        Ok(Box::new(mln_gl::ExportedTarget::new(self, device, ring, kind, size, producer)?))
    }
}

/// A backend's GPU objects, which the session owns by value: the backend,
/// the target it draws into, and the renderer that draws, once there is
/// one. Each call that reaches the backend's `Gpu` lends it, for its scope,
/// to the renderer and the target. Dropping it tears the renderer and the
/// target down on the `Gpu`, and drops the target before the backend.
struct BackendGfx<B: LocalBackend> {
    // Fields drop in order: the target's native objects, such as a swap
    // chain, go while the backend's device and `Gpu` live.
    frames: Frames<B>,
    renderer: Option<Renderer>,
    backend: B,
}

/// What a [`BackendGfx`] draws into.
enum Frames<B: RendererBackend> {
    Offscreen(OffscreenTarget<B::Gpu>),
    /// `None` once the surface is released.
    Presenting(Option<Box<dyn Presenting<B>>>),
    /// `target` is `None` once the ring is released.
    Borrowed {
        target: Option<Box<dyn Borrowed<B>>>,
        producer: SyncKind,
    },
    Exported(Box<dyn Exported<B>>),
}

impl<B: RendererBackend> Frames<B> {
    /// The target to draw into, or `None` while there is no surface or ring.
    fn target(&mut self) -> Option<&mut dyn FrameTarget<Gpu = B::Gpu>> {
        match self {
            Frames::Offscreen(target) => Some(target),
            Frames::Presenting(target) => {
                target.as_deref_mut().map(|target| -> &mut dyn FrameTarget<Gpu = B::Gpu> { target })
            }
            Frames::Borrowed { target, .. } => {
                target.as_deref_mut().map(|target| -> &mut dyn FrameTarget<Gpu = B::Gpu> { target })
            }
            Frames::Exported(target) => Some(&mut **target),
        }
    }
}

impl<B: LocalBackend> BackendGfx<B> {
    /// `backend`, and what `init` says it draws into, made on it.
    fn with_frames(mut backend: B, init: FramesInit<'_>) -> Result<Gfx, Error> {
        let frames = match init {
            FramesInit::Offscreen(size) => Frames::Offscreen(backend.offscreen(size)?),
            FramesInit::Borrowed(ring, producer) => {
                Frames::Borrowed { target: Some(backend.borrowed(ring, producer)?), producer }
            }
            FramesInit::Exported { device, ring, kind, size, producer } => {
                Frames::Exported(backend.exported(device, ring, kind, size, producer)?)
            }
            FramesInit::Surface(surface, overlay, size) => {
                Frames::Presenting(Some(backend.surface(surface, overlay, size)?))
            }
        };
        Ok(B::wrap(BackendGfx { backend, frames, renderer: None }))
    }

    /// The renderer that draws into the target, made by `make` with the
    /// backend's `Gpu` unless there is one.
    fn renderer_or(&mut self, make: impl FnOnce(&dyn Gpu) -> Renderer) -> &mut Renderer {
        let BackendGfx { backend, renderer, .. } = self;
        renderer.get_or_insert_with(|| make(&*backend.gpu()))
    }

    /// Runs `f` with the renderer and the backend's `Gpu`, if there is a
    /// renderer.
    fn with_renderer<R>(&mut self, f: impl FnOnce(&mut Renderer, &mut dyn Gpu) -> R) -> Option<R> {
        let renderer = self.renderer.as_mut()?;
        Some(f(renderer, &mut *self.backend.gpu()))
    }

    /// Tears the renderer down, if there is one, and returns its rendering
    /// stats as it leaves them.
    fn tear_down_renderer(&mut self) -> Option<RenderingStats> {
        let renderer = self.renderer.take()?;
        Some(renderer.tear_down(&mut *self.backend.gpu()))
    }

    /// Draws `update` into a frame of the target: the renderer updates to
    /// it, the target begins the frame, the renderer draws into it, and the
    /// target ends it. Returns where the frame went, or `None` when nothing
    /// was drawn.
    fn render(&mut self, update: &Arc<UpdateParameters>) -> Option<Handoff> {
        let renderer = self.renderer.as_mut()?;
        let target = self.frames.target()?;
        let mut gpu = self.backend.gpu();
        let gpu = &mut *gpu;
        let frame = renderer.update(gpu, update)?;
        let image = target.begin_frame(gpu)?;
        let drawn = renderer.render(gpu, &image, frame);
        let handoff = target.end_frame(gpu);
        renderer.finish_frame(gpu, drawn, handoff);
        Some(handoff)
    }

    /// Whether the target begins a frame, which it then keeps for the next
    /// render.
    fn ready_for_frame(&mut self) -> bool {
        let Some(target) = self.frames.target() else { return false };
        target.begin_frame(&mut self.backend.gpu()).is_some()
    }

    /// Runs `draw` with the backend's `Gpu` and the textures of a frame of
    /// the target, which the frame keeps until it ends, or `None` when the
    /// target has no frame to draw into.
    fn with_frame<R>(&mut self, draw: impl FnOnce(&mut dyn Gpu, &FrameImage) -> R) -> Option<R> {
        let target = self.frames.target()?;
        let mut gpu = self.backend.gpu();
        let image = target.begin_frame(&mut gpu)?;
        Some(draw(&mut *gpu, &image))
    }

    fn select_slot(&mut self, slot: usize) -> Result<(), Error> {
        match &mut self.frames {
            Frames::Exported(target) => target.select(slot),
            frames => ring(frames)?.select(slot),
        }
        Ok(())
    }

    fn release_slot(&mut self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Exported(target) => target.release(gpu, slot, consumer),
            frames => ring(frames)?.release(gpu, slot, consumer),
        }
    }

    fn discard_slot(&mut self, slot: usize) -> Result<(), Error> {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Exported(target) => target.discard(gpu, slot),
            frames => ring(frames)?.discard(gpu, slot),
        }
        Ok(())
    }

    fn exported_size(&self) -> Option<Size> {
        match &self.frames {
            Frames::Exported(target) => Some(target.size()),
            _ => None,
        }
    }

    fn resize_exported(&mut self, size: Size, held: &[usize]) -> Result<bool, Error> {
        match &mut self.frames {
            Frames::Exported(target) => target.resize(&mut self.backend, size, held),
            _ => Err(Error::unsupported("the target exports no ring")),
        }
    }

    fn exported_image(&self, slot: usize) -> Option<ExportedFrame> {
        match &self.frames {
            Frames::Exported(target) => target.image(slot),
            _ => None,
        }
    }

    fn set_ring(&mut self, ring: BorrowedRing) -> Result<Pending, Error> {
        match &mut self.frames {
            Frames::Borrowed { target: Some(target), .. } => target.replace(&mut self.backend.gpu(), ring),
            Frames::Borrowed { target, producer } => {
                *target = Some(self.backend.borrowed(ring, *producer)?);
                Ok(None)
            }
            _ => Err(Error::unsupported("the target has no ring")),
        }
    }

    fn resize(&mut self, size: Size) {
        match &mut self.frames {
            Frames::Offscreen(target) => target.resize(&mut self.backend.gpu(), size),
            Frames::Presenting(Some(target)) => target.resize(size),
            Frames::Presenting(None) | Frames::Borrowed { .. } | Frames::Exported(_) => {}
        }
    }

    fn read_image(&mut self) -> Result<PremultipliedImage, Error> {
        match &mut self.frames {
            Frames::Offscreen(target) => target.read_image(&mut self.backend.gpu()),
            Frames::Presenting(_) => Err(Error::unsupported("a surface session can't read its frames back")),
            Frames::Borrowed { .. } | Frames::Exported(_) => {
                Err(Error::unsupported("a ring's frames are the host's to read"))
            }
        }
    }

    fn release_surface(&mut self) -> Pending {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Presenting(target) => {
                if let Some(mut target) = target.take() {
                    target.tear_down(gpu);
                }
                None
            }
            Frames::Borrowed { target, .. } => target.take().and_then(|mut target| {
                let pending = target.take_pending(gpu);
                target.tear_down(gpu);
                pending
            }),
            Frames::Offscreen(_) | Frames::Exported(_) => None,
        }
    }

    /// Presents to `surface` from now on, in place of the surface the target
    /// has, if any, on the same backend.
    fn set_surface(&mut self, surface: Surface, overlay: Option<SurfaceOverlay>, size: Size) -> Result<(), Error> {
        let target = self.backend.surface(surface, overlay, size)?;
        if let Frames::Presenting(old) = &mut self.frames
            && let Some(mut old) = old.take()
        {
            old.tear_down(&mut self.backend.gpu());
        }
        self.frames = Frames::Presenting(Some(target));
        Ok(())
    }
}

/// The ring that `frames` draws into.
fn ring<B: RendererBackend>(frames: &mut Frames<B>) -> Result<&mut dyn Borrowed<B>, Error> {
    match frames {
        Frames::Borrowed { target: Some(target), .. } => Ok(&mut **target),
        Frames::Borrowed { target: None, .. } => Err(Error::invalid_state("the ring was released")),
        _ => Err(Error::unsupported("the target has no ring")),
    }
}

impl<B: LocalBackend> Drop for BackendGfx<B> {
    fn drop(&mut self) {
        let mut gpu = self.backend.gpu();
        if let Some(renderer) = self.renderer.take() {
            renderer.tear_down(&mut *gpu);
        }
        if let Some(target) = self.frames.target() {
            target.tear_down(&mut gpu);
        }
    }
}

/// A backend's GPU objects, by the backend's type, each boxed: a backend's
/// `Gpu` is large.
enum Gfx {
    #[cfg(feature = "webgpu")]
    WebGpu(Box<BackendGfx<mln_webgpu::RendererBackend>>),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(Box<BackendGfx<mln_mtl::RendererBackend>>),
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(Box<BackendGfx<mln_vulkan::RendererBackend>>),
    #[cfg(all(feature = "d3d12", windows))]
    D3d12(Box<BackendGfx<mln_d3d12::RendererBackend>>),
    #[cfg(feature = "gl")]
    Gl(Box<BackendGfx<mln_gl::RendererBackend>>),
}

/// Runs `$body` with `$gfx`'s [`BackendGfx`] as `$name`, of whichever
/// backend it is.
macro_rules! each {
    ($gfx:expr, $name:ident => $body:expr) => {
        match $gfx {
            #[cfg(feature = "webgpu")]
            Gfx::WebGpu($name) => $body,
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Gfx::Metal($name) => $body,
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Gfx::Vulkan($name) => $body,
            #[cfg(all(feature = "d3d12", windows))]
            Gfx::D3d12($name) => $body,
            #[cfg(feature = "gl")]
            Gfx::Gl($name) => $body,
        }
    };
}

impl Gfx {
    /// A backend on `device`, and what `init` says it draws into.
    fn new(device: &Device, init: FramesInit<'_>) -> Result<Gfx, Error> {
        match device {
            #[cfg(feature = "webgpu")]
            Device::WebGpu(device) => BackendGfx::with_frames(mln_webgpu::RendererBackend::new(device), init),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Device::Metal(device) => BackendGfx::with_frames(mln_mtl::RendererBackend::new(device), init),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Device::Vulkan(device) => BackendGfx::with_frames(mln_vulkan::RendererBackend::new(device)?, init),
            #[cfg(all(feature = "d3d12", windows))]
            Device::D3d12(device) => BackendGfx::with_frames(
                mln_d3d12::RendererBackend::new(device).map_err(|error| Error::native(error.to_string()))?,
                init,
            ),
            #[cfg(all(feature = "gl", not(target_family = "wasm")))]
            Device::Gl(device) => BackendGfx::with_frames(mln_gl::RendererBackend::new(device)?, init),
            #[cfg(all(feature = "gl", target_family = "wasm"))]
            Device::WebGl => {
                drop(init);
                Err(Error::unsupported("WebGL2 renders only into a canvas, with the canvas's own context"))
            }
        }
    }
}

/// The overlay of the backend that `own` picks out of `overlay`. Fails with
/// `Unsupported` for another backend's.
#[cfg_attr(
    not(any(
        feature = "webgpu",
        all(feature = "metal", target_vendor = "apple"),
        all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))),
        all(feature = "d3d12", windows),
        all(feature = "gl", any(windows, all(unix, not(target_vendor = "apple")))),
    )),
    expect(dead_code, reason = "a build without a backend that takes overlays uses none")
)]
fn overlay_of<T>(
    overlay: Option<SurfaceOverlay>,
    own: impl FnOnce(SurfaceOverlay) -> Result<T, SurfaceOverlay>,
) -> Result<Option<T>, Error> {
    overlay
        .map(own)
        .transpose()
        .map_err(|_| Error::unsupported("the surface's overlay is for another backend than the session's"))
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
fn metal_overlay(overlay: Option<SurfaceOverlay>) -> Result<Option<mln_mtl::surface_target::Overlay>, Error> {
    overlay_of(overlay, |overlay| if let SurfaceOverlay::Metal(overlay) = overlay { Ok(overlay) } else { Err(overlay) })
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
fn vulkan_overlay(overlay: Option<SurfaceOverlay>) -> Result<Option<Box<dyn mln_vulkan::Overlay>>, Error> {
    overlay_of(overlay, |overlay| match overlay {
        SurfaceOverlay::Vulkan(overlay) => Ok(overlay),
        other => Err(other),
    })
}

#[cfg(all(feature = "d3d12", windows))]
fn d3d12_overlay(overlay: Option<SurfaceOverlay>) -> Result<Option<Box<dyn mln_d3d12::Overlay>>, Error> {
    overlay_of(overlay, |overlay| match overlay {
        SurfaceOverlay::D3d12(overlay) => Ok(overlay),
        other => Err(other),
    })
}

/// A wgpu surface of `kind`, made with the instance of `backend`'s device
/// where the host didn't make it.
#[cfg(feature = "webgpu")]
fn webgpu_surface(backend: &mln_webgpu::RendererBackend, kind: SurfaceKind) -> Result<wgpu::Surface<'static>, Error> {
    let instance = || {
        backend
            .device()
            .instance()
            .ok_or_else(|| Error::invalid_argument("the session's device has no instance to make a surface with"))
    };
    let made = match kind {
        #[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
        SurfaceKind::Wgpu(surface) => return Ok(surface),
        SurfaceKind::Window(window) => instance()?.create_surface(window),
        #[cfg(target_family = "wasm")]
        SurfaceKind::WebCanvas { canvas, .. } => {
            use wasm_bindgen::JsCast as _;
            let target = match canvas.dyn_into::<web_sys::OffscreenCanvas>() {
                Ok(canvas) => wgpu::SurfaceTarget::OffscreenCanvas(canvas),
                Err(canvas) => wgpu::SurfaceTarget::Canvas(canvas.unchecked_into()),
            };
            instance()?.create_surface(target)
        }
        #[cfg(all(feature = "metal", target_vendor = "apple"))]
        SurfaceKind::MetalLayer(_) => return Err(Error::unsupported("a Metal layer presents on a Metal device")),
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        SurfaceKind::Vulkan(_) => return Err(Error::unsupported("a Vulkan surface presents on a Vulkan device")),
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        SurfaceKind::Gl(_) => return Err(Error::unsupported("an EGL surface presents on an OpenGL device")),
        #[cfg(all(feature = "d3d12", windows))]
        SurfaceKind::D3d12Composition { .. } => {
            return Err(Error::unsupported("a composition swap chain presents on a D3D12 device"));
        }
    };
    made.map_err(|error| Error::native(format!("can't make a surface on the window: {error}")))
}

/// A backend of the target's own on `device` and a target that presents to
/// `surface` with `overlay` drawn over its frames, for the backends whose
/// context or device is made for the window: Vulkan's, whose instance and
/// device take the window's surface extensions, and OpenGL ES's, whose
/// context is of the window's display.
fn surface_backend(
    device: &Device,
    surface: Surface,
    overlay: Option<SurfaceOverlay>,
    size: Size,
) -> Result<Gfx, Error> {
    let cap_frame_rate = surface.cap_frame_rate;
    match (device, surface.kind) {
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), SurfaceKind::Window(window)) => {
            let overlay = vulkan_overlay(overlay)?;
            let (mut backend, mut target) =
                mln_vulkan::SurfaceTarget::with_backend(device, window, size, cap_frame_rate)?;
            target.set_overlay(backend.gpu(), overlay);
            Ok(presenting(backend, target))
        }
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), kind @ SurfaceKind::Vulkan(_)) => BackendGfx::with_frames(
            mln_vulkan::RendererBackend::presenting(device)?,
            FramesInit::Surface(Surface { kind, cap_frame_rate }, overlay, size),
        ),
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        (Device::Gl(device), SurfaceKind::Gl(host)) => {
            if overlay.is_some() {
                return Err(Error::unsupported("a host's EGL surface takes no overlay"));
            }
            let (backend, target) = mln_gl::HostSurfaceTarget::with_backend(device, host, size)?;
            Ok(presenting(backend, target))
        }
        #[cfg(all(feature = "gl", any(windows, all(unix, not(target_vendor = "apple")))))]
        (Device::Gl(_), SurfaceKind::Window(window)) => {
            let overlay = overlay_of(overlay, |overlay| match overlay {
                SurfaceOverlay::Gl(overlay) => Ok(overlay),
                other => Err(other),
            })?;
            let (backend, mut target) = mln_gl::SurfaceTarget::with_backend(window, size, cap_frame_rate)?;
            target.set_overlay(overlay);
            Ok(presenting(backend, target))
        }
        #[cfg(all(feature = "gl", target_family = "wasm"))]
        (Device::WebGl, SurfaceKind::WebCanvas { canvas, .. }) => {
            use wasm_bindgen::JsCast as _;
            // WebGL draws no overlay.
            let canvas = match canvas.dyn_into::<web_sys::OffscreenCanvas>() {
                Ok(canvas) => mln_gl::WebCanvas::Offscreen(canvas),
                Err(canvas) => mln_gl::WebCanvas::Element(canvas.unchecked_into()),
            };
            let (backend, target) = mln_gl::CanvasTarget::with_backend(canvas, size)?;
            Ok(presenting(backend, target))
        }
        (_, kind) => Gfx::new(device, FramesInit::Surface(Surface { kind, cap_frame_rate }, overlay, size)),
    }
}

/// `backend`, presenting with `target`.
#[cfg_attr(
    not(any(feature = "gl", all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))),
    expect(dead_code, reason = "only Vulkan and OpenGL make backends of a surface's own")
)]
fn presenting<B: LocalBackend>(backend: B, target: impl Presenting<B> + 'static) -> Gfx {
    B::wrap(BackendGfx { backend, frames: Frames::Presenting(Some(Box::new(target))), renderer: None })
}

/// A target's backend, by its type, for a host that reads what the backend
/// tells of its device.
#[non_exhaustive]
pub enum BackendRef<'a> {
    #[cfg(feature = "webgpu")]
    WebGpu(&'a mln_webgpu::RendererBackend),
    #[cfg(all(feature = "metal", target_vendor = "apple"))]
    Metal(&'a mln_mtl::RendererBackend),
    #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
    Vulkan(&'a mln_vulkan::RendererBackend),
    #[cfg(all(feature = "d3d12", windows))]
    D3d12(&'a mln_d3d12::RendererBackend),
    #[cfg(feature = "gl")]
    Gl(&'a mln_gl::RendererBackend),
}

/// A render target built on this thread: the backend, on the target's
/// device, the frames it draws into, and the renderer that draws them,
/// which the target owns and lends the backend's `Gpu` to.
pub struct LocalTarget {
    device: Device,
    gfx: Gfx,
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
        let gfx = Gfx::new(&device, FramesInit::Offscreen(size))?;
        Ok(LocalTarget { device, gfx, window: None })
    }

    /// A target of `size` pixels that presents to `surface`, with `overlay`
    /// drawn over its frames, on `device` or, when that is `None`, on the
    /// device the surface's kind takes
    /// ([`RenderTarget::Surface`](super::RenderTarget::Surface)).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// surface that the host made without the host's device, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a window or
    /// surface without an area to present to, such as a minimized window,
    /// with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the device
    /// can't present to the surface or draw the overlay, and with the error
    /// of the device or the window's surface.
    pub fn surface(
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        device: Option<Device>,
        size: Size,
    ) -> Result<LocalTarget, Error> {
        surface.check_area()?;
        let window = surface.host_window();
        let device = match device {
            Some(device) => device,
            None => default_surface_device(&surface)?,
        };
        let gfx = surface_backend(&device, surface, overlay, size)?;
        Ok(LocalTarget { device, gfx, window })
    }

    /// A target of `size` pixels on `device` that renders into the host's
    /// `ring`, and hands each frame over with sync of the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't render into the ring with `producer`, and with the
    /// backend's error.
    pub fn borrowed(device: Device, ring: BorrowedRing, producer: SyncKind) -> Result<LocalTarget, Error> {
        let gfx = Gfx::new(&device, FramesInit::Borrowed(ring, producer))?;
        Ok(LocalTarget { device, gfx, window: None })
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
        let gfx = Gfx::new(&device, FramesInit::Exported { device: &device, ring, kind, size, producer })?;
        Ok(LocalTarget { device, gfx, window: None })
    }

    /// The device the target renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The backend the target and its renderer draw with.
    #[must_use]
    pub fn backend(&self) -> BackendRef<'_> {
        match &self.gfx {
            #[cfg(feature = "webgpu")]
            Gfx::WebGpu(gfx) => BackendRef::WebGpu(&gfx.backend),
            #[cfg(all(feature = "metal", target_vendor = "apple"))]
            Gfx::Metal(gfx) => BackendRef::Metal(&gfx.backend),
            #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
            Gfx::Vulkan(gfx) => BackendRef::Vulkan(&gfx.backend),
            #[cfg(all(feature = "d3d12", windows))]
            Gfx::D3d12(gfx) => BackendRef::D3d12(&gfx.backend),
            #[cfg(feature = "gl")]
            Gfx::Gl(gfx) => BackendRef::Gl(&gfx.backend),
        }
    }

    /// Runs `f` with the backend's `Gpu`, as when the host waits for the
    /// frames drawn so far.
    pub fn with_gpu<R>(&mut self, f: impl FnOnce(&mut dyn Gpu) -> R) -> R {
        each!(&mut self.gfx, gfx => f(&mut *gfx.backend.gpu()))
    }

    /// The renderer that draws into the target, which `make` makes with the
    /// backend's `Gpu` unless the target has one: the renderer draws with
    /// that `Gpu` only.
    pub fn renderer_or(&mut self, make: impl FnOnce(&dyn Gpu) -> Renderer) -> &mut Renderer {
        each!(&mut self.gfx, gfx => gfx.renderer_or(make))
    }

    /// The renderer that draws into the target, if it has one.
    #[must_use]
    pub fn renderer(&self) -> Option<&Renderer> {
        each!(&self.gfx, gfx => gfx.renderer.as_ref())
    }

    /// The renderer that draws into the target, if it has one.
    pub fn renderer_mut(&mut self) -> Option<&mut Renderer> {
        each!(&mut self.gfx, gfx => gfx.renderer.as_mut())
    }

    /// Runs `f` with the target's renderer and the backend's `Gpu`, which the
    /// renderer's calls that make, draw or free its GPU objects take, or
    /// returns `None` without a renderer.
    pub fn with_renderer<R>(&mut self, f: impl FnOnce(&mut Renderer, &mut dyn Gpu) -> R) -> Option<R> {
        each!(&mut self.gfx, gfx => gfx.with_renderer(f))
    }

    /// Tears the renderer down on the backend's `Gpu`, if the target has
    /// one, and returns its rendering stats as it leaves them.
    pub fn tear_down_renderer(&mut self) -> Option<RenderingStats> {
        each!(&mut self.gfx, gfx => gfx.tear_down_renderer())
    }

    /// Draws `update` with the target's renderer into a frame: the target
    /// begins the frame, the renderer draws into it, and the target ends
    /// it. Returns where the frame went, or `None` when nothing was drawn:
    /// there is no renderer, the target had nothing to draw into, or there
    /// was nothing to draw.
    pub fn render(&mut self, update: &Arc<UpdateParameters>) -> Option<Handoff> {
        each!(&mut self.gfx, gfx => gfx.render(update))
    }

    /// Runs `draw` with the backend's `Gpu` and the textures of the target's
    /// frame, to draw into them alongside the map. The frame is the one the
    /// next render draws into, which the target begins unless it has
    /// begun it. `None` when the target has nothing to draw into now.
    pub fn with_frame<R>(&mut self, draw: impl FnOnce(&mut dyn Gpu, &FrameImage) -> R) -> Option<R> {
        each!(&mut self.gfx, gfx => gfx.with_frame(draw))
    }

    /// Whether the target can take a frame now: its window, if any, has an
    /// area to present to, and it begins a frame, which a surface needs a
    /// texture for, and which the next render draws into.
    pub fn ready_for_frame(&mut self) -> bool {
        self.window_has_area() && each!(&mut self.gfx, gfx => gfx.ready_for_frame())
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
        each!(&self.gfx, gfx => matches!(gfx.frames, Frames::Presenting(_)))
    }

    /// Whether frames go to the host in images of the target's own.
    #[must_use]
    pub fn is_exported(&self) -> bool {
        each!(&self.gfx, gfx => matches!(gfx.frames, Frames::Exported(_)))
    }

    /// Whether frames go to the host's ring.
    #[must_use]
    pub fn is_borrowed(&self) -> bool {
        each!(&self.gfx, gfx => matches!(gfx.frames, Frames::Borrowed { .. }))
    }

    /// Whether the target is a ring that let go of its textures.
    #[must_use]
    pub fn ring_released(&self) -> bool {
        each!(&self.gfx, gfx => matches!(gfx.frames, Frames::Borrowed { target: None, .. }))
    }

    /// Whether frames are read back rather than presented.
    #[must_use]
    pub fn is_offscreen(&self) -> bool {
        each!(&self.gfx, gfx => matches!(gfx.frames, Frames::Offscreen(_)))
    }

    /// Has the next frame render into the ring's texture at `slot`.
    pub fn select_slot(&mut self, slot: usize) -> Result<(), Error> {
        each!(&mut self.gfx, gfx => gfx.select_slot(slot))
    }

    /// The host gave the ring's texture at `slot` back: the next frame
    /// rendered into it waits for `consumer`.
    pub fn release_slot(&mut self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        each!(&mut self.gfx, gfx => gfx.release_slot(slot, consumer))
    }

    /// The host gave up the ring's texture at `slot`, which the target
    /// renders into no more.
    pub fn discard_slot(&mut self, slot: usize) -> Result<(), Error> {
        each!(&mut self.gfx, gfx => gfx.discard_slot(slot))
    }

    /// The size in pixels of an exported ring's images, or `None` for
    /// another target.
    #[must_use]
    pub fn exported_size(&self) -> Option<Size> {
        each!(&self.gfx, gfx => gfx.exported_size())
    }

    /// Renders an exported ring's next frames into images of `size` pixels.
    /// The frames that the host holds, at `held` slots, keep their images.
    /// Returns whether the ring has new images, which it doesn't when they
    /// have `size` already.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that isn't an exported ring, and with the backend's error, when
    /// the ring keeps its images.
    pub fn resize_exported(&mut self, size: Size, held: &[usize]) -> Result<bool, Error> {
        each!(&mut self.gfx, gfx => gfx.resize_exported(size, held))
    }

    /// The exported image of the frame at `slot`, of the size it has.
    #[must_use]
    pub fn exported_texture(&self, slot: usize) -> Option<FrameTexture> {
        let image = each!(&self.gfx, gfx => gfx.exported_image(slot))?;
        Some(FrameTexture::exported(image, &self.device))
    }

    /// Renders into `ring` from now on, in place of the ring's textures, and
    /// returns the host's reads of the textures that it let go of, which the
    /// host may use once they end.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that isn't a ring, or a ring of another format, and with the
    /// backend's error.
    pub(crate) fn set_ring(&mut self, ring: BorrowedRing) -> Result<Pending, Error> {
        each!(&mut self.gfx, gfx => gfx.set_ring(ring))
    }

    /// Draws the next frame at `size` pixels.
    pub fn resize(&mut self, size: Size) {
        each!(&mut self.gfx, gfx => gfx.resize(size));
    }

    /// The last frame's pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// target that presents its frames, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) before a frame, and
    /// with [`ErrorKind::Native`](mln_util::ErrorKind) where the backend
    /// reads nothing back.
    pub fn read_image(&mut self) -> Result<PremultipliedImage, Error> {
        each!(&mut self.gfx, gfx => gfx.read_image())
    }

    /// Lets go of a surface target's surface, or of a ring's textures, and
    /// returns the host's reads of the textures, which the host may use once
    /// they end. The backend and its renderer stay.
    pub(crate) fn release_surface(&mut self) -> Pending {
        let pending = each!(&mut self.gfx, gfx => gfx.release_surface());
        if self.is_presenting() {
            self.window = None;
        }
        pending
    }

    /// Presents to `surface` at `size` pixels from now on, with `overlay`
    /// drawn over its frames, in place of the surface the target has, if
    /// any: on the same backend where it can present to `surface`, and on a
    /// new one of the device otherwise, which the target then returns true
    /// for, and which draws no overlay. The renderer on the old backend is
    /// torn down with it, and the target has none until one is made again
    /// on the new backend ([`renderer_or`](Self::renderer_or)).
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for an
    /// offscreen target or when the device can't present to `surface`, and
    /// as [`surface`](Self::surface) does. The target then keeps what it
    /// had.
    pub fn set_surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: Size,
    ) -> Result<bool, Error> {
        if !self.is_presenting() {
            return Err(Error::unsupported("only a surface target presents to another surface"));
        }
        surface.check_area()?;
        let window = surface.host_window();
        let retry = surface.clone_window();
        let new_backend = match each!(&mut self.gfx, gfx => gfx.set_surface(surface, overlay, size)) {
            Ok(()) => false,
            Err(error) if error.kind == mln_util::ErrorKind::Unsupported => {
                let surface = retry.ok_or(error)?;
                self.gfx = surface_backend(&self.device, surface, None, size)?;
                true
            }
            Err(error) => return Err(error),
        };
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
        #[cfg(all(feature = "d3d12", windows))]
        SurfaceKind::D3d12Composition { .. } => {
            Device::with_backend(super::target::BackendType::D3D12, DeviceOptions::default())
        }
        // A WebGPU build asked the browser for a device before the build.
        #[cfg(all(target_family = "wasm", feature = "gl"))]
        SurfaceKind::WebCanvas { .. } => Ok(Device::WebGl),
        #[cfg(all(target_family = "wasm", not(feature = "gl")))]
        SurfaceKind::WebCanvas { .. } => Err(Error::invalid_state("the browser's WebGPU device isn't there yet")),
    }
}
