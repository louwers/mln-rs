// A ring of the host's textures that frames are drawn into and given back to
// the host, and the ring target's Metal part (mln_gfx::ring): the shared
// event that the host waits on before it reads a texture, and the host's
// events that the target waits for before it draws into a texture the host
// read.

use std::convert::Infallible;
use std::ffi::c_void;
use std::ptr::NonNull;

use mln_gfx::gpu::{GpuError, TargetFormats, TextureFormat, TextureId};
use mln_gfx::ring::{self, RingBackend};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLEvent, MTLPixelFormat, MTLResource, MTLSharedEvent, MTLStorageMode, MTLTexture, MTLTextureType,
    MTLTextureUsage,
};

use crate::Id;
use crate::device::Device;
use crate::gpu::{MetalGpu, texture_format};
use crate::renderer_backend::RendererBackend;

/// A ring of the host's textures, all of `size` pixels, one pixel format and
/// one device, with `MTLTextureUsageRenderTarget`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedTextures {
    pub textures: Vec<Id<dyn MTLTexture>>,
    pub size: Size,
    /// The host's textures that frames draw their depth and stencil into
    /// and store, one for each of `textures`, of their size and device, one
    /// depth and stencil pixel format, and `MTLTextureUsageRenderTarget`, or
    /// none for depth of the target's own.
    pub depth_textures: Vec<Id<dyn MTLTexture>>,
}

// SAFETY: Metal textures may be used from any thread; the target that draws
// into them is on one.
unsafe impl Send for BorrowedTextures {}
// SAFETY: as above.
unsafe impl Sync for BorrowedTextures {}

impl BorrowedTextures {
    /// The host's textures `textures` and depth textures `depth_textures`,
    /// which the ring retains.
    ///
    /// # Safety
    ///
    /// Each pointer is a live `id<MTLTexture>`.
    #[must_use]
    pub unsafe fn from_raw(
        textures: &[NonNull<c_void>],
        depth_textures: &[NonNull<c_void>],
        size: Size,
    ) -> BorrowedTextures {
        let retain = |textures: &[NonNull<c_void>]| {
            textures
                .iter()
                // SAFETY: the caller's contract.
                .map(|texture| unsafe { texture.cast::<ProtocolObject<dyn MTLTexture>>().as_ref() }.retain())
                .collect()
        };
        BorrowedTextures { textures: retain(textures), size, depth_textures: retain(depth_textures) }
    }

    /// The formats of the depth textures that frames draw into: Metal's
    /// depth and stencil format of every GPU.
    #[must_use]
    pub fn depth_stencil_formats() -> DepthStencilFormats {
        DepthStencilFormats::of(&[DepthStencilFormat::Depth32FloatStencil8])
    }

