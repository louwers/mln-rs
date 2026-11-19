// A ring of shared textures that the target allocates and renders into, for
// the host to open through their NT handles on a D3D11 or D3D12 device of the
// same adapter. A resize makes a new generation of textures; the frames that
// the host holds keep the old generation's textures until they are given
// back.
//
// The textures are committed resources in a shared heap, which rest in the
// common state between frames, as a texture that another device opens must.

use mln_gfx::export::Generations;
use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::{
    ConsumerSync, ExportKind, ExportKinds, ExportedFrame, ExportedImage, ExportedRing, FrameDepth, NativeTexture,
    SyncKind, SyncKinds,
};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_FLAGS, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAGS, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATES, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12Device,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_SAMPLE_DESC,
};
use windows::core::Interface as _;

use crate::borrowed_target::{BorrowedTarget, BorrowedTextures, OwnedHandle, share};
use crate::device::Device;
use crate::gpu::D3d12Gpu;
use crate::renderer_backend::RendererBackend;

/// The format of the ring's textures, which D3D11 and Vulkan open as well.
const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
/// The state the ring's textures rest in, which another device takes them
/// in.
const STATE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
/// The format and state of the depth textures that a ring keeps on the
/// device.
const DEPTH_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D24_UNORM_S8_UINT;
const DEPTH_STATE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_DEPTH_WRITE;

/// The kinds of exported ring that sessions on `device` share their textures
/// in: DXGI shared handles, once the device has shared a texture.
#[must_use]
pub fn export_kinds(device: &Device) -> ExportKinds {
    device.export_kinds()
}

/// The kinds that `device` exports, found by sharing a texture.
pub(crate) fn probe(device: &ID3D12Device) -> ExportKinds {
    match SharedTexture::new(device, Size::new(1, 1)) {
        Ok(_) => ExportKinds::of(&[ExportKind::DxgiSharedHandle]),
        Err(error) => {
            Log::info(Event::Render, format!("The D3D12 device shares no textures: {}", error.message));
            ExportKinds::NONE
        }
    }
}

/// The kind of `ring` that a session on `device` shares its textures in.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the host
/// takes no DXGI shared handle.
pub fn plan(device: &Device, ring: &ExportedRing) -> Result<ExportKind, Error> {
    export_kinds(device)
        .intersection(ring.kinds)
        .iter()
        .next()
        .ok_or_else(|| Error::unsupported("the D3D12 device exports none of the ring's kinds of image"))
}

/// The kinds of sync with which the host waits for a frame (the
/// producer's), and the target for the host's reads (the consumer's): a
/// shared fence, which other devices and APIs open, or a finished frame.
#[must_use]
pub fn sync_kinds() -> (SyncKinds, SyncKinds) {
    let kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::D3d12Fence]);
    (kinds, kinds)
}

/// A committed 2D texture of `size`, `format` and `flags`, in a heap with
/// `heap_flags`, made in `state`.
fn committed_texture(
    device: &ID3D12Device,
    size: Size,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    heap_flags: D3D12_HEAP_FLAGS,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource, Error> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(size.width),
        Height: size.height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: flags,
    };
    let mut resource: Option<ID3D12Resource> = None;
    // SAFETY: the descriptions outlive the call, which fills `resource`.
    unsafe {
        device.CreateCommittedResource(&raw const heap, heap_flags, &raw const desc, state, None, &raw mut resource)
    }
    .map_err(|e| Error::native(format!("D3D12 can't make a texture: {e}")))?;
    resource.ok_or_else(|| Error::native("D3D12 made no texture"))
}

/// A texture of the ring, and the NT handle that shares it.
struct SharedTexture {
    resource: ID3D12Resource,
    handle: OwnedHandle,
}

impl SharedTexture {
    fn new(device: &ID3D12Device, size: Size) -> Result<SharedTexture, Error> {
        let resource = committed_texture(
            device,
            size,
            FORMAT,
            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            D3D12_HEAP_FLAG_SHARED,
            STATE,
        )?;
        let handle =
            share(device, &resource).map_err(|e| Error::native(format!("D3D12 can't share a texture: {e}")))?;
        Ok(SharedTexture { resource, handle })
    }
}

/// One generation of a ring's textures, of one size, and the borrowed ring
/// target that draws into them.
struct Generation {
    // Before the textures: dropping it waits for the frames that drew into
    // them.
    target: BorrowedTarget,
    textures: Vec<SharedTexture>,
    /// Each slot's depth texture, when the ring keeps depth.
    depth_textures: Vec<ID3D12Resource>,
    size: Size,
}

