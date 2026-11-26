//! A render target built on the thread that renders: a [`RenderTarget`]'s
//! device and surface made into the backend's objects, which belong to that
//! thread.
//!
//! [`RenderTarget`]: super::RenderTarget

// The matches over the backends' enums list a variant per backend that the
// build's features and target have, so whether their `_` arm is unreachable,
// or matches a single variant, and whether a `let` of one variant is
// irrefutable, depends on the build.
#![allow(
    unreachable_patterns,
    irrefutable_let_patterns,
    clippy::match_wildcard_for_single_variants,
    reason = "the backends' variants depend on the build's features and target"
)]
#![cfg_attr(
    all(target_family = "wasm", not(feature = "webgpu")),
    expect(dead_code, reason = "WebGL2 renders only into a canvas, whose target makes its backend")
)]

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
#[cfg(not(target_family = "wasm"))]
use mln_gfx::offscreen_target::OffscreenTarget;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::ring::{self, Exporter, RingBackend};
use mln_gfx::{
    ConsumerSync, ExportKind, ExportedFrame, ExportedRing, FrameImage, FrameTarget, GpuSync, Handoff, RendererBackend,
    SyncKind, Window,
};
use mln_renderer::{Renderer, UpdateParameters};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::size::Size;

use super::target::{
    BorrowedRing, Device, DeviceOptions, FrameTexture, RingKind, Surface, SurfaceKind, SurfaceOverlay,
};
use super::{gfx_error, gfx_size};

/// A target of the backend `B` that a session draws into.
type Target<B> = Box<dyn FrameTarget<Gpu = <B as RendererBackend>::Gpu>>;

/// Frames given back to the host: a ring of its textures, on the backend
/// `B`.
trait Borrowed<B: LocalBackend>: FrameTarget<Gpu = B::Gpu> {
    fn select(&mut self, slot: usize);
    fn release(&mut self, gpu: &mut B::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), mln_gfx::Error>;
    fn discard(&mut self, gpu: &mut B::Gpu, slot: usize);
    /// Renders into `textures` from now on, and returns the sync after
    /// which the host may use the textures that it let go of.
    fn replace(&mut self, gpu: &mut B::Gpu, textures: B::RingTextures) -> Result<GpuSync, mln_gfx::Error>;
    /// Lets go of its textures, and returns the sync after which the host
    /// may use them. It keeps what the sync names until the next release or
    /// replacement, or until it goes, and draws no frame until then.
    fn release_textures(&mut self, gpu: &mut B::Gpu) -> Result<GpuSync, mln_gfx::Error>;
}

/// Frames queued for the host: a ring of the target's own images, which it
/// exports, on the backend `B`.
trait Exported<B: RendererBackend>: FrameTarget<Gpu = B::Gpu> {
    fn select(&mut self, slot: usize);
    fn release(&mut self, gpu: &mut B::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), mln_gfx::Error>;
    fn discard(&mut self, gpu: &mut B::Gpu, slot: usize);
    fn resize_images(&mut self, backend: &mut B, size: mln_gfx::Size, held: &[usize]) -> Result<bool, mln_gfx::Error>;
    fn size(&self) -> mln_gfx::Size;
    fn image(&self, slot: usize) -> Option<ExportedFrame>;
}

impl<R: RingBackend + 'static> Borrowed<R::Renderer> for ring::BorrowedTarget<R>
where
    R::Renderer: LocalBackend<RingTextures = R::Textures>,
{
    fn select(&mut self, slot: usize) {
        ring::BorrowedTarget::select(self, slot);
    }
    fn release(&mut self, gpu: &mut R::Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), mln_gfx::Error> {
        ring::BorrowedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut R::Gpu, slot: usize) {
        ring::BorrowedTarget::discard(self, gpu, slot);
    }
    fn replace(&mut self, gpu: &mut R::Gpu, textures: R::Textures) -> Result<GpuSync, mln_gfx::Error> {
        ring::BorrowedTarget::replace(self, gpu, textures)
    }
    fn release_textures(&mut self, gpu: &mut R::Gpu) -> Result<GpuSync, mln_gfx::Error> {
        ring::BorrowedTarget::release_textures(self, gpu)
    }
}

#[cfg(all(feature = "webgpu", not(target_family = "wasm")))]
impl Borrowed<mln_webgpu::RendererBackend> for mln_webgpu::BorrowedTarget {
    fn select(&mut self, slot: usize) {
        mln_webgpu::BorrowedTarget::select(self, slot);
    }
    fn release(
        &mut self,
        _gpu: &mut mln_webgpu::WebGpu,
        slot: usize,
        consumer: ConsumerSync,
    ) -> Result<(), mln_gfx::Error> {
        mln_webgpu::BorrowedTarget::release(self, slot, consumer)
    }
    fn discard(&mut self, _gpu: &mut mln_webgpu::WebGpu, slot: usize) {
        mln_webgpu::BorrowedTarget::discard(self, slot);
    }
    fn replace(
        &mut self,
        gpu: &mut mln_webgpu::WebGpu,
        textures: mln_webgpu::BorrowedTextures,
    ) -> Result<GpuSync, mln_gfx::Error> {
        mln_webgpu::BorrowedTarget::replace(self, gpu, textures)
    }
    fn release_textures(&mut self, gpu: &mut mln_webgpu::WebGpu) -> Result<GpuSync, mln_gfx::Error> {
        mln_webgpu::BorrowedTarget::release_textures(self, gpu)
    }
}

