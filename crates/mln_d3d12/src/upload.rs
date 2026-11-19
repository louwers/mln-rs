//! Upload memory that the copies of vertex, index and texture writes read:
//! pages that a batch's writes take room in, reused once it finishes.

use gpu_allocator::MemoryLocation;
use mln_gfx::gpu::GpuError;
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::memory::{BufferMemory, Memory};

/// The size of a page, and of the writes that get one of their own past it.
const PAGE: u64 = 1 << 20;

struct Page {
    memory: BufferMemory,
    /// The bytes taken by the batch that `serial` names.
    taken: u64,
    /// The last batch whose writes took room in the page.
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
    /// The page that the batch being recorded takes room in.
    current: Option<usize>,
}

impl UploadRing {
    /// Fills `size` bytes, from an offset of `alignment`, for the batch
    /// `serial`, once `completed` has finished.
    pub(crate) fn write(
        &mut self,
        memory: &mut Memory,
        serial: u64,
        completed: u64,
        size: u64,
        alignment: u64,
        data: impl FnOnce(&mut [u8]),
    ) -> Result<Upload, GpuError> {
        let fits = |page: &Page| page.taken.next_multiple_of(alignment) + size <= page.memory.size;
        let index = match self.current {
            Some(index) if self.pages[index].serial == serial && fits(&self.pages[index]) => index,
            _ => {
                let reusable = self.pages.iter().position(|page| page.serial <= completed && size <= page.memory.size);
                let index = if let Some(index) = reusable {
                    index
                } else {
                    let memory = memory.buffer(MemoryLocation::CpuToGpu, size.max(PAGE))?;
                    self.pages.push(Page { memory, taken: 0, serial });
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
        // SAFETY: no batch that reads these bytes runs: they are past the
        // current batch's, or the page's last batch finished.
        let bytes = unsafe { page.memory.bytes_mut() };
        data(&mut bytes[offset as usize..(offset + size) as usize]);
        Ok(Upload { resource: page.memory.resource.clone(), offset: page.memory.offset + offset })
    }

    /// Gives the pages back to `memory`, once no work reads them.
    pub(crate) fn free(&mut self, memory: &mut Memory) {
        self.current = None;
        for page in self.pages.drain(..) {
            memory.free_buffer(page.memory);
        }
    }
}