    /// The kinds of sync with which the host can wait for a frame (the
    /// producer's) and the target for the host's reads (the consumer's), on
    /// `device`: queue order only on the host's queue.
    #[must_use]
    pub fn sync_kinds(device: &Device) -> (SyncKinds, SyncKinds) {
        let mut kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::MetalSharedEvent]);
        if device.has_host_queue() {
            kinds = kinds.with(SyncKind::QueueOrder);
        }
        (kinds, kinds)
    }

    /// Checks the textures against their ring and `device`.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// texture of another size than the ring's, of another device or pixel
    /// format than the first, or that isn't 2D with one sample and
    /// render-target usage.
    pub fn check(&self, device: &Device) -> Result<(), Error> {
        let Some(first) = self.textures.first() else {
            return Err(Error::invalid_argument("a ring needs at least one texture"));
        };
        let format = first.pixelFormat();
        for texture in &self.textures {
            if texture.width() != self.size.width as usize || texture.height() != self.size.height as usize {
                return Err(Error::invalid_argument("each texture's size must match the ring's physical size"));
            }
            if texture.pixelFormat() != format
                || Retained::as_ptr(&texture.device()) != Retained::as_ptr(device.device())
            {
                return Err(Error::invalid_argument(
                    "the ring's textures must share the device's MTLDevice and a pixel format",
                ));
            }
            if !texture.usage().contains(MTLTextureUsage::RenderTarget) {
                return Err(Error::invalid_argument("each texture needs MTLTextureUsageRenderTarget"));
            }
            if texture.textureType() != MTLTextureType::Type2D || texture.sampleCount() != 1 {
                return Err(Error::invalid_argument("each texture must be 2D with one sample"));
            }
        }
        let Some(depth) = self.depth_textures.first() else {
            return Ok(());
        };
        if self.depth_textures.len() != self.textures.len() {
            return Err(Error::invalid_argument("a ring's depth textures must be one for each of its textures"));
        }
        if depth.pixelFormat() != MTLPixelFormat::Depth32Float_Stencil8 {
            return Err(Error::unsupported("a ring's depth textures must be of MTLPixelFormatDepth32Float_Stencil8"));
        }
        for texture in &self.depth_textures {
            if texture.width() != self.size.width as usize
                || texture.height() != self.size.height as usize
                || texture.pixelFormat() != depth.pixelFormat()
                || Retained::as_ptr(&texture.device()) != Retained::as_ptr(device.device())
            {
                return Err(Error::invalid_argument(
                    "the ring's depth textures must have its size, a pixel format, and the device's MTLDevice",
                ));
            }
            if !texture.usage().contains(MTLTextureUsage::RenderTarget)
                || texture.storageMode() == MTLStorageMode::Memoryless
                || texture.textureType() != MTLTextureType::Type2D
                || texture.sampleCount() != 1
            {
                return Err(Error::invalid_argument(
                    "each depth texture must be 2D with one sample, render-target usage, and memory to store into",
                ));
            }
        }
        Ok(())
    }

    /// The textures' pixel format.
    #[must_use]
    pub fn pixel_format(&self) -> MTLPixelFormat {
        self.textures.first().map_or(MTLPixelFormat::Invalid, |texture| texture.pixelFormat())
    }

    /// The depth textures' pixel format, or `None` for a ring without them.
    #[must_use]
    pub fn depth_pixel_format(&self) -> Option<MTLPixelFormat> {
        self.depth_textures.first().map(|texture| texture.pixelFormat())
    }
}

type SharedEvent = Id<dyn MTLSharedEvent>;

/// Metal's one depth and stencil format of every GPU.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth32FloatStencil8;

/// Frames drawn into the host's textures, each given back to the host.
pub type BorrowedTarget = ring::BorrowedTarget<MetalRing>;

/// The host's reads of textures that a ring let go of: each shared event's
/// value, which the ring waits for before the host may use or destroy the
/// textures. Dropping it stops waiting.
pub type PendingReads = ring::PendingReads<MetalRing>;

/// A [`BorrowedTarget`]'s part on Metal: the shared event that its frames
/// signal, for a shared event producer.
pub struct MetalRing {
    device: Device,
    event: Option<SharedEvent>,
    event_value: u64,
}

impl RingBackend for MetalRing {
    type Gpu = MetalGpu;
    type Renderer = RendererBackend;
    type Textures = BorrowedTextures;
    type Image = Id<dyn MTLTexture>;
    /// The host's shared event, and the value its reads reach.
    type Wait = (SharedEvent, u64);
    type Handed = Infallible;
    type Reads = ();

    /// Fails as [`BorrowedTextures::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// that isn't one of [`BorrowedTextures::sync_kinds`]'s, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the device makes no
    /// shared event.
    fn new(backend: &mut RendererBackend, textures: &BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        let device = backend.device().clone();
        textures.check(&device)?;
        if !BorrowedTextures::sync_kinds(&device).0.contains(producer) {
            return Err(Error::unsupported(format!("the Metal device can't hand frames over with {producer:?}")));
        }
        let event = match producer {
            SyncKind::MetalSharedEvent => Some(
                device
                    .device()
                    .newSharedEvent()
                    .ok_or_else(|| Error::native("Failed to create a Metal shared event"))?,
            ),
            _ => None,
        };
        Ok(MetalRing { device, event, event_value: 0 })
    }

    fn images(textures: &BorrowedTextures) -> &[Self::Image] {
        &textures.textures
    }

    fn same_image(a: &Self::Image, b: &Self::Image) -> bool {
        std::ptr::eq(Retained::as_ptr(a), Retained::as_ptr(b))
    }

    fn size(textures: &BorrowedTextures) -> Size {
        textures.size
    }

