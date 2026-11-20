// A ring of the host's textures that frames are drawn into and given back to
// the host, and the ring target's D3D12 part (mln_gfx::ring): the fence that
// the host waits on before it reads a texture, and the host's fences that the
// target waits for before it draws into a texture the host read.

use std::convert::Infallible;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};

use mln_gfx::gpu::{GpuError, TargetFormats, TextureFormat, TextureId};
use mln_gfx::ring::{self, RingBackend, RingTextures};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::size::Size;
use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
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

impl RingTextures for BorrowedTextures {
    type Image = ID3D12Resource;

    fn images(&self) -> &[ID3D12Resource] {
        &self.textures
    }

    fn same_image(a: &ID3D12Resource, b: &ID3D12Resource) -> bool {
        a == b
    }

    fn size(&self) -> Size {
        self.size
    }

    fn has_depth(&self) -> bool {
        !self.depth_textures.is_empty()
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
            Ok(SharedFence { fence, handle: OwnedHandle::from_raw_handle(handle.0) })
        }
    }
}

/// The format of the depth and stencil texture of the target's own.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// Frames drawn into the host's textures, each given back to the host.
///
/// The waits and signals are fences on the device's queue: the submit of
/// the frame's first pass into a texture waits for the host's reads of it,
/// and the frame's last submit signals the ring's fence. A host that submits
/// its reads to the device's queue needs neither.
pub type BorrowedTarget = ring::BorrowedTarget<D3d12Ring>;

/// A [`BorrowedTarget`]'s part on D3D12: the shared fence that its frames
/// signal, for a fence producer, and the host's fences that it opened.
pub struct D3d12Ring {
    device: Device,
    /// The fence that frames signal, for a fence producer, and the value of
    /// the last frame's signal.
    fence: Option<SharedFence>,
    fence_value: u64,
    /// The host's fences, by their NT handles, opened once.
    host_fences: HashMap<u64, ID3D12Fence>,
}

impl D3d12Ring {
    /// The host's fence whose NT handle is `handle`, opened once.
    fn host_fence(&mut self, handle: u64) -> Result<ID3D12Fence, Error> {
        if let Some(fence) = self.host_fences.get(&handle) {
            return Ok(fence.clone());
        }
        let mut fence: Option<ID3D12Fence> = None;
        // SAFETY: the host's NT handle, which the release names and which
        // stays valid for the session's life; the call doesn't take it.
        unsafe { self.device.device().OpenSharedHandle(HANDLE(handle as usize as *mut _), &raw mut fence) }
            .map_err(|e| Error::invalid_argument(format!("the device can't open the host's fence: {e}")))?;
        let fence = fence.ok_or_else(|| Error::invalid_argument("the host's handle names no fence"))?;
        self.host_fences.insert(handle, fence.clone());
        Ok(fence)
    }
}

impl RingBackend for D3d12Ring {
    type Gpu = D3d12Gpu;
    type Renderer = RendererBackend;
    type Textures = BorrowedTextures;
    /// A fence, and the value that the work to wait for reaches.
    type Wait = (ID3D12Fence, u64);
    type Handed = Infallible;

    /// Fails as [`BorrowedTextures::check`] does, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another producer
    /// kind than [`BorrowedTextures::sync_kinds`]'s, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the device makes no
    /// shared fence.
    fn new(backend: &mut RendererBackend, textures: &BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        let device = backend.device().clone();
        textures.check(&device)?;
        if !BorrowedTextures::sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("the D3D12 device can't hand frames over with {producer:?}")));
        }
        let fence = match producer {
            SyncKind::D3d12Fence => Some(SharedFence::new(&device)?),
            _ => None,
        };
        Ok(D3d12Ring { device, fence, fence_value: 0, host_fences: HashMap::default() })
    }

    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another format, or depth textures of another format or where the
    /// ring has none, and as [`BorrowedTextures::check`] does.
    fn check_replacement(&self, current: &BorrowedTextures, next: &BorrowedTextures) -> Result<(), Error> {
        if next.format() != current.format() {
            return Err(Error::unsupported("a ring keeps its format"));
        }
        if next.depth_format() != current.depth_format() {
            return Err(Error::unsupported("a ring keeps its depth textures' format, or their lack"));
        }
        next.check(&self.device)
    }

    fn targets(_gpu: &D3d12Gpu, textures: &BorrowedTextures) -> Result<TargetFormats, Error> {
        Ok(TargetFormats {
            color: conv::from_texture_format(textures.format()),
            depth_stencil: textures.depth_format().and_then(conv::from_texture_format).or(Some(DEPTH_STENCIL)),
        })
    }

    fn import(
        gpu: &mut D3d12Gpu,
        textures: &BorrowedTextures,
        slot: usize,
        depth: bool,
    ) -> Result<TextureId, GpuError> {
        let (resources, state) =
            if depth { (&textures.depth_textures, textures.depth_state) } else { (&textures.textures, textures.state) };
        let resource = resources.get(slot).ok_or_else(|| GpuError::Invalid(format!("the ring has no slot {slot}")))?;
        gpu.import_texture(resource, state)
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// fence's handle that the device can't open, which stays the host's,
    /// and with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for another
    /// kind than [`BorrowedTextures::sync_kinds`]'s consumer kinds.
    fn wait_for(&mut self, _gpu: &mut D3d12Gpu, consumer: ConsumerSync) -> Result<Option<Self::Wait>, Error> {
        let sync = consumer.sync;
        match sync.kind {
            SyncKind::CpuComplete | SyncKind::QueueOrder => Ok(None),
            SyncKind::D3d12Fence => Ok(Some((self.host_fence(sync.handle)?, sync.value))),
            kind => Err(Error::unsupported(format!("the D3D12 target waits for no {kind:?}"))),
        }
    }

    fn wait_before_using(gpu: &mut D3d12Gpu, texture: TextureId, (fence, value): &Self::Wait) -> Result<(), GpuError> {
        gpu.wait_before_using(texture, fence.clone(), *value);
        Ok(())
    }

    fn take_waits_into_next_submit(gpu: &mut D3d12Gpu, texture: TextureId) {
        gpu.take_waits_into_next_submit(texture);
    }

    fn forget_waits(gpu: &mut D3d12Gpu, texture: TextureId) -> bool {
        gpu.forget_waits(texture)
    }

    fn signal_after_next_submit(&mut self, gpu: &mut D3d12Gpu) {
        if let Some(fence) = &self.fence {
            self.fence_value += 1;
            gpu.signal_after_next_submit(fence.fence.clone(), self.fence_value);
        }
    }

    fn producer_sync(&mut self, _gpu: &mut D3d12Gpu) -> Option<(GpuSync, Option<Infallible>)> {
        let fence = self.fence.as_ref()?;
        let handle = fence.handle.as_raw_handle() as usize as u64;
        Some((GpuSync { kind: SyncKind::D3d12Fence, handle, value: self.fence_value }, None))
    }
}

/// The NT handle of a shared texture, which the caller closes.
pub(crate) fn share(device: &ID3D12Device, resource: &ID3D12Resource) -> windows::core::Result<OwnedHandle> {
    // SAFETY: a plain call of the live device.
    let handle = unsafe { device.CreateSharedHandle(resource, None, GENERIC_ALL.0, PCWSTR::null()) }?;
    // SAFETY: the new handle is the caller's alone.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}
