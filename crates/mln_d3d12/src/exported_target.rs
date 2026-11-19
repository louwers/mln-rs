// A ring of shared textures that the target allocates and renders into, for
// the host to open through their NT handles on a D3D11 or D3D12 device of the
// same adapter. The ring target (mln_gfx::ring) draws into them as into a
// borrowed ring's.
//
// The textures are committed resources in a shared heap, which rest in the
// common state between frames, as a texture that another device opens must.

use std::os::windows::io::{AsRawHandle as _, OwnedHandle};

use mln_gfx::ring::{self, Exporter};
use mln_gfx::{
    ExportKind, ExportKinds, ExportedFrame, ExportedImage, ExportedRing, FrameDepth, NativeTexture, SyncKind, SyncKinds,
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

use crate::borrowed_target::{BorrowedTextures, D3d12Ring, share};
use crate::device::Device;
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

/// Frames drawn into shared textures of the target's own, and queued for the
/// host to acquire.
pub type ExportedTarget = ring::ExportedTarget<D3d12Exporter>;

/// An [`ExportedTarget`]'s part on D3D12: it allocates shared textures, and
/// with the ring's depth, a depth texture beside each.
pub struct D3d12Exporter {
    depth: usize,
    keep_depth: bool,
}

/// A generation's textures.
pub struct SharedTextures {
    textures: Vec<SharedTexture>,
    /// Each slot's depth texture, when the ring keeps depth.
    depth_textures: Vec<ID3D12Resource>,
}

impl Exporter for D3d12Exporter {
    type Ring = D3d12Ring;
    type Images = SharedTextures;

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind than DXGI shared handles, or another producer kind than
    /// [`sync_kinds`]'s.
    fn new(
        backend: &mut RendererBackend,
        ring: &ExportedRing,
        kind: ExportKind,
        producer: SyncKind,
    ) -> Result<D3d12Exporter, Error> {
        if !export_kinds(backend.device()).contains(kind) {
            return Err(Error::unsupported(format!("the D3D12 device exports no {kind:?}")));
        }
        if !sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("an exported D3D12 texture hands no frame over with {producer:?}")));
        }
        Ok(D3d12Exporter { depth: ring.depth, keep_depth: ring.keep_depth })
    }

    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a texture
    /// can't be made or shared.
    fn allocate(&self, backend: &mut RendererBackend, size: Size) -> Result<(SharedTextures, BorrowedTextures), Error> {
        let device = backend.device().device();
        let mut textures = Vec::with_capacity(self.depth);
        let mut depth_textures = Vec::new();
        for _ in 0..self.depth {
            textures.push(SharedTexture::new(device, size)?);
            if self.keep_depth {
                depth_textures.push(committed_texture(
                    device,
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
        Ok((SharedTextures { textures, depth_textures }, borrowed))
    }

    /// The shared texture of the frame at `slot`, its resource, and its
    /// depth texture when the ring keeps depth.
    fn frame(&self, images: &SharedTextures, textures: &BorrowedTextures, slot: usize) -> Option<ExportedFrame> {
        let texture = images.textures.get(slot)?;
        let native =
            |resource: &ID3D12Resource, format: DXGI_FORMAT, state: D3D12_RESOURCE_STATES| NativeTexture::D3d12 {
                resource: resource.as_raw() as usize as u64,
                format: format.0 as u32,
                state: state.0 as u32,
            };
        Some(ExportedFrame {
            size: textures.size,
            image: ExportedImage::DxgiSharedTexture {
                handle: texture.handle.as_raw_handle() as usize as u64,
                format: FORMAT.0 as u32,
            },
            texture: native(&texture.resource, FORMAT, STATE),
            depth: images
                .depth_textures
                .get(slot)
                .map(|depth| FrameDepth { texture: native(depth, DEPTH_FORMAT, DEPTH_STATE), export: None }),
        })
    }
}
