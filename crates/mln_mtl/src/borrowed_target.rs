// A ring of the host's textures that frames are drawn into and given back to
// the host, with what the host waits for before it reads them, and what the
// target waits for before it draws into a texture the host read.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLEvent, MTLPixelFormat, MTLRenderPassDescriptor, MTLResource,
    MTLSharedEvent, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};

use crate::device::Device;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr, MTLTexturePtr};
use crate::renderer_backend::{BackendImpl, RendererBackend};

/// A ring of the host's textures, all of `size` pixels, one pixel format and
/// one device, with `MTLTextureUsageRenderTarget`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedTextures {
    pub textures: Vec<MTLTexturePtr>,
    pub size: Size,
    /// The host's textures that frames draw their depth and stencil into
    /// and store, one for each of `textures`, of their size and device, one
    /// depth and stencil pixel format, and `MTLTextureUsageRenderTarget`, or
    /// none for depth of the target's own.
    pub depth_textures: Vec<MTLTexturePtr>,
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

type SharedEventPtr = Retained<ProtocolObject<dyn MTLSharedEvent>>;

/// The frame's command buffer and render pass over the selected texture, and
/// their depth and stencil textures.
struct Resource {
    backend: Rc<BackendImpl>,
    textures: RefCell<BorrowedTextures>,
    selected: Cell<usize>,
    depth: RefCell<Option<(Size, MTLTexturePtr, MTLTexturePtr)>>,
    command_buffer: RefCell<Option<MTLCommandBufferPtr>>,
    render_pass_descriptor: RefCell<Option<MTLRenderPassDescriptorPtr>>,
    /// What the next frame drawn into each slot waits for, which the
    /// frame's command buffer waits for before it draws.
    waits: RefCell<Vec<Option<(SharedEventPtr, u64)>>>,
}

impl Resource {
    fn texture_2d(&self, format: MTLPixelFormat, size: Size) -> Option<MTLTexturePtr> {
        // SAFETY: the size is non-zero and the format a valid 2D one.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format,
                size.width as usize,
                size.height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        descriptor.setStorageMode(MTLStorageMode::Private);
        self.backend.get_device().newTextureWithDescriptor(&descriptor)
    }

    /// The depth and stencil textures of the ring's size: the host's of the
    /// selected slot, which holds both, or the target's own. Intel and AMD
    /// GPUs and the simulator take one texture for both.
    fn depth_stencil(&self) -> Option<(MTLTexturePtr, MTLTexturePtr)> {
        if let Some(depth) = self.textures.borrow().depth_textures.get(self.selected.get()) {
            return Some((depth.clone(), depth.clone()));
        }
        let size = self.textures.borrow().size;
        let mut depth = self.depth.borrow_mut();
        if depth.as_ref().is_none_or(|(made, _, _)| *made != size) {
            *depth = if cfg!(target_arch = "x86_64") || cfg!(target_abi = "sim") {
                self.texture_2d(MTLPixelFormat::Depth32Float_Stencil8, size)
                    .map(|texture| (size, texture.clone(), texture))
            } else {
                match (
                    self.texture_2d(MTLPixelFormat::Depth32Float, size),
                    self.texture_2d(MTLPixelFormat::Stencil8, size),
                ) {
                    (Some(depth), Some(stencil)) => Some((size, depth, stencil)),
                    _ => None,
                }
            };
        }
        depth.as_ref().map(|(_, depth, stencil)| (depth.clone(), stencil.clone()))
    }
}

