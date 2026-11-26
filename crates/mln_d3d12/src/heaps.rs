//! The descriptor heaps of a `D3d12Gpu`: CPU-only heaps that views are made
//! in, the shader-visible heap of texture tables, and the shader-visible
//! sampler heap.

use mln_gfx::gpu::GpuError;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::util_hash::HashMap;
use mln_glsl::layout::IMAGE_TEXTURE_COUNT;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMPARISON_FUNC_NEVER, D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
    D3D12_DESCRIPTOR_HEAP_TYPE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
    D3D12_FILTER_ANISOTROPIC, D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT, D3D12_FILTER_MIN_MAG_MIP_LINEAR,
    D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_FILTER_MIN_MAG_POINT_MIP_LINEAR, D3D12_GPU_DESCRIPTOR_HANDLE,
    D3D12_SAMPLER_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
    D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV, D3D12_TEXTURE_ADDRESS_MODE_CLAMP, D3D12_TEXTURE_ADDRESS_MODE_WRAP,
    ID3D12DescriptorHeap, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

/// The descriptors of each CPU-only heap.
const CPU_HEAP_SIZE: u32 = 256;
/// The texture tables of the shader-visible heap: as many bindings with
/// textures as a `D3d12Gpu` has at once.
const TABLES: u32 = 32 << 10;
/// The descriptors of a texture table: one for each texture a shader reads.
pub(crate) const TABLE_SIZE: u32 = IMAGE_TEXTURE_COUNT as u32;
/// The samplers of the shader-visible sampler heap: as many as naga's
/// sampler heap declares.
pub(crate) const SAMPLER_HEAP_SIZE: u32 = 2048;

fn create_heap(
    device: &ID3D12Device,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    count: u32,
    shader_visible: bool,
) -> Result<ID3D12DescriptorHeap, GpuError> {
    let desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: kind,
        NumDescriptors: count,
        Flags: if shader_visible { D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE } else { D3D12_DESCRIPTOR_HEAP_FLAG_NONE },
        NodeMask: 0,
    };
    // SAFETY: plain calls.
    unsafe { device.CreateDescriptorHeap(&raw const desc) }
        .map_err(|error| GpuError::Invalid(format!("CreateDescriptorHeap: {error}")))
}

fn offset(handle: D3D12_CPU_DESCRIPTOR_HANDLE, increment: u32, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    D3D12_CPU_DESCRIPTOR_HANDLE { ptr: handle.ptr + (increment * index) as usize }
}

/// A descriptor of a CPU-only heap.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuSlot {
    pub(crate) handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    heap: u32,
    index: u32,
}

/// Descriptors of one type in CPU-only heaps, which grow a heap at a time.
pub(crate) struct CpuDescriptors {
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    increment: u32,
    heaps: Vec<ID3D12DescriptorHeap>,
    /// The descriptors given back, then the next of the last heap.
    free: Vec<(u32, u32)>,
    next: u32,
}

impl CpuDescriptors {
    pub(crate) fn new(device: &ID3D12Device, kind: D3D12_DESCRIPTOR_HEAP_TYPE) -> CpuDescriptors {
        // SAFETY: a plain query.
        let increment = unsafe { device.GetDescriptorHandleIncrementSize(kind) };
        CpuDescriptors { kind, increment, heaps: Vec::new(), free: Vec::new(), next: CPU_HEAP_SIZE }
    }

    pub(crate) fn allocate(&mut self, device: &ID3D12Device) -> Result<CpuSlot, GpuError> {
        let (heap, index) = if let Some(slot) = self.free.pop() {
            slot
        } else {
            if self.next == CPU_HEAP_SIZE {
                self.heaps.push(create_heap(device, self.kind, CPU_HEAP_SIZE, false)?);
                self.next = 0;
            }
            self.next += 1;
            (self.heaps.len() as u32 - 1, self.next - 1)
        };
        // SAFETY: a plain query of a heap of this type.
        let start = unsafe { self.heaps[heap as usize].GetCPUDescriptorHandleForHeapStart() };
        Ok(CpuSlot { handle: offset(start, self.increment, index), heap, index })
    }

    pub(crate) fn free(&mut self, slot: CpuSlot) {
        self.free.push((slot.heap, slot.index));
    }
}

/// The shader-visible heap of texture tables, each [`TABLE_SIZE`]
/// descriptors, which a draw's bindings point at.
pub(crate) struct TableHeap {
    pub(crate) heap: ID3D12DescriptorHeap,
    increment: u32,
    cpu_start: D3D12_CPU_DESCRIPTOR_HANDLE,
    gpu_start: D3D12_GPU_DESCRIPTOR_HANDLE,
    free: Vec<u32>,
    next: u32,
    /// A view of no texture, which a table holds where its shader reads no
    /// texture.
    null: ID3D12DescriptorHeap,
}

