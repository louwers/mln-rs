//! The GPU memory of a `D3d12Gpu`: gpu-allocator's heaps, which textures and
//! large buffers are placed in, and slabs of small buffers in placed buffers
//! of their own, as a placed buffer takes 64 KiB at least.
//!
//! The `Gpu` gives memory back through [`Memory::free_buffer`] and
//! [`Memory::free_texture`] once the GPU's work with it has finished.

use gpu_allocator::d3d12::{
    Allocator, AllocatorCreateDesc, ID3D12DeviceVersion, Resource, ResourceCategory, ResourceCreateDesc,
    ResourceStateOrBarrierLayout, ResourceType,
};
use gpu_allocator::{AllocationSizes, AllocatorDebugSettings, MemoryLocation};
use mln_gfx::gpu::GpuError;
use mln_util::logging::{Event, Log};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT, D3D12_RANGE, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_RESOURCE_STATES, D3D12_SMALL_RESOURCE_PLACEMENT_ALIGNMENT, D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12Device,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};

/// The range of a mapping that the CPU reads none of.
pub(crate) static NO_RANGE: D3D12_RANGE = D3D12_RANGE { Begin: 0, End: 0 };

/// The slab sizes: 256 bytes, the alignment of a constant buffer, doubling
/// up to 32 KiB. A larger buffer is placed on its own.
const SLAB_SIZES: usize = 8;
const MIN_SLAB: u64 = 256;
/// The size of the placed buffer that a size's slabs share.
const SLAB_PAGE: u64 = 64 << 10;

/// The memory locations of buffers, by index.
const LOCATIONS: [MemoryLocation; 3] = [MemoryLocation::GpuOnly, MemoryLocation::CpuToGpu, MemoryLocation::GpuToCpu];

fn location_index(location: MemoryLocation) -> usize {
    LOCATIONS.iter().position(|&known| known == location).unwrap_or(0)
}

fn allocation_error(what: &str) -> impl Fn(gpu_allocator::AllocationError) -> GpuError + '_ {
    move |error| GpuError::Invalid(format!("allocating {what}: {error}"))
}

/// The description of a buffer of `size` bytes.
#[must_use]
pub(crate) fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

/// What memory came from.
enum Owner {
    /// A slab of the slabs of a location and size, at an offset of a page.
    Slab { location: usize, size: usize, page: usize, offset: u64 },
    /// A resource of gpu-allocator's.
    Resource(Resource),
}

/// A buffer's memory: a range of a resource, mapped where the CPU writes or
/// reads it, which [`Memory::free_buffer`] gives back.
pub(crate) struct BufferMemory {
    pub(crate) resource: ID3D12Resource,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    /// The CPU's address of `offset` in upload memory, null elsewhere.
    mapped: *mut u8,
    owner: Owner,
}

impl BufferMemory {
    /// The GPU address of the start of the range.
    pub(crate) fn address(&self) -> u64 {
        // SAFETY: a plain query.
        unsafe { self.resource.GetGPUVirtualAddress() + self.offset }
    }

    /// The bytes of the range, in upload memory.
    ///
    /// # Safety
    ///
    /// No GPU work that runs while the slice lives reads them.
    pub(crate) unsafe fn bytes_mut(&mut self) -> &mut [u8] {
        debug_assert!(!self.mapped.is_null());
        // SAFETY: the mapping covers the range, as long as the memory lives.
        unsafe { std::slice::from_raw_parts_mut(self.mapped, self.size as usize) }
    }

    /// The bytes of the range, in upload memory.
    #[cfg(test)]
    pub(crate) fn read(&self) -> Vec<u8> {
        assert!(!self.mapped.is_null(), "a read of memory the CPU doesn't reach");
        // SAFETY: the mapping covers the range; the tests read it with no
        // work in flight.
        unsafe { std::slice::from_raw_parts(self.mapped, self.size as usize).to_vec() }
    }

    /// Copies `data` to `at` bytes into the range, in upload memory.
    ///
    /// # Safety
    ///
    /// No GPU work that runs during the call reads those bytes, and nothing
    /// else writes them.
    pub(crate) unsafe fn write(&self, at: usize, data: &[u8]) {
        assert!(!self.mapped.is_null() && at + data.len() <= self.size as usize, "a write past upload memory");
        // SAFETY: the bytes are inside the mapping, which lives as long as
        // the memory, and the caller keeps them from the GPU.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.mapped.add(at), data.len()) };
    }
}

