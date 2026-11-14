// A ring of the host's textures that frames are drawn into and given back to
// the host, with what the host waits for before it reads them, and what the
// target waits for before it draws into a texture the host read.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLEvent, MTLPixelFormat, MTLResource, MTLSharedEvent, MTLStorageMode, MTLTexture, MTLTextureType,
    MTLTextureUsage,
};

use crate::device::Device;
use crate::gpu::texture_format;
use crate::mtl_fwd::MTLTexturePtr;
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

/// What the next frame drawn into a slot waits for.
#[derive(Clone, Default)]
struct Slot {
    wait: Option<(SharedEventPtr, u64)>,
    /// The texture that has the next submit that draws into it wait for
    /// `wait`, from the frame begun on the slot until it ends.
    attached: Option<TextureId>,
}

/// Metal's one depth and stencil format of every GPU.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth32FloatStencil8;

/// The ring's textures as textures of the backend's `Gpu`, with their depth
/// and stencil textures: the host's, or one of the target's own.
struct Imports {
    backend: Rc<BackendImpl>,
    size: Size,
    targets: TargetFormats,
    color: Vec<TextureId>,
    /// The host's depth and stencil textures, one for each slot.
    depth: Vec<TextureId>,
    /// The target's own depth and stencil, when the host gave none.
    own_depth: Option<TextureId>,
}

impl Imports {
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// pixel format the `Gpu` has no texture of, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it can't make the
    /// depth and stencil texture.
    fn new(backend: &Rc<BackendImpl>, textures: &BorrowedTextures) -> Result<Imports, Error> {
        let pixel_format = textures.pixel_format();
        let color_format = texture_format(pixel_format)
            .ok_or_else(|| Error::unsupported(format!("the Metal target draws into no {pixel_format:?}")))?;
        let mut imports = Imports {
            backend: Rc::clone(backend),
            size: textures.size,
            targets: TargetFormats { color: Some(color_format), depth_stencil: Some(DEPTH_STENCIL) },
            color: Vec::new(),
            depth: Vec::new(),
            own_depth: None,
        };
        let mut gpu = backend.gpu().borrow_mut();
        let import = |gpu: &mut crate::gpu::MetalGpu, texture: &ProtocolObject<dyn MTLTexture>, format| {
            gpu.import_texture(texture, format)
                .map_err(|error| Error::native(format!("Importing a borrowed texture failed: {error}")))
        };
        for texture in &textures.textures {
            imports.color.push(import(&mut gpu, texture, color_format)?);
        }
        for texture in &textures.depth_textures {
            imports.depth.push(import(&mut gpu, texture, DEPTH_STENCIL)?);
        }
        if textures.depth_textures.is_empty() {
            let desc = TextureDesc {
                label: "BorrowedDepthStencil",
                size: textures.size,
                format: DEPTH_STENCIL,
                usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
            };
            let texture = gpu
                .create_texture(&desc)
                .map_err(|error| Error::native(format!("Making the depth and stencil texture failed: {error}")))?;
            imports.own_depth = Some(texture);
        }
        Ok(imports)
    }

    /// The textures that a frame drawn into `slot` draws into.
    fn frame_image(&self, slot: usize) -> Option<FrameImage> {
        Some(FrameImage {
            size: self.size,
            color: *self.color.get(slot)?,
            depth_stencil: self.own_depth.or_else(|| self.depth.get(slot).copied()),
            targets: self.targets,
        })
    }
}

