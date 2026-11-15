//! Persistently mapped upload memory that writes are copied from: pages
//! that a submission's writes take room in, reused once it finishes.

use mln_gfx::gpu::GpuError;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ, ID3D12Device, ID3D12Resource,
};

use crate::gpu::{NO_RANGE, buffer_desc, create_resource};

/// The size of a page, and of the writes that get one of their own past it.
const PAGE: u64 = 1 << 20;

struct Page {
    resource: ID3D12Resource,
    data: *mut u8,
    size: u64,
    /// The bytes taken by the submission that `serial` names.
    taken: u64,
    /// The last submission whose writes took room in the page.
    serial: u64,
}

/// Room in upload memory: its page's resource and the offset in it.
pub(crate) struct Upload {
    pub(crate) resource: ID3D12Resource,
    pub(crate) offset: u64,
}

#[derive(Default)]
pub(crate) struct UploadRing {
    pages: Vec<Page>,
    /// The page that the submission being recorded takes room in.
    current: Option<usize>,
}

impl UploadRing {
    /// Copies `data` into `size` bytes, from an offset of `alignment`, for
    /// the submission `serial`, once `completed` has finished.
    pub(crate) fn write(
        &mut self,
        device: &ID3D12Device,
        serial: u64,
        completed: u64,
        size: u64,
        alignment: u64,
        data: impl FnOnce(&mut [u8]),
    ) -> Result<Upload, GpuError> {
        let fits = |page: &Page| page.taken.next_multiple_of(alignment) + size <= page.size;
        let index = match self.current {
            Some(index) if self.pages[index].serial == serial && fits(&self.pages[index]) => index,
            _ => {
                let reusable = self.pages.iter().position(|page| page.serial <= completed && size <= page.size);
                let index = if let Some(index) = reusable {
                    index
                } else {
                    let page_size = size.max(PAGE);
                    let resource = create_resource(
                        device,
                        D3D12_HEAP_TYPE_UPLOAD,
                        &buffer_desc(page_size),
                        D3D12_RESOURCE_STATE_GENERIC_READ,
                    )?;
                    let mut data = std::ptr::null_mut();
                    // SAFETY: an upload buffer stays mapped as long as
                    // it lives; the CPU reads none of it.
                    unsafe { resource.Map(0, Some(&raw const NO_RANGE), Some(&raw mut data)) }
                        .map_err(|error| GpuError::Invalid(format!("mapping upload memory: {error}")))?;
                    self.pages.push(Page { resource, data: data.cast(), size: page_size, taken: 0, serial });
                    self.pages.len() - 1
                };
                let page = &mut self.pages[index];
                page.taken = 0;
                page.serial = serial;
                self.current = Some(index);
                index
            }
        };
        let page = &mut self.pages[index];
        let offset = page.taken.next_multiple_of(alignment);
        page.taken = offset + size;
        // SAFETY: the bytes are inside the mapped page, and no submission
        // that read them is running: they are past the last one's, or the
        // page's submission finished.
        let bytes = unsafe { std::slice::from_raw_parts_mut(page.data.add(offset as usize), size as usize) };
        data(bytes);
        Ok(Upload { resource: page.resource.clone(), offset })
    }
}
