// A ring of the host's textures that frames are drawn into and given back to
// the host, with what the host waits for before it reads them, and what the
// target waits for before it draws into a texture the host read.
//
// The waits and signals are fences on the device's queue: the submit of the
// frame's first pass into a texture waits for the host's reads of it, and the
// frame's last submit signals the ring's fence. A host that submits its reads
// to the device's queue needs neither.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, ResourceId, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATES, ID3D12Device, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, DXGI_FORMAT_UNKNOWN,
};
use windows::core::PCWSTR;

use crate::conv;
use crate::device::Device;
use crate::gpu::D3d12Gpu;
use crate::renderer_backend::RendererBackend;

/// A ring of the host's textures, all of `size` pixels, one format and the
/// device's, each in `state` when the host gives it and after each frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedTextures {
    /// 2D textures with one mip level, one layer and one sample, which allow
    /// render targets, of a typed color format that the backend draws.
    pub textures: Vec<ID3D12Resource>,
    pub size: Size,
    pub state: D3D12_RESOURCE_STATES,
    /// The host's textures that frames draw their depth and stencil into
    /// and store, one for each of `textures`, of their size and device, in
    /// `DXGI_FORMAT_D24_UNORM_S8_UINT` or `DXGI_FORMAT_D32_FLOAT_S8X24_UINT`,
    /// allowing depth-stencil views, or none for depth of the target's own.
    pub depth_textures: Vec<ID3D12Resource>,
    pub depth_state: D3D12_RESOURCE_STATES,
}

/// The depth and stencil format of a texture's `DXGI_FORMAT`.
#[must_use]
pub fn depth_stencil_format(format: DXGI_FORMAT) -> Option<DepthStencilFormat> {
    match format {
        DXGI_FORMAT_D24_UNORM_S8_UINT => Some(DepthStencilFormat::Depth24Stencil8),
        DXGI_FORMAT_D32_FLOAT_S8X24_UINT => Some(DepthStencilFormat::Depth32FloatStencil8),
        _ => None,
    }
}

/// The format of `resource`.
fn format_of(resource: &ID3D12Resource) -> DXGI_FORMAT {
    // SAFETY: a plain query.
    unsafe { resource.GetDesc() }.Format
}

impl BorrowedTextures {
    /// The formats of the depth textures that frames draw into: both, which
    /// every D3D12 device draws.
    #[must_use]
    pub fn depth_stencil_formats() -> DepthStencilFormats {
        DepthStencilFormats::of(&DepthStencilFormat::BY_PREFERENCE)
    }

    /// The kinds of sync with which the host can wait for a frame (the
    /// producer's) and the target for the host's reads (the consumer's): a
    /// finished frame, a shared fence, or the order of the device's queue,
    /// which the host submits to as well.
    #[must_use]
    pub fn sync_kinds() -> (SyncKinds, SyncKinds) {
        let kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::QueueOrder, SyncKind::D3d12Fence]);
        (kinds, kinds)
    }

    /// Checks the textures against their ring and `device`.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// texture of another size than the ring's, of another device or format
    /// than the first, or that isn't 2D with one mip level, layer and sample
    /// and allowing render targets, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a format that the
    /// backend doesn't draw.
    pub fn check(&self, device: &Device) -> Result<(), Error> {
        let Some(first) = self.textures.first() else {
            return Err(Error::invalid_argument("a ring needs at least one texture"));
        };
        let format = format_of(first);
        if conv::from_texture_format(format).is_none_or(conv::is_depth) {
            return Err(Error::unsupported(format!("the D3D12 target draws into no {format:?}")));
        }
        for texture in &self.textures {
            check_texture(texture, device, self.size, format, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0)?;
        }
        let Some(depth) = self.depth_textures.first() else {
            return Ok(());
        };
        if self.depth_textures.len() != self.textures.len() {
            return Err(Error::invalid_argument("a ring's depth textures must be one for each of its textures"));
        }
        let depth_format = format_of(depth);
        if depth_stencil_format(depth_format).is_none() {
            return Err(Error::unsupported(
                "a ring's depth textures must be of DXGI_FORMAT_D24_UNORM_S8_UINT or DXGI_FORMAT_D32_FLOAT_S8X24_UINT",
            ));
        }
        for texture in &self.depth_textures {
            check_texture(texture, device, self.size, depth_format, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0)?;
        }
        Ok(())
    }

    /// The textures' format.
    #[must_use]
    pub fn format(&self) -> DXGI_FORMAT {
        self.textures.first().map_or(DXGI_FORMAT_UNKNOWN, format_of)
    }

    /// The depth textures' format, or `None` for a ring without them.
    #[must_use]
    pub fn depth_format(&self) -> Option<DXGI_FORMAT> {
        self.depth_textures.first().map(format_of)
    }
}