impl Drop for Imports {
    fn drop(&mut self) {
        let mut gpu = self.backend.gpu().borrow_mut();
        for texture in self.color.drain(..).chain(self.depth.drain(..)).chain(self.own_depth.take()) {
            gpu.destroy_texture(texture);
        }
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    textures: RefCell<BorrowedTextures>,
    imports: RefCell<Imports>,
    /// The slot the next frame draws into.
    selected: Cell<usize>,
    slots: RefCell<Vec<Slot>>,
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
        let imports = Imports::new(backend.backend_impl(), &textures)?;
        let slots = textures.textures.len();
        Ok(BorrowedTarget {
            backend,
            textures: RefCell::new(textures),
            imports: RefCell::new(imports),
            selected: Cell::new(0),
            slots: RefCell::new(vec![Slot::default(); slots]),
            producer,
            event,
            event_value: Cell::new(0),
        })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&self, slot: usize) {
        self.selected.set(slot);
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
        let mut slots = self.slots.borrow_mut();
        let Some(state) = slots.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        self.detach(state);
        let wait = &mut state.wait;
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
        if let Some(state) = self.slots.borrow_mut().get_mut(slot) {
            self.detach(state);
            state.wait = None;
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
            let current = self.textures.borrow();
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
        // The old imports go with what they wait for: the slots keep it.
        *self.imports.borrow_mut() = Imports::new(self.backend.backend_impl(), &textures)?;

        let old = std::mem::replace(&mut *self.textures.borrow_mut(), textures);
        // A texture that the new ring has again keeps what it waits for, at
        // its new slot.
        let mut slots = self.slots.borrow_mut();
        let mut previous = std::mem::take(&mut *slots);
        let kept = self
            .textures
            .borrow()
            .textures
            .iter()
            .map(|texture| {
                old.textures
                    .iter()
                    .position(|old| std::ptr::eq(Retained::as_ptr(old), Retained::as_ptr(texture)))
                    .and_then(|index| previous.get_mut(index))
                    .and_then(|slot| slot.wait.take())
            })
            .map(|wait| Slot { wait, attached: None })
            .collect();
        *slots = kept;
        Ok(PendingReads { waits: previous.into_iter().filter_map(|slot| slot.wait).collect() })
    }

    /// Takes what the ring's textures still wait for, the host's reads of
    /// them, for the ring to wait for before it lets go of the textures.
    pub fn take_pending(&self) -> PendingReads {
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            self.detach(slot);
        }
        PendingReads { waits: slots.iter_mut().filter_map(|slot| slot.wait.take()).collect() }
    }

    /// Has no submit wait for what `slot` waits for.
    fn detach(&self, slot: &mut Slot) {
        if let Some(texture) = slot.attached.take() {
            self.backend.backend_impl().gpu().borrow_mut().forget_waits(texture);
        }
    }

    /// The sync the host waits for, once the frame was submitted.
    fn producer_sync(&self) -> Option<GpuSync> {
        match (self.producer, &self.event) {
            (SyncKind::MetalSharedEvent, Some(event)) => Some(GpuSync {
                kind: SyncKind::MetalSharedEvent,
                handle: Retained::as_ptr(event) as usize as u64,
                value: self.event_value.get(),
            }),
            (SyncKind::QueueOrder, _) => Some(GpuSync::of(SyncKind::QueueOrder)),
            _ => match self.backend.backend_impl().gpu().borrow_mut().finish() {
                Ok(()) => Some(GpuSync::of(SyncKind::CpuComplete)),
                Err(error) => {
                    Log::error(Event::Render, format!("Waiting for a borrowed texture's frame failed: {error}"));
                    None
                }
            },
        }
    }
}

impl FrameTarget for BorrowedTarget {
    /// The selected slot's texture, which the frame's first submit that
    /// draws into it waits for the host's reads before.
    fn begin_frame(&self) -> Option<FrameImage> {
        let slot = self.selected.get();
        let image = self.imports.borrow().frame_image(slot)?;
        let mut slots = self.slots.borrow_mut();
        let Some(state) = slots.get_mut(slot) else {
            return Some(image);
        };
        // A frame begun again keeps the wait it attached.
        if let (Some((event, value)), None) = (&state.wait, state.attached.filter(|&t| t == image.color)) {
            let mut gpu = self.backend.backend_impl().gpu().borrow_mut();
            let event = ProtocolObject::<dyn MTLEvent>::from_ref(&**event);
            if let Err(error) = gpu.wait_before_using(image.color, event, *value) {
                Log::error(Event::Render, format!("Waiting for the host's reads failed: {error}"));
            }
            state.attached = Some(image.color);
        }
        Some(image)
    }

    /// Submits what is left of the frame, signaling the producer's event
    /// after it.
    fn end_frame(&self) -> Handoff {
        let slot = self.selected.get();
        let Some(color) = self.imports.borrow().color.get(slot).copied() else {
            return Handoff::Dropped;
        };
        let submitted = {
            let mut gpu = self.backend.backend_impl().gpu().borrow_mut();
            // What no pass took yet, such as where the frame drew nothing.
            gpu.take_waits_into_next_submit(color);
            if let Some(event) = &self.event {
                let value = self.event_value.get() + 1;
                self.event_value.set(value);
                gpu.signal_after_next_submit(ProtocolObject::<dyn MTLEvent>::from_ref(&**event), value);
            }
            gpu.submit(&[])
        };
        // The frame's submits took what the slot waited for.
        if let Some(state) = self.slots.borrow_mut().get_mut(slot)
            && state.attached.take() == Some(color)
        {
            state.wait = None;
        }
        if let Err(error) = submitted {
            Log::error(Event::Render, format!("Submitting a borrowed texture's frame failed: {error}"));
            return Handoff::Dropped;
        }
        match self.producer_sync() {
            Some(sync) => Handoff::GivenBack { slot, sync },
            None => Handoff::Dropped,
        }
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