/// A texture's resource: placed in gpu-allocator's heaps, which
/// [`Memory::free_texture`] gives back, or the host's.
pub(crate) struct TextureMemory {
    pub(crate) resource: ID3D12Resource,
    /// gpu-allocator's resource, `None` for the host's.
    owner: Option<Owner>,
}

impl TextureMemory {
    /// The resource of a texture the host made, which stays the host's.
    pub(crate) fn imported(resource: ID3D12Resource) -> TextureMemory {
        TextureMemory { resource, owner: None }
    }
}

/// A placed buffer that a size's slabs share.
struct Page {
    resource: Resource,
    mapped: *mut u8,
}

/// The slabs of one location: pages for each size, and the free slabs of
/// each size.
#[derive(Default)]
struct Slabs {
    pages: Vec<Page>,
    free: [Vec<(usize, u64)>; SLAB_SIZES],
}

/// A `D3d12Gpu`'s allocator and slabs.
pub(crate) struct Memory {
    allocator: Allocator,
    slabs: [Slabs; LOCATIONS.len()],
}

impl Memory {
    pub(crate) fn new(device: &ID3D12Device) -> Result<Memory, GpuError> {
        const MB: u64 = 1024 * 1024;
        let allocator = Allocator::new(&AllocatorCreateDesc {
            device: ID3D12DeviceVersion::Device(device.clone()),
            debug_settings: AllocatorDebugSettings::default(),
            allocation_sizes: AllocationSizes::new(16 * MB, 8 * MB)
                .with_max_device_memblock_size(256 * MB)
                .with_max_host_memblock_size(64 * MB),
        })
        .map_err(allocation_error("the allocator"))?;
        Ok(Memory { allocator, slabs: Default::default() })
    }

    /// A resource of `desc` placed in a heap of `location`, in `state`.
    fn place(
        &mut self,
        name: &str,
        location: MemoryLocation,
        desc: &D3D12_RESOURCE_DESC,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<Resource, GpuError> {
        self.allocator
            .create_resource(&ResourceCreateDesc {
                name,
                memory_location: location,
                resource_category: ResourceCategory::from(desc),
                resource_desc: desc,
                castable_formats: &[],
                clear_value: None,
                initial_state_or_layout: ResourceStateOrBarrierLayout::ResourceState(state),
                resource_type: &ResourceType::Placed,
            })
            .map_err(allocation_error(name))
    }

    /// A buffer of `desc` placed in `location`, with its address as [`map`]
    /// gives it. A failed map frees the buffer.
    fn place_mapped(
        &mut self,
        name: &str,
        location: MemoryLocation,
        desc: &D3D12_RESOURCE_DESC,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<(Resource, *mut u8), GpuError> {
        let resource = self.place(name, location, desc, state)?;
        match map(resource.resource(), location) {
            Ok(mapped) => Ok((resource, mapped)),
            Err(error) => {
                if let Err(free) = self.allocator.free_resource(resource) {
                    Log::error(Event::Render, format!("freeing GPU memory failed: {free}"));
                }
                Err(error)
            }
        }
    }

    /// `size` bytes of a buffer in `location`, mapped unless only the GPU
    /// reaches it: a slab, or above the largest slab a buffer of its own.
    pub(crate) fn buffer(&mut self, location: MemoryLocation, size: u64) -> Result<BufferMemory, GpuError> {
        let size = size.max(MIN_SLAB);
        let slab = (size.next_power_of_two().trailing_zeros() - MIN_SLAB.trailing_zeros()) as usize;
        let index = location_index(location);
        // Upload memory starts in the state that reads it, readback memory in
        // the state that copies to it.
        let state = match location {
            MemoryLocation::CpuToGpu => D3D12_RESOURCE_STATE_GENERIC_READ,
            MemoryLocation::GpuToCpu => D3D12_RESOURCE_STATE_COPY_DEST,
            _ => D3D12_RESOURCE_STATE_COMMON,
        };
        if slab >= SLAB_SIZES {
            let (resource, mapped) = self.place_mapped("buffer", location, &buffer_desc(size), state)?;
            return Ok(BufferMemory {
                resource: resource.resource().clone(),
                offset: 0,
                size,
                mapped,
                owner: Owner::Resource(resource),
            });
        }
        if self.slabs[index].free[slab].is_empty() {
            let (resource, mapped) = self.place_mapped("slabs", location, &buffer_desc(SLAB_PAGE), state)?;
            let slabs = &mut self.slabs[index];
            let page = slabs.pages.len();
            slabs.pages.push(Page { resource, mapped });
            let slab_size = MIN_SLAB << slab;
            slabs.free[slab].extend((0..SLAB_PAGE / slab_size).rev().map(|n| (page, n * slab_size)));
        }
        let slabs = &mut self.slabs[index];
        let Some((page, offset)) = slabs.free[slab].pop() else {
            return Err(GpuError::Invalid("no free slab".to_owned()));
        };
        let Page { resource, mapped } = &slabs.pages[page];
        Ok(BufferMemory {
            resource: resource.resource().clone(),
            offset,
            size: MIN_SLAB << slab,
            // SAFETY: the offset is inside the page's mapping.
            mapped: if mapped.is_null() { std::ptr::null_mut() } else { unsafe { mapped.add(offset as usize) } },
            owner: Owner::Slab { location: index, size: slab, page, offset },
        })
    }

    /// A texture of `desc`, in `state`: with the 4 KiB alignment of a small
    /// texture where it can have it, a texture of `bytes` up to 64 KiB that
    /// isn't a render target.
    pub(crate) fn texture(
        &mut self,
        device: &ID3D12Device,
        desc: &D3D12_RESOURCE_DESC,
        bytes: u64,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<TextureMemory, GpuError> {
        let mut desc = *desc;
        let target = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
        if (desc.Flags & target).0 == 0 && bytes <= u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) {
            desc.Alignment = u64::from(D3D12_SMALL_RESOURCE_PLACEMENT_ALIGNMENT);
            // SAFETY: a plain query.
            let info = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(&desc)) };
            if info.Alignment != desc.Alignment {
                desc.Alignment = 0;
            }
        }
        let resource = self.place("texture", MemoryLocation::GpuOnly, &desc, state)?;
        Ok(TextureMemory { resource: resource.resource().clone(), owner: Some(Owner::Resource(resource)) })
    }