impl Generation {
    fn new(
        backend: &mut RendererBackend,
        depth: usize,
        size: Size,
        producer: SyncKind,
        keep_depth: bool,
    ) -> Result<Generation, Error> {
        let device = backend.device().device().clone();
        let mut textures = Vec::with_capacity(depth);
        let mut depth_textures = Vec::new();
        for _ in 0..depth {
            textures.push(SharedTexture::new(&device, size)?);
            if keep_depth {
                depth_textures.push(committed_texture(
                    &device,
                    size,
                    DEPTH_FORMAT,
                    D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
                    D3D12_HEAP_FLAG_NONE,
                    DEPTH_STATE,
                )?);
            }
        }
        let borrowed = BorrowedTextures {
            textures: textures.iter().map(|texture| texture.resource.clone()).collect(),
            size,
            state: STATE,
            depth_textures: depth_textures.clone(),
            depth_state: DEPTH_STATE,
        };
        let target = BorrowedTarget::new(backend, borrowed, producer)?;
        Ok(Generation { target, textures, depth_textures, size })
    }
}

/// Frames drawn into shared textures of the target's own, and queued for the
/// host to acquire.
pub struct ExportedTarget {
    depth: usize,
    producer: SyncKind,
    keep_depth: bool,
    generations: Generations<Generation>,
}

impl ExportedTarget {
    /// A target on `backend` that draws into `ring.depth` shared textures of
    /// `size` pixels, and hands each frame over with sync of the kind
    /// `producer`, one of [`sync_kinds`]'s.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind than DXGI shared handles or `producer`, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a texture can't be
    /// made.
    pub fn new(
        backend: &mut RendererBackend,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<ExportedTarget, Error> {
        if !export_kinds(backend.device()).contains(kind) {
            return Err(Error::unsupported(format!("the D3D12 device exports no {kind:?}")));
        }
        if !sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("an exported D3D12 texture hands no frame over with {producer:?}")));
        }
        let current = Generation::new(backend, ring.depth, size, producer, ring.keep_depth)?;
        Ok(ExportedTarget {
            depth: ring.depth,
            producer,
            keep_depth: ring.keep_depth,
            generations: Generations::new(current),
        })
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.generations.current_mut().target.select(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`.
    ///
    /// Fails as [`BorrowedTarget::release`] does.
    pub fn release(&mut self, gpu: &mut D3d12Gpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        if self.generations.give_back(slot, |mut generation| generation.target.tear_down(gpu)) {
            return Ok(());
        }
        self.generations.current_mut().target.release(gpu, slot, consumer)
    }

    /// The host gave the frame at `slot` up.
    pub fn discard(&mut self, gpu: &mut D3d12Gpu, slot: usize) {
        if !self.generations.give_back(slot, |mut generation| generation.target.tear_down(gpu)) {
            self.generations.current_mut().target.discard(gpu, slot);
        }
    }

    /// Draws into textures of `size` pixels from now on. The frames that the
    /// host holds, at `held` slots, keep their textures. Returns whether the
    /// ring has new textures, which it doesn't when they have `size` already.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a texture
    /// can't be made, and the target then keeps its own.
    pub fn resize(&mut self, backend: &mut RendererBackend, size: Size, held: &[usize]) -> Result<bool, Error> {
        use mln_gfx::RendererBackend as _;
        if self.size() == size {
            return Ok(false);
        }
        let next = Generation::new(backend, self.depth, size, self.producer, self.keep_depth)?;
        let gpu = backend.gpu();
        self.generations.replace(next, held, |mut generation| generation.target.tear_down(gpu));
        Ok(true)
    }

    /// The size of the textures that frames draw into.
    #[must_use]
    pub fn size(&self) -> Size {
        self.generations.current().size
    }

    /// The shared texture of the frame at `slot`, its resource, and its
    /// depth texture when the ring keeps depth.
    #[must_use]
    pub fn image(&self, slot: usize) -> Option<ExportedFrame> {
        let generations = &self.generations;
        let generation = generations.retired(slot).unwrap_or(generations.current());
        let texture = generation.textures.get(slot)?;
        let native =
            |resource: &ID3D12Resource, format: DXGI_FORMAT, state: D3D12_RESOURCE_STATES| NativeTexture::D3d12 {
                resource: resource.as_raw() as usize as u64,
                format: format.0 as u32,
                state: state.0 as u32,
            };
        Some(ExportedFrame {
            size: generation.size,
            image: ExportedImage::DxgiSharedTexture {
                handle: texture.handle.0.0 as usize as u64,
                format: FORMAT.0 as u32,
            },
            texture: native(&texture.resource, FORMAT, STATE),
            depth: generation
                .depth_textures
                .get(slot)
                .map(|depth| FrameDepth { texture: native(depth, DEPTH_FORMAT, DEPTH_STATE), export: None }),
        })
    }
}

impl FrameTarget for ExportedTarget {
    type Gpu = D3d12Gpu;

    fn begin_frame(&mut self, gpu: &mut D3d12Gpu) -> Option<FrameImage> {
        self.generations.current_mut().target.begin_frame(gpu)
    }

    fn end_frame(&mut self, gpu: &mut D3d12Gpu) -> Handoff {
        match self.generations.current_mut().target.end_frame(gpu) {
            Handoff::GivenBack { slot, sync } => Handoff::Queued { slot, sync },
            other => other,
        }
    }

    fn tear_down(&mut self, gpu: &mut D3d12Gpu) {
        self.generations.clear(|generation| generation.target.tear_down(gpu));
    }
}
