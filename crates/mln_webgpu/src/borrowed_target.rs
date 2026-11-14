// A ring of the host's textures, on the device that renders into them, that
// frames are drawn into and given back to the host. Both sides submit to the
// device's one queue, which orders the host's reads after the frame and the
// next frame after the host's reads.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, TargetFormats};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gpu::from_wgpu_format;
use crate::renderer_backend::RendererBackend;

/// A ring of the host's 2D textures, of `size` pixels and one format, with
/// one layer, one mip level, one sample and `RENDER_ATTACHMENT` usage.
#[derive(Clone, Debug, PartialEq)]
pub struct BorrowedTextures {
    pub textures: Vec<wgpu::Texture>,
    pub size: Size,
    /// The host's textures that frames draw their depth and stencil into
    /// and store, one for each of `textures`, of their size and kind and one
    /// depth and stencil format, or none for depth of the target's own.
    pub depth_textures: Vec<wgpu::Texture>,
}

impl BorrowedTextures {
    /// The kinds of sync with which the host can wait for a frame (the
    /// producer's) and the target for the host's reads (the consumer's):
    /// both share the device's queue.
    #[must_use]
    pub fn sync_kinds() -> (SyncKinds, SyncKinds) {
        let kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::QueueOrder]);
        (kinds, kinds)
    }

    /// The formats of the depth textures that frames draw into on `device`.
    #[must_use]
    pub fn depth_stencil_formats(device: &wgpu::Device) -> DepthStencilFormats {
        let formats = DepthStencilFormats::of(&[DepthStencilFormat::Depth24Stencil8]);
        if device.features().contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
            formats.with(DepthStencilFormat::Depth32FloatStencil8)
        } else {
            formats
        }
    }

    /// Checks the textures against their ring.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// texture of another size than the ring's, of another format than the
    /// first, or that a frame can't render into, for depth textures that
    /// aren't one for each texture, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for depth textures of
    /// no depth and stencil format.
    pub fn check(&self) -> Result<(), Error> {
        let Some(first) = self.textures.first() else {
            return Err(Error::invalid_argument("a ring needs at least one texture"));
        };
        if let Some(depth) = self.depth_textures.first() {
            if self.depth_textures.len() != self.textures.len() {
                return Err(Error::invalid_argument("a ring's depth textures must be one for each of its textures"));
            }
            if depth_stencil_format(depth.format()).is_none() {
                return Err(Error::unsupported("a ring's depth textures must have a depth and stencil format"));
            }
        }
        for texture in self.textures.iter().chain(&self.depth_textures) {
            let size = texture.size();
            if (size.width, size.height) != (self.size.width, self.size.height) {
                return Err(Error::invalid_argument("each texture's size must match the ring's physical size"));
            }
            let kin =
                if texture.format().is_depth_stencil_format() { self.depth_textures.first() } else { Some(first) };
            if Some(texture.format()) != kin.map(wgpu::Texture::format) {
                return Err(Error::invalid_argument("the ring's textures must share a format"));
            }
            if !texture.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
                || texture.dimension() != wgpu::TextureDimension::D2
                || texture.mip_level_count() != 1
                || texture.sample_count() != 1
                || size.depth_or_array_layers != 1
            {
                return Err(Error::invalid_argument(
                    "each texture must be 2D with one layer, mip level and sample, and render-attachment usage",
                ));
            }
        }
        Ok(())
    }

    fn format(&self) -> Option<wgpu::TextureFormat> {
        self.textures.first().map(wgpu::Texture::format)
    }

    fn depth_format(&self) -> Option<wgpu::TextureFormat> {
        self.depth_textures.first().map(wgpu::Texture::format)
    }
}

/// The depth and stencil format that `format` is, if it is one.
#[must_use]
pub fn depth_stencil_format(format: wgpu::TextureFormat) -> Option<DepthStencilFormat> {
    match format {
        wgpu::TextureFormat::Depth24PlusStencil8 => Some(DepthStencilFormat::Depth24Stencil8),
        wgpu::TextureFormat::Depth32FloatStencil8 => Some(DepthStencilFormat::Depth32FloatStencil8),
        _ => None,
    }
}

/// The ring, the texture the next frame draws into, and the target's own
/// depth and stencil texture where the host gave none.
struct Resource {
    device: wgpu::Device,
    textures: RefCell<BorrowedTextures>,
    selected: Cell<usize>,
    depth_stencil_format: wgpu::TextureFormat,
    depth_stencil: RefCell<Option<wgpu::Texture>>,
    /// The slot of the frame begun and its textures, imported into the
    /// backend's `WebGpu`, until the frame ends.
    imports: Cell<Option<(usize, FrameImage)>>,
}

