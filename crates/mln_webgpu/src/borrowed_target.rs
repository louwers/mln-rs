// A ring of the host's textures, on the device that renders into them, that
// frames are drawn into and given back to the host. Both sides submit to the
// device's one queue, which orders the host's reads after the frame and the
// next frame after the host's reads.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::draw_target::{DrawTarget, TargetResource};
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

/// The selected texture's view, and the ring's depth and stencil texture.
struct Resource {
    device: wgpu::Device,
    textures: RefCell<BorrowedTextures>,
    selected: Cell<usize>,
    depth_stencil_format: wgpu::TextureFormat,
    depth_stencil: RefCell<Option<(Size, wgpu::TextureView)>>,
}

impl TargetResource for Resource {
    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        let textures = self.textures.borrow();
        let texture = textures.textures.get(self.selected.get())?;
        Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }

    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.textures.borrow().format()
    }

    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        if let Some(depth) = self.textures.borrow().depth_textures.get(self.selected.get()) {
            return Some(depth.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let size = self.textures.borrow().size;
        let mut depth_stencil = self.depth_stencil.borrow_mut();
        if depth_stencil.as_ref().is_none_or(|(made, _)| *made != size) {
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
            *depth_stencil = Some((size, texture.create_view(&wgpu::TextureViewDescriptor::default())));
        }
        depth_stencil.as_ref().map(|(_, view)| view.clone())
    }

    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        Some(self.textures.borrow().depth_format().unwrap_or(self.depth_stencil_format))
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    resource: Rc<Resource>,
    target: Rc<DrawTarget>,
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
        let size = textures.size;
        let resource = Rc::new(Resource {
            device,
            textures: RefCell::new(textures),
            selected: Cell::new(0),
            depth_stencil_format,
            depth_stencil: RefCell::new(None),
        });
        let target = Rc::new(DrawTarget::new(size, resource.clone()));
        Ok(BorrowedTarget { backend, resource, target, producer })
    }

    /// How many textures the ring has.
    #[must_use]
    pub fn depth(&self) -> usize {
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
        self.target.set_size(textures.size);
        *self.resource.textures.borrow_mut() = textures;
        Ok(())
    }
}

impl FrameTarget for BorrowedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        Some(self.target.clone())
    }

    fn end_frame(&self, encoder: &dyn GfxCommandEncoder) -> Handoff {
        let Some(encoder) = encoder.downcast_ref::<CommandEncoder>() else {
            return Handoff::Dropped;
        };
        encoder.submit_frame(Some(self.target.resource()));
        let sync = if self.producer == SyncKind::QueueOrder {
            GpuSync::of(SyncKind::QueueOrder)
        } else {
            let wait = wgpu::PollType::Wait { submission_index: None, timeout: None };
            let _ = self.backend.device().device().poll(wait);
            GpuSync::of(SyncKind::CpuComplete)
        };
        Handoff::GivenBack { slot: self.resource.selected.get(), sync }
    }
}