#[cfg(all(feature = "gl", not(target_family = "wasm")))]
impl Borrowed<mln_gl::RendererBackend> for mln_gl::BorrowedTarget {
    fn select(&mut self, slot: usize) {
        mln_gl::BorrowedTarget::select(self, slot);
    }
    fn release(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize, consumer: ConsumerSync) -> Result<(), mln_gfx::Error> {
        mln_gl::BorrowedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize) {
        mln_gl::BorrowedTarget::discard(self, gpu, slot);
    }
    fn replace(
        &mut self,
        gpu: &mut mln_gl::GlGpu,
        textures: mln_gl::BorrowedTextures,
    ) -> Result<GpuSync, mln_gfx::Error> {
        mln_gl::BorrowedTarget::replace(self, gpu, textures)
    }
    fn release_textures(&mut self, gpu: &mut mln_gl::GlGpu) -> Result<GpuSync, mln_gfx::Error> {
        mln_gl::BorrowedTarget::release_textures(self, gpu)
    }
}

impl<E: Exporter> Exported<<E::Ring as RingBackend>::Renderer> for ring::ExportedTarget<E> {
    fn select(&mut self, slot: usize) {
        ring::ExportedTarget::select(self, slot);
    }
    fn release(
        &mut self,
        gpu: &mut <E::Ring as RingBackend>::Gpu,
        slot: usize,
        consumer: ConsumerSync,
    ) -> Result<(), mln_gfx::Error> {
        ring::ExportedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut <E::Ring as RingBackend>::Gpu, slot: usize) {
        ring::ExportedTarget::discard(self, gpu, slot);
    }
    fn resize_images(
        &mut self,
        backend: &mut <E::Ring as RingBackend>::Renderer,
        size: mln_gfx::Size,
        held: &[usize],
    ) -> Result<bool, mln_gfx::Error> {
        ring::ExportedTarget::resize(self, backend, size, held)
    }
    fn size(&self) -> mln_gfx::Size {
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
    fn release(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize, consumer: ConsumerSync) -> Result<(), mln_gfx::Error> {
        mln_gl::ExportedTarget::release(self, gpu, slot, consumer)
    }
    fn discard(&mut self, gpu: &mut mln_gl::GlGpu, slot: usize) {
        mln_gl::ExportedTarget::discard(self, gpu, slot);
    }
    fn resize_images(
        &mut self,
        backend: &mut mln_gl::RendererBackend,
        size: mln_gfx::Size,
        held: &[usize],
    ) -> Result<bool, mln_gfx::Error> {
        mln_gl::ExportedTarget::resize(self, &mut backend.gpu(), size, held)
    }
    fn size(&self) -> mln_gfx::Size {
        mln_gl::ExportedTarget::size(self)
    }
    fn image(&self, slot: usize) -> Option<ExportedFrame> {
        mln_gl::ExportedTarget::image(self, slot)
    }
}

/// What a [`BackendGfx`] draws into, which it makes on its backend.
enum FramesInit<'a> {
    Offscreen(mln_gfx::Size),
    Borrowed(BorrowedRing, SyncKind),
    Exported { device: &'a Device, ring: &'a ExportedRing, kind: ExportKind, size: mln_gfx::Size, producer: SyncKind },
    Surface(Surface, Option<SurfaceOverlay>, mln_gfx::Size),
}

fn no_ring() -> Error {
    Error::unsupported("the session's backend renders into no ring of this kind")
}

fn no_surface() -> Error {
    Error::unsupported("the session's backend presents to no surface of this kind")
}

/// A backend of the build, and the targets that the session makes on it.
/// A kind of target that a backend has none of fails with `Unsupported`.
trait LocalBackend: RendererBackend + Sized {
    /// The host's textures of a ring that the backend renders into.
    type RingTextures;

    /// The backend's textures of `ring`, `None` for another backend's.
    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures>;

    /// The backend, by its type.
    fn by_type(&self) -> BackendRef<'_>;

    /// A target on the backend that renders into the ring of `textures`,
    /// handing frames over with sync of the kind `producer`.
    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        let _ = (textures, producer);
        Err(no_ring())
    }

    /// A target on the backend that renders into a ring of `ring.depth`
    /// images of `size` pixels, which it exports as `kind`, and hands frames
    /// over with sync of the kind `producer`.
    fn exported(
        &mut self,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: mln_gfx::Size,
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
        size: mln_gfx::Size,
    ) -> Result<Target<Self>, Error> {
        let _ = (surface, overlay, size);
        Err(no_surface())
    }
}

#[cfg(feature = "webgpu")]
impl LocalBackend for mln_webgpu::RendererBackend {
    #[cfg(not(target_family = "wasm"))]
    type RingTextures = mln_webgpu::BorrowedTextures;
    #[cfg(target_family = "wasm")]
    type RingTextures = std::convert::Infallible;

    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures> {
        match ring {
            #[cfg(not(target_family = "wasm"))]
            RingKind::WebGpu(textures) => Some(textures),
            _ => None,
        }
    }