    /// How many pages the slabs of every location and size have.
    #[cfg(test)]
    pub(crate) fn slab_pages(&self) -> usize {
        self.slabs.iter().map(|slabs| slabs.pages.len()).sum()
    }

    /// Takes back `memory`, which no work that hasn't finished uses.
    pub(crate) fn free_buffer(&mut self, memory: BufferMemory) {
        self.free(memory.owner);
    }

    /// Takes back `memory`, which no work that hasn't finished uses, unless
    /// it is the host's.
    pub(crate) fn free_texture(&mut self, memory: TextureMemory) {
        if let Some(owner) = memory.owner {
            self.free(owner);
        }
    }

    fn free(&mut self, owner: Owner) {
        match owner {
            Owner::Slab { location, size, page, offset } => self.slabs[location].free[size].push((page, offset)),
            Owner::Resource(resource) => {
                if let Err(error) = self.allocator.free_resource(resource) {
                    Log::error(Event::Render, format!("freeing GPU memory failed: {error}"));
                }
            }
        }
    }
}

impl Drop for Memory {
    /// Frees the slabs' pages before the allocator goes. The `Gpu` has
    /// freed everything else of its memory.
    fn drop(&mut self) {
        for slabs in &mut self.slabs {
            for page in slabs.pages.drain(..) {
                let _ = self.allocator.free_resource(page.resource);
            }
        }
    }
}

/// The CPU's address of `resource` in upload memory, which stays mapped as
/// long as it lives; null elsewhere: readback memory is mapped for each
/// read, with the range the CPU reads.
fn map(resource: &ID3D12Resource, location: MemoryLocation) -> Result<*mut u8, GpuError> {
    if location != MemoryLocation::CpuToGpu {
        return Ok(std::ptr::null_mut());
    }
    let mut data = std::ptr::null_mut();
    // SAFETY: a buffer in upload memory, which the CPU only writes.
    unsafe { resource.Map(0, Some(&raw const NO_RANGE), Some(&raw mut data)) }
        .map_err(|error| GpuError::Invalid(format!("mapping a buffer: {error}")))?;
    Ok(data.cast())
}