impl TableHeap {
    pub(crate) fn new(device: &ID3D12Device) -> Result<TableHeap, GpuError> {
        let heap = create_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, TABLES * TABLE_SIZE, true)?;
        let null = create_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1, false)?;
        let null_view = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() },
            },
        };
        // SAFETY: plain calls on the heaps just made.
        unsafe {
            device.CreateShaderResourceView(
                None,
                Some(&raw const null_view),
                null.GetCPUDescriptorHandleForHeapStart(),
            );
            Ok(TableHeap {
                increment: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
                cpu_start: heap.GetCPUDescriptorHandleForHeapStart(),
                gpu_start: heap.GetGPUDescriptorHandleForHeapStart(),
                heap,
                free: Vec::new(),
                next: 0,
                null,
            })
        }
    }

    /// A table whose descriptors view `views`, by the texture index, and no
    /// texture elsewhere.
    pub(crate) fn allocate(
        &mut self,
        device: &ID3D12Device,
        views: &[Option<D3D12_CPU_DESCRIPTOR_HANDLE>; TABLE_SIZE as usize],
    ) -> Result<u32, GpuError> {
        let table = match self.free.pop() {
            Some(table) => table,
            None if self.next < TABLES => {
                self.next += 1;
                self.next - 1
            }
            None => return Err(GpuError::Unsupported(format!("more than {TABLES} bindings with textures"))),
        };
        // SAFETY: the table's descriptors aren't in use: it was never
        // handed out, or its bindings' work finished.
        unsafe {
            let null = self.null.GetCPUDescriptorHandleForHeapStart();
            for (index, view) in views.iter().enumerate() {
                let dest = offset(self.cpu_start, self.increment, table * TABLE_SIZE + index as u32);
                device.CopyDescriptorsSimple(1, dest, view.unwrap_or(null), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            }
        }
        Ok(table)
    }

    /// The start of `table` for `SetGraphicsRootDescriptorTable`.
    pub(crate) fn gpu(&self, table: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: self.gpu_start.ptr + u64::from(self.increment * table * TABLE_SIZE) }
    }

    /// Takes back `table`, whose bindings' work has finished.
    pub(crate) fn free(&mut self, table: u32) {
        self.free.push(table);
    }
}

/// The shader-visible sampler heap: a sampler for each state, kept as long
/// as the heap, as there are few states.
pub(crate) struct SamplerHeap {
    pub(crate) heap: ID3D12DescriptorHeap,
    increment: u32,
    slots: HashMap<SamplerState, u32>,
}

impl SamplerHeap {
    pub(crate) fn new(device: &ID3D12Device) -> Result<SamplerHeap, GpuError> {
        let heap = create_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, SAMPLER_HEAP_SIZE, true)?;
        // SAFETY: a plain query.
        let increment = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER) };
        Ok(SamplerHeap { heap, increment, slots: HashMap::default() })
    }

    /// The index in the heap of the sampler of `state`, made on first use.
    pub(crate) fn slot(&mut self, device: &ID3D12Device, state: &SamplerState) -> Result<u32, GpuError> {
        if let Some(&slot) = self.slots.get(state) {
            return Ok(slot);
        }
        let slot = self.slots.len() as u32;
        if slot == SAMPLER_HEAP_SIZE {
            return Err(GpuError::Unsupported(format!("more than {SAMPLER_HEAP_SIZE} sampler states")));
        }
        let linear = state.filter == TextureFilterType::Linear;
        let anisotropy = state.anisotropy();
        let filter = match (linear, state.mipmapped) {
            _ if anisotropy > 1 => D3D12_FILTER_ANISOTROPIC,
            (true, true) => D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            (true, false) => D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT,
            (false, true) => D3D12_FILTER_MIN_MAG_POINT_MIP_LINEAR,
            (false, false) => D3D12_FILTER_MIN_MAG_MIP_POINT,
        };
        let address = |wrap| match wrap {
            TextureWrapType::Repeat => D3D12_TEXTURE_ADDRESS_MODE_WRAP,
            TextureWrapType::Clamp => D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        };
        let desc = D3D12_SAMPLER_DESC {
            Filter: filter,
            AddressU: address(state.wrap_u),
            AddressV: address(state.wrap_v),
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MipLODBias: 0.0,
            // D3D12's most.
            MaxAnisotropy: u32::from(anisotropy.min(16)),
            ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            // Without mips, level 0 only.
            MaxLOD: if state.mipmapped { f32::MAX } else { 0.0 },
        };
        // SAFETY: the slot is past every sampler handed out, so no work
        // reads it.
        unsafe {
            let start = self.heap.GetCPUDescriptorHandleForHeapStart();
            device.CreateSampler(&raw const desc, offset(start, self.increment, slot));
        }
        self.slots.insert(*state, slot);
        Ok(slot)
    }
}
