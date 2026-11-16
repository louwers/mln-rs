//! Video frames in GPU memory, imported through their DXGI shared handles.

use std::os::windows::io::AsRawHandle as _;
use std::rc::Rc;

use mln_gfx::gpu::validation::TextureInfo;
use mln_gfx::gpu::{Gpu as _, GpuError, ResourceId, TextureFormat, TextureId, TextureUsage};
use mln_gfx::video::{FrameKeepAlive, GpuFrame, ImportedFrame, PlaneLayout};
use mln_util::size::Size;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_STATE_COMMON,
    D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0, D3D12_SRV_DIMENSION_TEXTURE2D,
    D3D12_TEX2D_SRV, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12};

use super::{D3d12Gpu, Texture};
use crate::conv;
use crate::memory::TextureMemory;

/// One plane of a frame's resource: the format its view reads, its slice of
/// the resource and its size.
struct Plane {
    format: TextureFormat,
    slice: u32,
    size: Size,
}

/// The planes of a frame of `layout` in a resource of `format` and `size`,
/// `None` for a pair the backend doesn't import.
fn planes(layout: PlaneLayout, format: DXGI_FORMAT, size: Size) -> Option<Vec<Plane>> {
    match (layout, format) {
        (PlaneLayout::Rgba, DXGI_FORMAT_B8G8R8A8_UNORM) => {
            Some(vec![Plane { format: TextureFormat::Bgra8Unorm, slice: 0, size }])
        }
        (PlaneLayout::Nv12, DXGI_FORMAT_NV12) => {
            let chroma = Size::new(size.width.div_ceil(2), size.height.div_ceil(2));
            Some(vec![
                Plane { format: TextureFormat::R8Unorm, slice: 0, size },
                Plane { format: TextureFormat::Rg8Unorm, slice: 1, size: chroma },
            ])
        }
        _ => None,
    }
}

impl D3d12Gpu {
    /// Opens `frame`'s shared texture and makes a texture of each of its
    /// planes, which share the resource and `keep_alive`.
    ///
    /// The planes rest in the common state, as a texture that another device
    /// shares must between uses: draws that sample them promote them to a
    /// shader resource implicitly, and they decay back once the batch ends.
    /// The frame's producer finished writing it before handing it over
    /// ([`GpuFrame::DxgiSharedHandle`]), so the queue waits for nothing.
    pub(super) fn import_frame(
        &mut self,
        frame: &GpuFrame,
        keep_alive: FrameKeepAlive,
    ) -> Result<ImportedFrame, GpuError> {
        let GpuFrame::DxgiSharedHandle { handle, layout, color } = frame else {
            return Err(GpuError::Unsupported(format!("importing {} video frames", frame.kind())));
        };
        let device = self.device.device();
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: the frame owns the handle while the call reads it; the
        // resource the call opens holds its own reference.
        unsafe { device.OpenSharedHandle(HANDLE(handle.as_raw_handle()), &raw mut resource) }
            // A texture of another adapter, or one D3D12 can't open: the
            // player sends frames in memory instead.
            .map_err(|error| GpuError::Unsupported(format!("opening the video frame's shared texture: {error}")))?;
        let resource =
            resource.ok_or_else(|| GpuError::Invalid("the video frame's handle names no resource".to_owned()))?;
        // SAFETY: a plain query.
        let desc = unsafe { resource.GetDesc() };
        let size = Size::new(desc.Width as u32, desc.Height);
        let planes = (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && desc.SampleDesc.Count == 1)
            .then(|| planes(*layout, desc.Format, size))
            .flatten()
            .ok_or_else(|| {
                GpuError::Unsupported(format!("a {layout:?} video frame in a texture of format {}", desc.Format.0))
            })?;
        let memory = Rc::new(TextureMemory::of_frame(resource, self.memory.freed(), keep_alive));
        let mut ids: Vec<TextureId> = Vec::with_capacity(planes.len());
        for plane in planes {
            match self.insert_plane(&memory, &plane) {
                Ok(id) => ids.push(id),
                Err(error) => {
                    for id in ids {
                        self.destroy(ResourceId::Texture(id));
                    }
                    return Err(error);
                }
            }
        }
        Ok(ImportedFrame { layout: *layout, planes: ids, color: *color, external: false })
    }

    /// A sampled texture of `plane` of `memory`'s resource.
    fn insert_plane(&mut self, memory: &Rc<TextureMemory>, plane: &Plane) -> Result<TextureId, GpuError> {
        let device = self.device.device();
        let format = conv::texture_format(plane.format);
        let slot = self.srv_heap.allocate(device)?;
        let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: plane.slice,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        // SAFETY: the view is of the resource, into a descriptor no work
        // reads.
        unsafe { device.CreateShaderResourceView(&memory.resource, Some(&raw const view), slot.handle) };
        let usage = TextureUsage { sampled: true, render_target: false, copy_src: false };
        Ok(self.textures.insert(Texture {
            memory: Rc::clone(memory),
            info: TextureInfo { format: Some(plane.format), size: plane.size, usage },
            format,
            rest: D3D12_RESOURCE_STATE_COMMON,
            srv: Some(slot),
            rtv: None,
            dsv: None,
        }))
    }
}