    fn by_type(&self) -> BackendRef<'_> {
        BackendRef::WebGpu(self)
    }

    #[cfg(not(target_family = "wasm"))]
    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        Ok(Box::new(mln_webgpu::BorrowedTarget::new(self, textures, producer).map_err(gfx_error)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<Target<Self>, Error> {
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
        let mut target = mln_webgpu::SurfaceTarget::new(self, target, size, options).map_err(gfx_error)?;
        target.set_overlay(overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
impl LocalBackend for mln_mtl::RendererBackend {
    type RingTextures = mln_mtl::BorrowedTextures;

    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures> {
        if let RingKind::Metal(textures) = ring { Some(textures) } else { None }
    }

    fn by_type(&self) -> BackendRef<'_> {
        BackendRef::Metal(self)
    }

    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        Ok(Box::new(mln_mtl::BorrowedTarget::new(self, textures, producer).map_err(gfx_error)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: mln_gfx::Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_mtl::ExportedTarget::new(self, ring, kind, size, producer).map_err(gfx_error)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<Target<Self>, Error> {
        let layer = match surface.kind {
            SurfaceKind::MetalLayer(layer) => layer,
            SurfaceKind::Window(window) => mln_mtl::Layer::of_window(&*window).map_err(gfx_error)?,
            _ => return Err(no_surface()),
        };
        let overlay =
            overlay_of(
                overlay,
                |overlay| {
                    if let SurfaceOverlay::Metal(overlay) = overlay { Ok(overlay) } else { Err(overlay) }
                },
            )?;
        let mut target = mln_mtl::SurfaceTarget::new(self, layer, size, surface.cap_frame_rate);
        target.set_overlay(overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
impl LocalBackend for mln_vulkan::RendererBackend {
    type RingTextures = mln_vulkan::BorrowedImages;

    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures> {
        if let RingKind::Vulkan(images) = ring { Some(images) } else { None }
    }

    fn by_type(&self) -> BackendRef<'_> {
        BackendRef::Vulkan(self)
    }

    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        Ok(Box::new(mln_vulkan::BorrowedTarget::new(self, textures, producer).map_err(gfx_error)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: mln_gfx::Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_vulkan::ExportedTarget::new(self, ring, kind, size, producer).map_err(gfx_error)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<Target<Self>, Error> {
        let overlay = vulkan_overlay(overlay)?;
        let mut target = match surface.kind {
            SurfaceKind::Window(window) => {
                mln_vulkan::SurfaceTarget::new(self, window, size, surface.cap_frame_rate).map_err(gfx_error)?
            }
            SurfaceKind::Vulkan(host) => {
                mln_vulkan::SurfaceTarget::on_host_surface(self, host, size, surface.cap_frame_rate)
                    .map_err(gfx_error)?
            }
            _ => return Err(no_surface()),
        };
        target.set_overlay(self.gpu(), overlay);
        Ok(Box::new(target))
    }
}

#[cfg(all(feature = "d3d12", windows))]
impl LocalBackend for mln_d3d12::RendererBackend {
    type RingTextures = mln_d3d12::BorrowedTextures;

    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures> {
        if let RingKind::D3d12(textures) = ring { Some(textures) } else { None }
    }

    fn by_type(&self) -> BackendRef<'_> {
        BackendRef::D3d12(self)
    }

    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        Ok(Box::new(mln_d3d12::BorrowedTarget::new(self, textures, producer).map_err(gfx_error)?))
    }

    fn exported(
        &mut self,
        _device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: mln_gfx::Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        Ok(Box::new(mln_d3d12::ExportedTarget::new(self, ring, kind, size, producer).map_err(gfx_error)?))
    }

    fn surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<Target<Self>, Error> {
        let overlay = || {
            overlay_of(overlay, |overlay| match overlay {
                SurfaceOverlay::D3d12(overlay) => Ok(overlay),
                other => Err(other),
            })
        };
        let (mut target, overlay) = match surface.kind {
            SurfaceKind::Window(window) => {
                let overlay = overlay()?;
                (mln_d3d12::SurfaceTarget::new(self, window, size).map_err(gfx_error)?, overlay)
            }
            SurfaceKind::D3d12Composition { composition, thread } => {
                if composition.is_swap_chain_panel() && thread != std::thread::current().id() {
                    return Err(Error::unsupported(
                        "a SwapChainPanel takes its swap chain on its UI thread, which made the surface: attach it \
                         with Driver::CallerThread, serviced there",
                    ));
                }
                let overlay = overlay()?;
                (mln_d3d12::SurfaceTarget::for_composition(self, composition, size).map_err(gfx_error)?, overlay)
            }
            _ => return Err(no_surface()),
        };
        target.set_overlay(self.gpu(), overlay);
        Ok(Box::new(target))
    }
}

#[cfg(feature = "gl")]
impl LocalBackend for mln_gl::RendererBackend {
    #[cfg(not(target_family = "wasm"))]
    type RingTextures = mln_gl::BorrowedTextures;
    #[cfg(target_family = "wasm")]
    type RingTextures = std::convert::Infallible;

    fn ring_textures(ring: RingKind) -> Option<Self::RingTextures> {
        match ring {
            #[cfg(not(target_family = "wasm"))]
            RingKind::Gl(textures) => Some(textures),
            _ => None,
        }
    }

    fn by_type(&self) -> BackendRef<'_> {
        BackendRef::Gl(self)
    }

    #[cfg(not(target_family = "wasm"))]
    fn borrowed(&mut self, textures: Self::RingTextures, producer: SyncKind) -> Result<Box<dyn Borrowed<Self>>, Error> {
        Ok(Box::new(mln_gl::BorrowedTarget::new(textures, producer).map_err(gfx_error)?))
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    fn exported(
        &mut self,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: mln_gfx::Size,
        producer: SyncKind,
    ) -> Result<Box<dyn Exported<Self>>, Error> {
        // Irrefutable in a build with OpenGL alone, which has no other kind
        // of device, and refutable in one with other backends.
        let Device::Gl(device) = device else {
            return Err(Error::unsupported("an OpenGL backend renders on an OpenGL device"));
        };
        Ok(Box::new(mln_gl::ExportedTarget::new(self, device, ring, kind, size, producer).map_err(gfx_error)?))
    }
}

/// What a session draws with and into, whatever its backend: the methods of
/// [`LocalTarget`] that reach the backend, each one dynamic call.
trait Gfx {
    fn backend(&self) -> BackendRef<'_>;
    fn frames(&self) -> FramesKind;
    fn renderer(&self) -> Option<&Renderer>;
    fn renderer_mut(&mut self) -> Option<&mut Renderer>;
    fn renderer_or(&mut self, make: &mut dyn FnMut(&dyn Gpu) -> Renderer) -> &mut Renderer;
    fn with_gpu(&mut self, f: &mut dyn FnMut(&mut dyn Gpu, Option<&mut Renderer>));
    fn tear_down_renderer(&mut self) -> Option<RenderingStats>;
    fn render(&mut self, update: &Arc<UpdateParameters>) -> Option<Handoff>;
    fn ready_for_frame(&mut self) -> bool;
    fn with_frame(&mut self, draw: &mut dyn FnMut(&mut dyn Gpu, &FrameImage));
    fn select_slot(&mut self, slot: usize) -> Result<(), Error>;
    fn release_slot(&mut self, slot: usize, consumer: ConsumerSync) -> Result<(), Error>;
    fn discard_slot(&mut self, slot: usize) -> Result<(), Error>;
    fn exported_size(&self) -> Option<mln_gfx::Size>;
    fn resize_exported(&mut self, size: mln_gfx::Size, held: &[usize]) -> Result<bool, Error>;
    fn exported_image(&self, slot: usize) -> Option<ExportedFrame>;
    fn set_ring(&mut self, ring: BorrowedRing) -> Result<GpuSync, Error>;
    fn resize(&mut self, size: mln_gfx::Size);
    fn read_image(&mut self) -> Result<PremultipliedImage, Error>;
    fn release_surface(&mut self) -> Result<Option<GpuSync>, Error>;
    fn set_surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<(), Error>;
}

/// What a session draws into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FramesKind {
    Offscreen,
    Presenting,
    Borrowed { released: bool },
    Exported,
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
enum Frames<B: LocalBackend> {
    /// None in the browser, which renders into canvases only.
    #[cfg(not(target_family = "wasm"))]
    Offscreen(OffscreenTarget<B::Gpu>),
    /// `None` once the surface is released.
    Presenting(Option<Target<B>>),
    /// `released` once the target let go of the ring's textures, which it
    /// keeps without them until it gets new ones.
    Borrowed {
        target: Box<dyn Borrowed<B>>,
        released: bool,
        producer: SyncKind,
    },
    Exported(Box<dyn Exported<B>>),
}

impl<B: LocalBackend> Frames<B> {
    /// The target to draw into, or `None` while there is no surface or ring.
    fn target(&mut self) -> Option<&mut dyn FrameTarget<Gpu = B::Gpu>> {
        match self {
            #[cfg(not(target_family = "wasm"))]
            Frames::Offscreen(target) => Some(target),
            Frames::Presenting(target) => Some(&mut **target.as_mut()?),
            Frames::Borrowed { target, released: false, .. } => Some(&mut **target),
            Frames::Borrowed { released: true, .. } => None,
            Frames::Exported(target) => Some(&mut **target),
        }
    }

    /// The ring that the frames draw into.
    fn ring(&mut self) -> Result<&mut dyn Borrowed<B>, Error> {
        match self {
            Frames::Borrowed { target, released: false, .. } => Ok(&mut **target),
            Frames::Borrowed { released: true, .. } => Err(Error::invalid_state("the ring was released")),
            _ => Err(Error::unsupported("the target has no ring")),
        }
    }
}

/// `backend`, and what `init` says it draws into, made on it.
fn gfx<B: LocalBackend>(mut backend: B, init: FramesInit<'_>) -> Result<Box<dyn Gfx>, Error> {
    let frames = match init {
        #[cfg(not(target_family = "wasm"))]
        FramesInit::Offscreen(size) => Frames::Offscreen(OffscreenTarget::new(size)),
        #[cfg(target_family = "wasm")]
        FramesInit::Offscreen(size) => {
            let _ = size;
            return Err(Error::unsupported("the browser build renders into canvases only"));
        }
        FramesInit::Borrowed(ring, producer) => {
            let textures = B::ring_textures(ring.kind).ok_or_else(no_ring)?;
            Frames::Borrowed { target: backend.borrowed(textures, producer)?, released: false, producer }
        }
        FramesInit::Exported { device, ring, kind, size, producer } => {
            Frames::Exported(backend.exported(device, ring, kind, size, producer)?)
        }
        FramesInit::Surface(surface, overlay, size) => {
            Frames::Presenting(Some(backend.surface(surface, overlay, size)?))
        }
    };
    Ok(Box::new(BackendGfx { backend, frames, renderer: None }))
}

/// `backend`, presenting with `target`.
#[cfg_attr(
    not(any(feature = "gl", all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))),
    expect(dead_code, reason = "only Vulkan and OpenGL make backends of a surface's own")
)]
fn presenting<B: LocalBackend>(backend: B, target: impl FrameTarget<Gpu = B::Gpu> + 'static) -> Box<dyn Gfx> {
    Box::new(BackendGfx { backend, frames: Frames::Presenting(Some(Box::new(target))), renderer: None })
}

impl<B: LocalBackend> Gfx for BackendGfx<B> {
    fn backend(&self) -> BackendRef<'_> {
        self.backend.by_type()
    }

    fn frames(&self) -> FramesKind {
        match &self.frames {
            #[cfg(not(target_family = "wasm"))]
            Frames::Offscreen(_) => FramesKind::Offscreen,
            Frames::Presenting(_) => FramesKind::Presenting,
            Frames::Borrowed { released, .. } => FramesKind::Borrowed { released: *released },
            Frames::Exported(_) => FramesKind::Exported,
        }
    }

    fn renderer(&self) -> Option<&Renderer> {
        self.renderer.as_ref()
    }

    fn renderer_mut(&mut self) -> Option<&mut Renderer> {
        self.renderer.as_mut()
    }

    fn renderer_or(&mut self, make: &mut dyn FnMut(&dyn Gpu) -> Renderer) -> &mut Renderer {
        let BackendGfx { backend, renderer, .. } = self;
        renderer.get_or_insert_with(|| make(&*backend.gpu()))
    }

    fn with_gpu(&mut self, f: &mut dyn FnMut(&mut dyn Gpu, Option<&mut Renderer>)) {
        f(&mut *self.backend.gpu(), self.renderer.as_mut());
    }

    fn tear_down_renderer(&mut self) -> Option<RenderingStats> {
        let renderer = self.renderer.take()?;
        Some(renderer.tear_down(&mut *self.backend.gpu()))
    }

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

    fn ready_for_frame(&mut self) -> bool {
        let Some(target) = self.frames.target() else { return false };
        target.begin_frame(&mut self.backend.gpu()).is_some()
    }

    fn with_frame(&mut self, draw: &mut dyn FnMut(&mut dyn Gpu, &FrameImage)) {
        let Some(target) = self.frames.target() else { return };
        let mut gpu = self.backend.gpu();
        if let Some(image) = target.begin_frame(&mut gpu) {
            draw(&mut *gpu, &image);
        }
    }

    fn select_slot(&mut self, slot: usize) -> Result<(), Error> {
        match &mut self.frames {
            Frames::Exported(target) => target.select(slot),
            frames => frames.ring()?.select(slot),
        }
        Ok(())
    }

    fn release_slot(&mut self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Exported(target) => target.release(gpu, slot, consumer),
            frames => frames.ring()?.release(gpu, slot, consumer),
        }
        .map_err(gfx_error)
    }

    fn discard_slot(&mut self, slot: usize) -> Result<(), Error> {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Exported(target) => target.discard(gpu, slot),
            frames => frames.ring()?.discard(gpu, slot),
        }
        Ok(())
    }

    fn exported_size(&self) -> Option<mln_gfx::Size> {
        match &self.frames {
            Frames::Exported(target) => Some(target.size()),
            _ => None,
        }
    }

    fn resize_exported(&mut self, size: mln_gfx::Size, held: &[usize]) -> Result<bool, Error> {
        match &mut self.frames {
            Frames::Exported(target) => target.resize_images(&mut self.backend, size, held).map_err(gfx_error),
            _ => Err(Error::unsupported("the target exports no ring")),
        }
    }

    fn exported_image(&self, slot: usize) -> Option<ExportedFrame> {
        match &self.frames {
            Frames::Exported(target) => target.image(slot),
            _ => None,
        }
    }

    fn set_ring(&mut self, ring: BorrowedRing) -> Result<GpuSync, Error> {
        let Frames::Borrowed { target, released, producer } = &mut self.frames else {
            return Err(Error::unsupported("the target has no ring"));
        };
        let textures = B::ring_textures(ring.kind).ok_or_else(no_ring)?;
        if !*released {
            return target.replace(&mut self.backend.gpu(), textures).map_err(gfx_error);
        }
        // The released target, which kept what its release's sync names
        // until now, goes once its frames are done.
        let next = self.backend.borrowed(textures, *producer)?;
        std::mem::replace(target, next).tear_down(&mut self.backend.gpu());
        *released = false;
        Ok(GpuSync::of(SyncKind::CpuComplete))
    }

    fn resize(&mut self, size: mln_gfx::Size) {
        if let Some(target) = self.frames.target() {
            target.resize(&mut self.backend.gpu(), size);
        }
    }

    fn read_image(&mut self) -> Result<PremultipliedImage, Error> {
        match &mut self.frames {
            #[cfg(not(target_family = "wasm"))]
            Frames::Offscreen(target) => {
                let pixels = target.read_image(&mut self.backend.gpu()).map_err(gfx_error)?;
                Ok(PremultipliedImage::from_vec(Size::new(pixels.size.width, pixels.size.height), pixels.data))
            }
            Frames::Presenting(_) => Err(Error::unsupported("a surface session can't read its frames back")),
            Frames::Borrowed { .. } | Frames::Exported(_) => {
                Err(Error::unsupported("a ring's frames are the host's to read"))
            }
        }
    }

    fn release_surface(&mut self) -> Result<Option<GpuSync>, Error> {
        let gpu = &mut *self.backend.gpu();
        match &mut self.frames {
            Frames::Presenting(target) => {
                if let Some(mut target) = target.take() {
                    target.tear_down(gpu);
                }
                Ok(None)
            }
            Frames::Borrowed { released: true, .. } => Ok(Some(GpuSync::of(SyncKind::CpuComplete))),
            Frames::Borrowed { target, released, .. } => {
                let sync = target.release_textures(gpu).map_err(gfx_error)?;
                *released = true;
                Ok(Some(sync))
            }
            #[cfg(not(target_family = "wasm"))]
            Frames::Offscreen(_) => Ok(None),
            Frames::Exported(_) => Ok(None),
        }
    }

    fn set_surface(
        &mut self,
        surface: Surface,
        overlay: Option<SurfaceOverlay>,
        size: mln_gfx::Size,
    ) -> Result<(), Error> {
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

impl<B: LocalBackend> Drop for BackendGfx<B> {
    fn drop(&mut self) {
        let mut gpu = self.backend.gpu();
        if let Some(renderer) = self.renderer.take() {
            renderer.tear_down(&mut *gpu);
        }
        // A released ring keeps the objects that its release's sync names.
        if let Frames::Borrowed { target, .. } = &mut self.frames {
            target.tear_down(&mut gpu);
        } else if let Some(target) = self.frames.target() {
            target.tear_down(&mut gpu);
        }
    }
}

/// A backend on `device`, and what `init` says it draws into.
fn new_gfx(device: &Device, init: FramesInit<'_>) -> Result<Box<dyn Gfx>, Error> {
    match device {
        #[cfg(feature = "webgpu")]
        Device::WebGpu(device) => gfx(mln_webgpu::RendererBackend::new(device), init),
        #[cfg(all(feature = "metal", target_vendor = "apple"))]
        Device::Metal(device) => gfx(mln_mtl::RendererBackend::new(device), init),
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        Device::Vulkan(device) => gfx(mln_vulkan::RendererBackend::new(device).map_err(gfx_error)?, init),
        #[cfg(all(feature = "d3d12", windows))]
        Device::D3d12(device) => {
            gfx(mln_d3d12::RendererBackend::new(device).map_err(|error| Error::native(error.to_string()))?, init)
        }
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        Device::Gl(device) => gfx(mln_gl::RendererBackend::new(device).map_err(gfx_error)?, init),
        #[cfg(all(feature = "gl", target_family = "wasm"))]
        Device::WebGl => {
            drop(init);
            Err(Error::unsupported("WebGL2 renders only into a canvas, with the canvas's own context"))
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

#[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
fn vulkan_overlay(overlay: Option<SurfaceOverlay>) -> Result<Option<Box<dyn mln_vulkan::Overlay>>, Error> {
    overlay_of(overlay, |overlay| match overlay {
        SurfaceOverlay::Vulkan(overlay) => Ok(overlay),
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
    size: mln_gfx::Size,
) -> Result<Box<dyn Gfx>, Error> {
    let cap_frame_rate = surface.cap_frame_rate;
    match (device, surface.kind) {
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), SurfaceKind::Window(window)) => {
            let overlay = vulkan_overlay(overlay)?;
            let (mut backend, mut target) =
                mln_vulkan::SurfaceTarget::with_backend(device, window, size, cap_frame_rate).map_err(gfx_error)?;
            target.set_overlay(backend.gpu(), overlay);
            Ok(presenting(backend, target))
        }
        #[cfg(all(feature = "vulkan", not(any(target_vendor = "apple", target_family = "wasm"))))]
        (Device::Vulkan(device), kind @ SurfaceKind::Vulkan(_)) => gfx(
            mln_vulkan::RendererBackend::presenting(device).map_err(gfx_error)?,
            FramesInit::Surface(Surface { kind, cap_frame_rate }, overlay, size),
        ),
        #[cfg(all(feature = "gl", not(target_family = "wasm")))]
        (Device::Gl(device), SurfaceKind::Gl(host)) => {
            if overlay.is_some() {
                return Err(Error::unsupported("a host's EGL surface takes no overlay"));
            }
            let (backend, target) = mln_gl::HostSurfaceTarget::with_backend(device, host, size).map_err(gfx_error)?;
            Ok(presenting(backend, target))
        }
        #[cfg(all(feature = "gl", any(windows, all(unix, not(target_vendor = "apple")))))]
        (Device::Gl(_), SurfaceKind::Window(window)) => {
            let overlay = overlay_of(overlay, |overlay| match overlay {
                SurfaceOverlay::Gl(overlay) => Ok(overlay),
                other => Err(other),
            })?;
            let (mut backend, mut target) =
                mln_gl::SurfaceTarget::with_backend(window, size, cap_frame_rate).map_err(gfx_error)?;
            target.set_overlay(&mut backend.gpu(), overlay);
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
            let (backend, target) = mln_gl::CanvasTarget::with_backend(canvas, size).map_err(gfx_error)?;
            Ok(presenting(backend, target))
        }
        (_, kind) => new_gfx(device, FramesInit::Surface(Surface { kind, cap_frame_rate }, overlay, size)),
    }
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
    gfx: Box<dyn Gfx>,
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
        let gfx = new_gfx(&device, FramesInit::Offscreen(gfx_size(size)))?;
        Ok(LocalTarget { device, gfx, window: None })
    }

    /// A target of `size` pixels that presents to `surface`, with `overlay`
    /// drawn over its frames, on `device` or, when that is `None`, on the
    /// device the surface's kind takes
    /// ([`RenderTarget::Surface`](super::RenderTarget::Surface)).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a
    /// surface that the host made without the host's device, with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) for a window or
    /// surface without an area to present to, such as a minimized window,
    /// with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) when the device
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
        let gfx = surface_backend(&device, surface, overlay, gfx_size(size))?;
        Ok(LocalTarget { device, gfx, window })
    }

    /// A target of `size` pixels on `device` that renders into the host's
    /// `ring`, and hands each frame over with sync of the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) when the
    /// device can't render into the ring with `producer`, and with the
    /// backend's error.
    pub fn borrowed(device: Device, ring: BorrowedRing, producer: SyncKind) -> Result<LocalTarget, Error> {
        let gfx = new_gfx(&device, FramesInit::Borrowed(ring, producer))?;
        Ok(LocalTarget { device, gfx, window: None })
    }

    /// A target on `device` that renders into a ring of `ring.depth` images
    /// of `size` pixels, which it exports as `kind`, one that
    /// [`Device::export_plan`] picked, and hands each frame over with sync of
    /// the kind `producer`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) when the
    /// device can't export `kind` or hand frames over with `producer`, and
    /// with the backend's error.
    pub fn exported(
        device: Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<LocalTarget, Error> {
        let gfx =
            new_gfx(&device, FramesInit::Exported { device: &device, ring, kind, size: gfx_size(size), producer })?;
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
        self.gfx.backend()
    }

    /// Runs `f` with the backend's `Gpu`, as when the host waits for the
    /// frames drawn so far.
    pub fn with_gpu(&mut self, mut f: impl FnMut(&mut dyn Gpu)) {
        self.gfx.with_gpu(&mut |gpu, _| f(gpu));
    }

    /// The renderer that draws into the target, which `make` makes with the
    /// backend's `Gpu` unless the target has one: the renderer draws with
    /// that `Gpu` only.
    pub fn renderer_or(&mut self, mut make: impl FnMut(&dyn Gpu) -> Renderer) -> &mut Renderer {
        self.gfx.renderer_or(&mut make)
    }

    /// The renderer that draws into the target, if it has one.
    #[must_use]
    pub fn renderer(&self) -> Option<&Renderer> {
        self.gfx.renderer()
    }

    /// The renderer that draws into the target, if it has one.
    pub fn renderer_mut(&mut self) -> Option<&mut Renderer> {
        self.gfx.renderer_mut()
    }

    /// Runs `f` with the target's renderer and the backend's `Gpu`, which the
    /// renderer's calls that make, draw or free its GPU objects take, or
    /// returns `None` without a renderer.
    pub fn with_renderer<R>(&mut self, f: impl FnOnce(&mut Renderer, &mut dyn Gpu) -> R) -> Option<R> {
        self.gfx.renderer()?;
        let (mut f, mut result) = (Some(f), None);
        self.gfx.with_gpu(&mut |gpu, renderer| {
            if let (Some(renderer), Some(f)) = (renderer, f.take()) {
                result = Some(f(renderer, gpu));
            }
        });
        result
    }

    /// Tears the renderer down on the backend's `Gpu`, if the target has
    /// one, and returns its rendering stats as it leaves them.
    pub fn tear_down_renderer(&mut self) -> Option<RenderingStats> {
        self.gfx.tear_down_renderer()
    }

    /// Draws `update` with the target's renderer into a frame: the target
    /// begins the frame, the renderer draws into it, and the target ends
    /// it. Returns where the frame went, or `None` when nothing was drawn:
    /// there is no renderer, the target had nothing to draw into, or there
    /// was nothing to draw.
    pub fn render(&mut self, update: &Arc<UpdateParameters>) -> Option<Handoff> {
        self.gfx.render(update)
    }

    /// Runs `draw` with the backend's `Gpu` and the textures of the target's
    /// frame, to draw into them alongside the map. The frame is the one the
    /// next render draws into, which the target begins unless it has
    /// begun it. `None` when the target has nothing to draw into now.
    pub fn with_frame<R>(&mut self, draw: impl FnOnce(&mut dyn Gpu, &FrameImage) -> R) -> Option<R> {
        let (mut draw, mut result) = (Some(draw), None);
        self.gfx.with_frame(&mut |gpu, image| result = draw.take().map(|draw| draw(gpu, image)));
        result
    }

    /// Whether the target can take a frame now: its window, if any, has an
    /// area to present to, and it begins a frame, which a surface needs a
    /// texture for, and which the next render draws into.
    pub fn ready_for_frame(&mut self) -> bool {
        self.window_has_area() && self.gfx.ready_for_frame()
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
        self.gfx.frames() == FramesKind::Presenting
    }

    /// Whether frames go to the host in images of the target's own.
    #[must_use]
    pub fn is_exported(&self) -> bool {
        self.gfx.frames() == FramesKind::Exported
    }

    /// Whether frames go to the host's ring.
    #[must_use]
    pub fn is_borrowed(&self) -> bool {
        matches!(self.gfx.frames(), FramesKind::Borrowed { .. })
    }

    /// Whether the target is a ring that let go of its textures.
    #[must_use]
    pub fn ring_released(&self) -> bool {
        self.gfx.frames() == FramesKind::Borrowed { released: true }
    }

    /// Whether frames are read back rather than presented.
    #[must_use]
    pub fn is_offscreen(&self) -> bool {
        self.gfx.frames() == FramesKind::Offscreen
    }

    /// Has the next frame render into the ring's texture at `slot`.
    pub fn select_slot(&mut self, slot: usize) -> Result<(), Error> {
        self.gfx.select_slot(slot)
    }

    /// The host gave the ring's texture at `slot` back: the next frame
    /// rendered into it waits for `consumer`.
    pub fn release_slot(&mut self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        self.gfx.release_slot(slot, consumer)
    }

    /// The host gave up the ring's texture at `slot`, which the target
    /// renders into no more.
    pub fn discard_slot(&mut self, slot: usize) -> Result<(), Error> {
        self.gfx.discard_slot(slot)
    }

    /// The size in pixels of an exported ring's images, or `None` for
    /// another target.
    #[must_use]
    pub fn exported_size(&self) -> Option<Size> {
        self.gfx.exported_size().map(|size| Size::new(size.width, size.height))
    }

    /// Renders an exported ring's next frames into images of `size` pixels.
    /// The frames that the host holds, at `held` slots, keep their images.
    /// Returns whether the ring has new images, which it doesn't when they
    /// have `size` already.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) for a
    /// target that isn't an exported ring, and with the backend's error, when
    /// the ring keeps its images.
    pub fn resize_exported(&mut self, size: Size, held: &[usize]) -> Result<bool, Error> {
        self.gfx.resize_exported(gfx_size(size), held)
    }

    /// The exported image of the frame at `slot`, of the size it has.
    #[must_use]
    pub fn exported_texture(&self, slot: usize) -> Option<FrameTexture> {
        let image = self.gfx.exported_image(slot)?;
        Some(FrameTexture::exported(image, &self.device))
    }

    /// Renders into `ring` from now on, in place of the ring's textures, and
    /// returns the sync after which the host may use the textures that it
    /// let go of: of the target's producer kind, or
    /// [`SyncKind::CpuComplete`] when it had none since a release.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) for a
    /// target that isn't a ring, or a ring of another format, and with the
    /// backend's error. The target then keeps its textures.
    pub(crate) fn set_ring(&mut self, ring: BorrowedRing) -> Result<GpuSync, Error> {
        self.gfx.set_ring(ring)
    }

    /// Draws the next frame at `size` pixels.
    pub fn resize(&mut self, size: Size) {
        self.gfx.resize(gfx_size(size));
    }

    /// The last frame's pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) for a
    /// target that presents its frames, with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) before a frame, and
    /// with [`ErrorKind::Native`](mln_style::error::ErrorKind) where the backend
    /// reads nothing back.
    pub fn read_image(&mut self) -> Result<PremultipliedImage, Error> {
        self.gfx.read_image()
    }

    /// Lets go of a surface target's surface, or of a ring's textures, and
    /// returns, for a ring, the sync after which the host may use them. The
    /// backend and its renderer stay.
    ///
    /// Fails with [`ErrorKind::Native`](mln_style::error::ErrorKind) when a ring
    /// can't submit its work on the textures, which it keeps then.
    pub(crate) fn release_surface(&mut self) -> Result<Option<GpuSync>, Error> {
        let sync = self.gfx.release_surface()?;
        if self.is_presenting() {
            self.window = None;
        }
        Ok(sync)
    }

    /// Presents to `surface` at `size` pixels from now on, with `overlay`
    /// drawn over its frames, in place of the surface the target has, if
    /// any: on the same backend where it can present to `surface`, and on a
    /// new one of the device otherwise, which the target then returns true
    /// for, and which draws no overlay. The renderer on the old backend is
    /// torn down with it, and the target has none until one is made again
    /// on the new backend ([`renderer_or`](Self::renderer_or)).
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) for an
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
        let size = gfx_size(size);
        let new_backend = match self.gfx.set_surface(surface, overlay, size) {
            Ok(()) => false,
            Err(error) if error.kind == mln_style::error::ErrorKind::Unsupported => {
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