    fn has_depth(textures: &BorrowedTextures) -> bool {
        !textures.depth_textures.is_empty()
    }

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another pixel format, or depth textures of another pixel format or
    /// where the ring has none, and as [`BorrowedTextures::check`] does.
    fn check_replacement(&self, current: &BorrowedTextures, next: &BorrowedTextures) -> Result<(), Error> {
        if next.pixel_format() != current.pixel_format() {
            return Err(Error::unsupported("a ring keeps its pixel format"));
        }
        if next.depth_pixel_format() != current.depth_pixel_format() {
            return Err(Error::unsupported("a ring keeps its depth textures' pixel format, or their lack"));
        }
        next.check(&self.device)
    }

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// pixel format that `MetalGpu` has no texture of.
    fn targets(_gpu: &MetalGpu, textures: &BorrowedTextures) -> Result<TargetFormats, Error> {
        let pixel_format = textures.pixel_format();
        let color = texture_format(pixel_format)
            .ok_or_else(|| Error::unsupported(format!("the Metal target draws into no {pixel_format:?}")))?;
        Ok(TargetFormats { color: Some(color), depth_stencil: Some(DEPTH_STENCIL) })
    }

    fn import(
        gpu: &mut MetalGpu,
        textures: &BorrowedTextures,
        slot: usize,
        depth: bool,
    ) -> Result<TextureId, GpuError> {
        let textures = if depth { &textures.depth_textures } else { &textures.textures };
        let texture = textures.get(slot).ok_or_else(|| GpuError::Invalid(format!("the ring has no slot {slot}")))?;
        let pixel_format = texture.pixelFormat();
        let format = if depth { Some(DEPTH_STENCIL) } else { texture_format(pixel_format) }
            .ok_or_else(|| GpuError::Unsupported(format!("a texture of {pixel_format:?}")))?;
        gpu.import_texture(texture, format)
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// shared event that names no object, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another kind than
    /// [`BorrowedTextures::sync_kinds`]'s consumer kinds.
    fn wait_for(&mut self, _gpu: &mut MetalGpu, consumer: ConsumerSync) -> Result<Option<Self::Wait>, Error> {
        let sync = consumer.sync;
        match sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => Ok(None),
            SyncKind::MetalSharedEvent => {
                // SAFETY: the host's `id<MTLSharedEvent>`, which the release
                // names and which stays valid for the session's life.
                let event =
                    unsafe { Retained::retain(sync.handle as usize as *mut ProtocolObject<dyn MTLSharedEvent>) }
                        .ok_or_else(|| Error::invalid_argument("a shared event release names no event"))?;
                Ok(Some((event, sync.value)))
            }
            kind => Err(Error::unsupported(format!("the Metal target waits for no {kind:?}"))),
        }
    }

    fn wait_before_using(gpu: &mut MetalGpu, texture: TextureId, (event, value): &Self::Wait) -> Result<(), GpuError> {
        gpu.wait_before_using(texture, ProtocolObject::<dyn MTLEvent>::from_ref(&**event), *value)
    }

    fn take_waits_into_next_submit(gpu: &mut MetalGpu, texture: TextureId) {
        gpu.take_waits_into_next_submit(texture);
    }

    fn forget_waits(gpu: &mut MetalGpu, texture: TextureId) {
        gpu.forget_waits(texture);
    }

    fn signal_after_next_submit(&mut self, gpu: &mut MetalGpu) {
        if let Some(event) = &self.event {
            self.event_value += 1;
            gpu.signal_after_next_submit(ProtocolObject::<dyn MTLEvent>::from_ref(&**event), self.event_value);
        }
    }

    fn producer_sync(&mut self, _gpu: &mut MetalGpu) -> Option<(GpuSync, Option<Infallible>)> {
        let event = self.event.as_ref()?;
        let handle = Retained::as_ptr(event) as usize as u64;
        Some((GpuSync { kind: SyncKind::MetalSharedEvent, handle, value: self.event_value }, None))
    }

    fn reads(&self) -> &() {
        &()
    }

    /// Whether the event reached the value, by its signaled value, which any
    /// macOS with shared events reports.
    fn ended((): &(), (event, value): &Self::Wait) -> bool {
        event.signaledValue() >= *value
    }
}