/// Checks that `texture` is a 2D texture of `device`, `size` and `format`,
/// with one mip level, layer and sample, that allows `flag`.
fn check_texture(
    texture: &ID3D12Resource,
    device: &Device,
    size: Size,
    format: DXGI_FORMAT,
    flag: i32,
) -> Result<(), Error> {
    // SAFETY: plain queries.
    let (desc, owner) = unsafe {
        let mut owner: Option<ID3D12Device> = None;
        let _ = texture.GetDevice(&raw mut owner);
        (texture.GetDesc(), owner)
    };
    if owner.as_ref() != Some(device.device()) {
        return Err(Error::invalid_argument("the ring's textures must be of the session's D3D12 device"));
    }
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || desc.MipLevels != 1
        || desc.DepthOrArraySize != 1
        || desc.SampleDesc.Count != 1
    {
        return Err(Error::invalid_argument("each texture must be 2D with one mip level, one layer and one sample"));
    }
    if desc.Width != u64::from(size.width) || desc.Height != size.height {
        return Err(Error::invalid_argument("each texture's size must match the ring's physical size"));
    }
    if desc.Format != format {
        return Err(Error::invalid_argument("the ring's textures must share a format"));
    }
    if desc.Flags.0 & flag == 0 {
        return Err(Error::invalid_argument(
            "each texture must allow render targets, and each depth texture depth-stencil views",
        ));
    }
    Ok(())
}

