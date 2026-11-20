//! A ring of the host's textures, on the device that renders into them, that
//! frames are drawn into and given back to the host. Both sides submit to the
//! device's one queue, which orders the host's reads after the frame and the
//! next frame after the host's reads.

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, ResourceId, TargetFormats, destroy_unused};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gpu::{WebGpu, from_wgpu_format};
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

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    device: wgpu::Device,
    /// `None` once the target let go of them.
    textures: Option<BorrowedTextures>,
    /// The slot that the next frame draws into.
    selected: usize,
    producer: SyncKind,
    depth_stencil_format: wgpu::TextureFormat,
    /// The target's own depth and stencil texture, where the host gave
    /// none.
    depth_stencil: Option<wgpu::Texture>,
    /// The slot of the frame begun and its textures, imported into the
    /// backend's `WebGpu`, until the frame ends.
    imports: Option<(usize, FrameImage)>,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `textures`, which are of its
    /// device, and hands each frame to the host with sync of the kind
    /// `producer`, one of [`BorrowedTextures::sync_kinds`]'s.
    ///
    /// Fails as [`BorrowedTextures::check`] does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// the target can't hand over.
    pub fn new(backend: &RendererBackend, textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
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
        Ok(BorrowedTarget {
            device,
            textures: Some(textures),
            selected: 0,
            producer,
            depth_stencil_format,
            depth_stencil: None,
            imports: None,
        })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.selected = slot;
    }

    /// The host gave the texture at `slot` back. The host's reads went to
    /// the queue before the next frame does, or have finished.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&mut self, _slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        match consumer.sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => Ok(()),
            kind => Err(Error::unsupported(format!("the WebGPU target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again.
    pub fn discard(&mut self, _slot: usize) {}

    /// Draws into `textures` from now on, which must be as many, of the
    /// ring's format. Returns the sync after which the host may use the old
    /// textures, as [`release_textures`](Self::release_textures) does.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another format,
    /// with [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the target
    /// let go of its textures, as [`BorrowedTextures::check`] does, and as
    /// [`release_textures`](Self::release_textures) does. The target then
    /// keeps its textures.
    pub fn replace(&mut self, gpu: &mut WebGpu, textures: BorrowedTextures) -> Result<GpuSync, Error> {
        let Some(current) = &self.textures else {
            return Err(Error::invalid_state("the ring let go of its textures"));
        };
        if textures.textures.len() != current.textures.len() {
            return Err(Error::invalid_argument("a ring keeps its depth"));
        }
        if (textures.format(), textures.depth_format()) != (current.format(), current.depth_format()) {
            return Err(Error::unsupported("a ring keeps its formats, and its depth textures or their lack"));
        }
        textures.check()?;
        let sync = self.end_work(gpu)?;
        self.textures = Some(textures);
        Ok(sync)
    }

    /// Lets go of the textures, and returns the sync after which the host
    /// may use them, of the producer's kind. The target draws no frame
    /// after.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the
    /// target let go of its textures, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it can't submit its
    /// work, or wait for it for [`SyncKind::CpuComplete`]. It keeps its
    /// textures then.
    pub fn release_textures(&mut self, gpu: &mut WebGpu) -> Result<GpuSync, Error> {
        if self.textures.is_none() {
            return Err(Error::invalid_state("the ring let go of its textures"));
        }
        let sync = self.end_work(gpu)?;
        self.textures = None;
        self.depth_stencil = None;
        Ok(sync)
    }

    /// Forgets a frame begun on the textures, submits the work recorded so
    /// far, and returns the sync that ends it.
    fn end_work(&mut self, gpu: &mut WebGpu) -> Result<GpuSync, Error> {
        if let Some((_, image)) = self.imports.take() {
            destroy_imports(gpu, image);
        }
        let wait = self.producer == SyncKind::CpuComplete;
        gpu.flush(wait).map_err(|error| Error::native(format!("Submitting a ring's frames failed: {error}")))?;
        Ok(GpuSync::of(self.producer))
    }
}

/// A target's own depth and stencil texture of `size` and `format`, made
/// on `device` in `own` when it has none of that size.
fn own_depth_stencil(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    own: &mut Option<wgpu::Texture>,
    size: Size,
) -> wgpu::Texture {
    match own {
        Some(texture) if (texture.width(), texture.height()) == (size.width, size.height) => texture.clone(),
        _ => own
            .insert(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("borrowed ring depth and stencil"),
                size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            }))
            .clone(),
    }
}

fn destroy_imports(gpu: &mut WebGpu, image: FrameImage) {
    let imports = [Some(image.color), image.depth_stencil].into_iter().flatten();
    destroy_unused(gpu, imports.map(ResourceId::Texture));
}

impl FrameTarget for BorrowedTarget {
    type Gpu = WebGpu;

    fn begin_frame(&mut self, gpu: &mut WebGpu) -> Option<FrameImage> {
        let slot = self.selected;
        match self.imports {
            Some((imported, image)) if imported == slot => return Some(image),
            // A slot was selected since the frame began.
            Some((_, image)) => {
                self.imports = None;
                destroy_imports(gpu, image);
            }
            None => {}
        }
        let textures = self.textures.as_ref()?;
        let texture = textures.textures.get(slot)?.clone();
        // The host's depth and stencil texture of the slot, or the
        // target's own of the ring's size.
        let depth_stencil = match textures.depth_textures.get(slot) {
            Some(depth) => depth.clone(),
            None => own_depth_stencil(&self.device, self.depth_stencil_format, &mut self.depth_stencil, textures.size),
        };
        let image = FrameImage {
            size: textures.size,
            color: gpu.import_texture(&texture),
            depth_stencil: Some(gpu.import_texture(&depth_stencil)),
            targets: TargetFormats {
                color: from_wgpu_format(texture.format()),
                depth_stencil: from_wgpu_format(depth_stencil.format()),
            },
        };
        self.imports = Some((slot, image));
        Some(image)
    }

    fn end_frame(&mut self, gpu: &mut WebGpu) -> Handoff {
        let Some((_, image)) = self.imports.take() else {
            return Handoff::Dropped;
        };
        destroy_imports(gpu, image);
        let sync = if self.producer == SyncKind::QueueOrder {
            GpuSync::of(SyncKind::QueueOrder)
        } else {
            if let Err(error) = gpu.flush(true) {
                Log::error(Event::Render, format!("WebGPU: waiting for a borrowed texture's frame failed: {error}"));
                return Handoff::Dropped;
            }
            GpuSync::of(SyncKind::CpuComplete)
        };
        Handoff::GivenBack { slot: self.selected, sync }
    }

    /// Forgets the textures of a frame begun and not ended.
    fn tear_down(&mut self, gpu: &mut WebGpu) {
        if let Some((_, image)) = self.imports.take() {
            destroy_imports(gpu, image);
        }
    }
}