impl TargetResource for Resource {
    fn bind(&self) {
        // An upload pass and a render pass both bind the resource in one
        // frame, and the frame's end commits the one command buffer.
        let mut command_buffer = self.command_buffer.borrow_mut();
        if command_buffer.is_none() {
            let buffer = self.backend.get_command_queue().commandBuffer();
            let wait = self.waits.borrow_mut().get_mut(self.selected.get()).and_then(Option::take);
            if let (Some(buffer), Some((event, value))) = (&buffer, wait) {
                buffer.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*event), value);
            }
            *command_buffer = buffer;
        }
        let descriptor = MTLRenderPassDescriptor::new();
        let texture = self.textures.borrow().textures.get(self.selected.get()).cloned();
        // SAFETY: index 0 is in the attachment array's bounds.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }.setTexture(texture.as_deref());
        if let Some((depth, stencil)) = self.depth_stencil() {
            descriptor.depthAttachment().setTexture(Some(&depth));
            descriptor.stencilAttachment().setTexture(Some(&stencil));
        }
        *self.render_pass_descriptor.borrow_mut() = Some(descriptor);
    }

    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        self.command_buffer.borrow().clone()
    }

    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        self.render_pass_descriptor.borrow().clone()
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    resource: Rc<Resource>,
    target: Rc<DrawTarget>,
    producer: SyncKind,
    /// The event that frames signal, for a shared event producer.
    event: Option<SharedEventPtr>,
    event_value: Cell<u64>,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `textures` and hands each frame
    /// to the host with sync of the kind `producer`, which must be one of
    /// [`BorrowedTextures::sync_kinds`]'s producer kinds.
    ///
    /// Fails as [`BorrowedTextures::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a producer kind
    /// the device lacks, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when the device makes no shared event.
    pub fn new(backend: Rc<RendererBackend>, textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
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
        let depth = textures.textures.len();
        let size = textures.size;
        let resource = Rc::new(Resource {
            backend: Rc::clone(backend.backend_impl()),
            textures: RefCell::new(textures),
            selected: Cell::new(0),
            depth: RefCell::new(None),
            command_buffer: RefCell::new(None),
            render_pass_descriptor: RefCell::new(None),
            waits: RefCell::new(vec![None; depth]),
        });
        let target = Rc::new(DrawTarget::new(size, Rc::<Resource>::clone(&resource)));
        Ok(BorrowedTarget { backend, resource, target, producer, event, event_value: Cell::new(0) })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&self, slot: usize) {
        self.resource.selected.set(slot);
    }

    /// The host gave the texture at `slot` back: the next frame drawn into
    /// it waits for `consumer`, a sync of one of
    /// [`BorrowedTextures::sync_kinds`]'s consumer kinds.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// shared event that names no object.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let sync = consumer.sync;
        let mut waits = self.resource.waits.borrow_mut();
        let Some(wait) = waits.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        *wait = None;
        match sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => Ok(()),
            SyncKind::MetalSharedEvent => {
                // SAFETY: the host's `id<MTLSharedEvent>`, which the release
                // names and which stays valid for the session's life.
                let event =
                    unsafe { Retained::retain(sync.handle as usize as *mut ProtocolObject<dyn MTLSharedEvent>) }
                        .ok_or_else(|| Error::invalid_argument("a shared event release names no event"))?;
                *wait = Some((event, sync.value));
                Ok(())
            }
            kind => Err(Error::unsupported(format!("the Metal target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again.
    pub fn discard(&self, slot: usize) {
        if let Some(wait) = self.resource.waits.borrow_mut().get_mut(slot) {
            *wait = None;
        }
    }

    /// Draws into `textures` from now on, which must be as many, of the
    /// ring's pixel format.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another pixel
    /// format, and as [`BorrowedTextures::check`] does. The target then keeps
    /// its textures. Returns the host's reads of the textures that the ring
    /// let go of, which the ring waits for before the host may use them.
    pub fn replace(&self, textures: BorrowedTextures) -> Result<PendingReads, Error> {
        {
            let current = self.resource.textures.borrow();
            if textures.textures.len() != current.textures.len() {
                return Err(Error::invalid_argument("a ring keeps its depth"));
            }
            if textures.pixel_format() != current.pixel_format() {
                return Err(Error::unsupported("a ring keeps its pixel format"));
            }
            let depth_format = |textures: &BorrowedTextures| textures.depth_textures.first().map(|t| t.pixelFormat());
            if depth_format(&textures) != depth_format(&current) {
                return Err(Error::unsupported("a ring keeps its depth textures' pixel format, or their lack"));
            }
        }
        textures.check(self.backend.device())?;
        self.target.set_size(textures.size);
        let old = std::mem::replace(&mut *self.resource.textures.borrow_mut(), textures);
        // A texture that the new ring has again keeps what it waits for, at
        // its new slot.
        let mut waits = self.resource.waits.borrow_mut();
        let mut previous = std::mem::take(&mut *waits);
        let kept = self
            .resource
            .textures
            .borrow()
            .textures
            .iter()
            .map(|texture| {
                old.textures
                    .iter()
                    .position(|old| std::ptr::eq(Retained::as_ptr(old), Retained::as_ptr(texture)))
                    .and_then(|index| previous.get_mut(index))
                    .and_then(Option::take)
            })
            .collect();
        *waits = kept;
        Ok(PendingReads { waits: previous.into_iter().flatten().collect() })
    }

    /// Takes what the ring's textures still wait for, the host's reads of
    /// them, for the ring to wait for before it lets go of the textures.
    pub fn take_pending(&self) -> PendingReads {
        PendingReads { waits: self.resource.waits.borrow_mut().iter_mut().filter_map(Option::take).collect() }
    }

    /// The sync the host waits for, once `command_buffer` was committed.
    fn producer_sync(&self, command_buffer: &ProtocolObject<dyn MTLCommandBuffer>) -> GpuSync {
        match (self.producer, &self.event) {
            (SyncKind::MetalSharedEvent, Some(event)) => GpuSync {
                kind: SyncKind::MetalSharedEvent,
                handle: Retained::as_ptr(event) as usize as u64,
                value: self.event_value.get(),
            },
            (SyncKind::QueueOrder, _) => GpuSync::of(SyncKind::QueueOrder),
            _ => {
                command_buffer.waitUntilCompleted();
                GpuSync::of(SyncKind::CpuComplete)
            }
        }
    }
}

impl FrameTarget for BorrowedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        Some(Rc::<DrawTarget>::clone(&self.target))
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        self.resource.render_pass_descriptor.borrow_mut().take();
        let Some(command_buffer) = self.resource.command_buffer.borrow_mut().take() else {
            return Handoff::Dropped;
        };
        if let Some(event) = &self.event {
            let value = self.event_value.get() + 1;
            self.event_value.set(value);
            command_buffer.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&**event), value);
        }
        command_buffer.commit();
        let sync = self.producer_sync(&command_buffer);
        Handoff::GivenBack { slot: self.resource.selected.get(), sync }
    }
}

/// The host's reads of textures that a ring let go of, which the ring
/// waits for before the host may use or destroy the textures. Dropping it
/// stops waiting.
pub struct PendingReads {
    waits: Vec<(SharedEventPtr, u64)>,
}

impl PendingReads {
    /// Whether every read ended, which it checks without blocking: each
    /// shared event's signaled value, which any macOS with shared events
    /// reports.
    pub fn poll(&mut self) -> bool {
        self.waits.retain(|(event, value)| event.signaledValue() < *value);
        self.waits.is_empty()
    }
}