/// An NT handle, which dropping closes.
pub(crate) struct OwnedHandle(pub(crate) HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is this one's to close.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A fence that the device's queue signals and other devices and APIs
/// open through its NT handle.
pub(crate) struct SharedFence {
    pub(crate) fence: ID3D12Fence,
    pub(crate) handle: OwnedHandle,
}

impl SharedFence {
    pub(crate) fn new(device: &Device) -> Result<SharedFence, Error> {
        let native = |what: &'static str| move |e: windows::core::Error| Error::native(format!("{what}: {e}"));
        // SAFETY: plain calls of the live device; the handle is then the
        // fence's to close.
        unsafe {
            let fence: ID3D12Fence =
                device.device().CreateFence(0, D3D12_FENCE_FLAG_SHARED).map_err(native("CreateFence"))?;
            let handle = device
                .device()
                .CreateSharedHandle(&fence, None, GENERIC_ALL.0, PCWSTR::null())
                .map_err(native("Sharing a fence"))?;
            Ok(SharedFence { fence, handle: OwnedHandle(handle) })
        }
    }
}

/// What the next frame drawn into a slot waits for: the host's fence and
/// the value its reads reach.
#[derive(Clone, Default)]
struct Slot {
    wait: Option<(ID3D12Fence, u64)>,
    /// The texture that has the next pass that draws into it wait for
    /// `wait`, from the frame begun on the slot until it ends.
    attached: Option<TextureId>,
}

/// The format of the depth and stencil texture of the target's own.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The ring's textures as textures of the backend's `Gpu`, with their depth
/// and stencil textures: the host's, or one of the target's own.
struct Imports {
    backend: Rc<RendererBackend>,
    size: Size,
    targets: TargetFormats,
    color: Vec<TextureId>,
    /// The host's depth and stencil textures, one for each slot.
    depth: Vec<TextureId>,
    /// The target's own depth and stencil, when the host gave none.
    own_depth: Option<TextureId>,
}

impl Imports {
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when the `Gpu`
    /// can't import a texture or make the depth and stencil texture.
    fn new(backend: &Rc<RendererBackend>, textures: &BorrowedTextures) -> Result<Imports, Error> {
        let mut imports = Imports {
            backend: Rc::clone(backend),
            size: textures.size,
            targets: TargetFormats {
                color: conv::from_texture_format(textures.format()),
                depth_stencil: textures.depth_format().and_then(conv::from_texture_format).or(Some(DEPTH_STENCIL)),
            },
            color: Vec::new(),
            depth: Vec::new(),
            own_depth: None,
        };
        let mut gpu = backend.d3d12_gpu();
        let import = |gpu: &mut D3d12Gpu, texture: &ID3D12Resource, state| {
            gpu.import_texture(texture, state)
                .map_err(|error| Error::native(format!("Importing a borrowed texture failed: {error}")))
        };
        for texture in &textures.textures {
            imports.color.push(import(&mut gpu, texture, textures.state)?);
        }
        for texture in &textures.depth_textures {
            imports.depth.push(import(&mut gpu, texture, textures.depth_state)?);
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
    /// Destroys the imports, which the `Gpu` keeps, and with them the host's
    /// textures, until the work that uses them finishes.
    fn drop(&mut self) {
        let mut gpu = self.backend.d3d12_gpu();
        for texture in self.color.drain(..).chain(self.depth.drain(..)).chain(self.own_depth.take()) {
            gpu.destroy(ResourceId::Texture(texture));
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
    /// The fence that frames signal, for a fence producer, and the value of
    /// the last frame's signal.
    fence: Option<SharedFence>,
    fence_value: Cell<u64>,
    /// The host's fences, by their NT handles, opened once.
    host_fences: RefCell<HashMap<u64, ID3D12Fence>>,
}

impl BorrowedTarget {
    /// A target on `backend` that draws into `textures` and hands each frame
    /// to the host with sync of the kind `producer`, which must be one of
    /// [`BorrowedTextures::sync_kinds`]'s producer kinds.
    ///
    /// Fails as [`BorrowedTextures::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another producer
    /// kind, and with [`ErrorKind::Native`](mln_util::ErrorKind) when the
    /// device makes no shared fence or the `Gpu` can't import a texture.
    pub fn new(backend: Rc<RendererBackend>, textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        let device = backend.d3d12_gpu().device().clone();
        textures.check(&device)?;
        if !BorrowedTextures::sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("the D3D12 device can't hand frames over with {producer:?}")));
        }
        let fence = match producer {
            SyncKind::D3d12Fence => Some(SharedFence::new(&device)?),
            _ => None,
        };
        let imports = Imports::new(&backend, &textures)?;
        let slots = textures.textures.len();
        Ok(BorrowedTarget {
            backend,
            textures: RefCell::new(textures),
            imports: RefCell::new(imports),
            selected: Cell::new(0),
            slots: RefCell::new(vec![Slot::default(); slots]),
            producer,
            fence,
            fence_value: Cell::new(0),
            host_fences: RefCell::new(HashMap::default()),
        })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&self, slot: usize) {
        self.selected.set(slot);
    }

    /// The host gave the texture at `slot` back: the next frame drawn into
    /// it waits for `consumer`, a sync of one of
    /// [`BorrowedTextures::sync_kinds`]'s consumer kinds. A fence's NT
    /// handle stays the host's.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// fence's handle that the device can't open.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let sync = consumer.sync;
        let wait = match sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => None,
            SyncKind::D3d12Fence => Some((self.host_fence(sync.handle)?, sync.value)),
            kind => return Err(Error::unsupported(format!("the D3D12 target waits for no {kind:?}"))),
        };
        let mut slots = self.slots.borrow_mut();
        let Some(state) = slots.get_mut(slot) else {
            return Err(Error::invalid_argument("the ring has no such slot"));
        };
        self.detach(state);
        state.wait = wait;
        Ok(())
    }

    /// The host's fence whose NT handle is `handle`, opened once.
    fn host_fence(&self, handle: u64) -> Result<ID3D12Fence, Error> {
        if let Some(fence) = self.host_fences.borrow().get(&handle) {
            return Ok(fence.clone());
        }
        let device = self.backend.d3d12_gpu().device().clone();
        let mut fence: Option<ID3D12Fence> = None;
        // SAFETY: the host's NT handle, which the release names and which
        // stays valid for the session's life; the call doesn't take it.
        unsafe { device.device().OpenSharedHandle(HANDLE(handle as usize as *mut _), &raw mut fence) }
            .map_err(|e| Error::invalid_argument(format!("the device can't open the host's fence: {e}")))?;
        let fence = fence.ok_or_else(|| Error::invalid_argument("the host's handle names no fence"))?;
        self.host_fences.borrow_mut().insert(handle, fence.clone());
        Ok(fence)
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again.
    pub fn discard(&self, slot: usize) {
        if let Some(state) = self.slots.borrow_mut().get_mut(slot) {
            self.detach(state);
            state.wait = None;
        }
    }

    /// Has no submit wait for what `slot` waits for.
    fn detach(&self, slot: &mut Slot) {
        if let Some(texture) = slot.attached.take() {
            self.backend.d3d12_gpu().forget_waits(texture);
        }
    }

    /// The host's reads that `waits` end with, and the frames submitted so
    /// far, which the host may use the textures after.
    fn pending(&self, waits: impl IntoIterator<Item = (ID3D12Fence, u64)>) -> PendingReads {
        let submitted = self.backend.d3d12_gpu().last_submit();
        PendingReads { waits: waits.into_iter().chain([submitted]).collect() }
    }

    /// Draws into `textures` from now on, which must be as many, of the
    /// ring's format, with depth textures of the ring's format or none as
    /// the ring has.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another format,
    /// and as [`BorrowedTextures::check`] does. The target then keeps its
    /// textures. Returns the host's reads of the textures that the ring let
    /// go of, which the ring waits for before the host may use them.
    pub fn replace(&self, textures: BorrowedTextures) -> Result<PendingReads, Error> {
        {
            let current = self.textures.borrow();
            if textures.textures.len() != current.textures.len() {
                return Err(Error::invalid_argument("a ring keeps its depth"));
            }
            if textures.format() != current.format() {
                return Err(Error::unsupported("a ring keeps its format"));
            }
            if textures.depth_format() != current.depth_format() {
                return Err(Error::unsupported("a ring keeps its depth textures' format, or their lack"));
            }
        }
        textures.check(self.backend.d3d12_gpu().device())?;
        // The old imports go, and what they wait for with them: the `Gpu`
        // keeps the textures until the work that uses them finishes, and the
        // slots keep the host's fences.
        *self.imports.borrow_mut() = Imports::new(&self.backend, &textures)?;

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
                    .position(|old| old == texture)
                    .and_then(|index| previous.get_mut(index))
                    .and_then(|slot| slot.wait.take())
            })
            .map(|wait| Slot { wait, attached: None })
            .collect();
        *slots = kept;
        Ok(self.pending(previous.into_iter().filter_map(|slot| slot.wait)))
    }

    /// Takes what the ring's textures still wait for, the host's reads of
    /// them, and the frames submitted so far, for the ring to wait for before
    /// it lets go of the textures.
    pub fn take_pending(&self) -> PendingReads {
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            self.detach(slot);
        }
        self.pending(slots.iter_mut().filter_map(|slot| slot.wait.take()).collect::<Vec<_>>())
    }

    /// The sync the host waits for, once the frame was submitted.
    fn producer_sync(&self) -> Option<GpuSync> {
        match (self.producer, &self.fence) {
            (SyncKind::D3d12Fence, Some(fence)) => Some(GpuSync {
                kind: SyncKind::D3d12Fence,
                handle: fence.handle.0.0 as usize as u64,
                value: self.fence_value.get(),
            }),
            (SyncKind::QueueOrder, _) => Some(GpuSync::of(SyncKind::QueueOrder)),
            _ => match self.backend.d3d12_gpu().flush(true) {
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
    /// The selected slot's texture, which the frame's first pass that draws
    /// into it waits for the host's reads before.
    fn begin_frame(&self) -> Option<FrameImage> {
        let slot = self.selected.get();
        let image = self.imports.borrow().frame_image(slot)?;
        let mut slots = self.slots.borrow_mut();
        // A frame begun again keeps the wait it attached.
        if let Some(state) = slots.get_mut(slot)
            && state.attached != Some(image.color)
            && let Some((fence, value)) = &state.wait
        {
            self.backend.d3d12_gpu().wait_before_using(image.color, fence.clone(), *value);
            state.attached = Some(image.color);
        }
        Some(image)
    }

    /// Submits what is left of the frame, signaling the producer's fence
    /// after it.
    fn end_frame(&self) -> Handoff {
        let slot = self.selected.get();
        let Some(color) = self.imports.borrow().color.get(slot).copied() else {
            return Handoff::Dropped;
        };
        let submitted = {
            let mut gpu = self.backend.d3d12_gpu();
            // What no pass took yet, such as where the frame drew nothing.
            gpu.take_waits_into_next_submit(color);
            if let Some(fence) = &self.fence {
                let value = self.fence_value.get() + 1;
                self.fence_value.set(value);
                gpu.signal_after_next_submit(fence.fence.clone(), value);
            }
            gpu.flush(false)
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

/// The host's reads of textures that a ring let go of, and the ring's frames
/// into them, which the ring waits for before the host may use or destroy the
/// textures. Dropping it stops waiting.
pub struct PendingReads {
    waits: Vec<(ID3D12Fence, u64)>,
}

impl PendingReads {
    /// Whether every read ended, which it checks without blocking: each
    /// fence's completed value.
    pub fn poll(&mut self) -> bool {
        // SAFETY: a plain query.
        self.waits.retain(|(fence, value)| unsafe { fence.GetCompletedValue() } < *value);
        self.waits.is_empty()
    }
}

/// The NT handle of a shared texture, which the caller closes.
pub(crate) fn share(device: &ID3D12Device, resource: &ID3D12Resource) -> windows::core::Result<OwnedHandle> {
    // SAFETY: a plain call of the live device, whose handle the caller owns.
    unsafe { device.CreateSharedHandle(resource, None, GENERIC_ALL.0, PCWSTR::null()) }.map(OwnedHandle)
}