impl Resource {
    /// The depth and stencil texture of the selected slot: the host's, or
    /// the target's own of the ring's size.
    fn depth_stencil(&self) -> wgpu::Texture {
        let textures = self.textures.borrow();
        if let Some(depth) = textures.depth_textures.get(self.selected.get()) {
            return depth.clone();
        }
        let size = textures.size;
        let mut depth_stencil = self.depth_stencil.borrow_mut();
        let made =
            depth_stencil.as_ref().filter(|texture| (texture.width(), texture.height()) == (size.width, size.height));
        if let Some(texture) = made {
            return texture.clone();
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("borrowed ring depth and stencil"),
            size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.depth_stencil_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        *depth_stencil = Some(texture.clone());
        texture
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    resource: Resource,
    producer: SyncKind,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `textures`, which are of its
    /// device, and hands each frame to the host with sync of the kind
    /// `producer`, one of [`BorrowedTextures::sync_kinds`]'s.
    ///
    /// Fails as [`BorrowedTextures::check`] does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// the target can't hand over.
    pub fn new(backend: Rc<RendererBackend>, textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        textures.check()?;
        if !BorrowedTextures::sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("a WebGPU target hands frames over with no {producer:?}")));
        }
        let device = backend.device().device().clone();
        let depth_stencil_format = if device.features().contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
            wgpu::TextureFormat::Depth32FloatStencil8
        } else {
            wgpu::TextureFormat::Depth24PlusStencil8
        };
        let resource = Resource {
            device,
            textures: RefCell::new(textures),
            selected: Cell::new(0),
            depth_stencil_format,
            depth_stencil: RefCell::new(None),
            imports: Cell::new(None),
        };
        Ok(BorrowedTarget { backend, resource, producer })
    }

    /// How many textures the ring has.
    #[must_use]
    pub(crate) fn depth(&self) -> usize {
        self.resource.textures.borrow().textures.len()
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&self, slot: usize) {
        self.resource.selected.set(slot);
    }

    /// The host gave the texture at `slot` back. The host's reads went to
    /// the queue before the next frame does, or have finished.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&self, _slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        match consumer.sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => Ok(()),
            kind => Err(Error::unsupported(format!("the WebGPU target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again.
    pub fn discard(&self, _slot: usize) {}

    /// Draws into `textures` from now on, which must be as many, of the
    /// ring's format.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another format,
    /// and as [`BorrowedTextures::check`] does.
    pub fn replace(&self, textures: BorrowedTextures) -> Result<(), Error> {
        if textures.textures.len() != self.depth() {
            return Err(Error::invalid_argument("a ring keeps its depth"));
        }
        let current = self.resource.textures.borrow();
        if (textures.format(), textures.depth_format()) != (current.format(), current.depth_format()) {
            return Err(Error::unsupported("a ring keeps its formats, and its depth textures or their lack"));
        }
        drop(current);
        textures.check()?;
        // A frame begun on the old ring draws into the new one.
        if let Some((_, image)) = self.resource.imports.take() {
            let mut gpu = self.backend.backend_impl().gpu().borrow_mut();
            for import in [Some(image.color), image.depth_stencil].into_iter().flatten() {
                gpu.destroy_texture(import);
            }
        }
        *self.resource.textures.borrow_mut() = textures;
        Ok(())
    }
}

impl FrameTarget for BorrowedTarget {
    fn begin_frame(&self) -> Option<FrameImage> {
        let resource = &self.resource;
        let slot = resource.selected.get();
        let mut gpu = self.backend.backend_impl().gpu().borrow_mut();
        match resource.imports.take() {
            Some((imported, image)) if imported == slot => {
                resource.imports.set(Some((imported, image)));
                return Some(image);
            }
            // A slot was selected since the frame began.
            Some((_, image)) => {
                for import in [Some(image.color), image.depth_stencil].into_iter().flatten() {
                    gpu.destroy_texture(import);
                }
            }
            None => {}
        }
        let textures = resource.textures.borrow();
        let texture = textures.textures.get(slot)?;
        let depth_stencil = resource.depth_stencil();
        let image = FrameImage {
            size: textures.size,
            color: gpu.import_texture(texture),
            depth_stencil: Some(gpu.import_texture(&depth_stencil)),
            targets: TargetFormats {
                color: from_wgpu_format(texture.format()),
                depth_stencil: from_wgpu_format(depth_stencil.format()),
            },
        };
        resource.imports.set(Some((slot, image)));
        Some(image)
    }

    fn end_frame(&self) -> Handoff {
        let Some((_, image)) = self.resource.imports.take() else {
            return Handoff::Dropped;
        };
        let mut gpu = self.backend.backend_impl().gpu().borrow_mut();
        for import in [Some(image.color), image.depth_stencil].into_iter().flatten() {
            gpu.destroy_texture(import);
        }
        let sync = if self.producer == SyncKind::QueueOrder {
            GpuSync::of(SyncKind::QueueOrder)
        } else {
            if let Err(error) = gpu.finish() {
                Log::error(Event::Render, format!("WebGPU: waiting for a borrowed texture's frame failed: {error}"));
                return Handoff::Dropped;
            }
            GpuSync::of(SyncKind::CpuComplete)
        };
        Handoff::GivenBack { slot: self.resource.selected.get(), sync }
    }
}
