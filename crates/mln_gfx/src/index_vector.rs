use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;

/// A backend buffer attached to a shared index vector.
pub trait IndexBufferBase: Any {}

impl dyn IndexBufferBase {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: IndexBufferBase>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

/// Indexes for a draw, and the backend buffer they were uploaded to.
/// `emplace_back_group` appends whole primitives of a draw mode.
#[derive(Default)]
pub struct IndexVectorBase {
    v: Vec<u16>,
    /// Port-only: the indexes while they are in a JS array; `v` is empty
    /// then.
    #[cfg(target_family = "wasm")]
    host: Option<HostBytes>,
    buffer: Option<Box<dyn IndexBufferBase>>,
    dirty: bool,
    released: bool,
    /// The element count of the CPU data freed after upload.
    freed_count: usize,
}

/// A shared index vector. The upload pass attaches the buffer through the
/// shared handle, hence the `RefCell`.
pub type IndexVectorBasePtr = Rc<RefCell<IndexVectorBase>>;

impl IndexVectorBase {
    #[must_use]
    pub fn new(indexes: Vec<u16>) -> Self {
        IndexVectorBase {
            v: indexes,
            #[cfg(target_family = "wasm")]
            host: None,
            buffer: None,
            dirty: true,
            released: false,
            freed_count: 0,
        }
    }

    /// A vector holding `indexes` that will not be updated (`release()`d):
    /// its CPU copy is freed once it is uploaded.
    #[must_use]
    pub fn new_released(indexes: Vec<u16>) -> Self {
        let mut result = IndexVectorBase::new(indexes);
        result.release();
        result
    }

    /// Port-only: a released vector of the indexes in a JS array, which the
    /// upload pass writes to the GPU from there. Released, it frees them once
    /// it is uploaded, so no buffer is ever updated from them.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn from_host_released(indexes: HostBytes) -> Self {
        debug_assert!(indexes.len().is_multiple_of(size_of::<u16>()));
        let mut result = IndexVectorBase::new(Vec::new());
        result.host = Some(indexes);
        result.release();
        result
    }

    /// Port-only: the indexes while they are in a JS array rather than in
    /// wasm memory (`data` is empty then), until they are freed after upload.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn host_data(&self) -> Option<&HostBytes> {
        self.host.as_ref()
    }

    /// The index count in a JS array.
    #[cfg_attr(
        not(target_family = "wasm"),
        expect(clippy::unused_self, reason = "only the wasm build keeps indexes in a JS array")
    )]
    fn host_count(&self) -> usize {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = &self.host {
            return bytes.len() / size_of::<u16>();
        }
        0
    }

    #[must_use]
    pub fn get_buffer(&self) -> Option<&dyn IndexBufferBase> {
        self.buffer.as_deref()
    }
    pub fn set_buffer(&mut self, value: Option<Box<dyn IndexBufferBase>>) {
        self.buffer = value;
    }
    #[must_use]
    pub fn get_dirty(&self) -> bool {
        self.dirty
    }
    pub fn set_dirty(&mut self, value: bool) {
        self.dirty = value;
        // Clean and uploaded: a released vector will not be modified or re-uploaded.
        if !self.dirty && self.released && self.buffer.is_some() {
            self.free_raw_data();
        }
    }
    pub fn at_mut(&mut self, n: usize) -> &mut u16 {
        debug_assert!(!self.released);
        self.dirty = true;
        &mut self.v[n]
    }
    /// The element count, which stays when the CPU copy is freed.
    #[must_use]
    pub fn elements(&self) -> usize {
        self.v.len() + self.host_count() + self.freed_count
    }
    #[must_use]
    pub fn empty(&self) -> bool {
        self.elements() == 0
    }
    /// Indicate that this shared index vector will no longer be updated.
    pub(crate) fn release(&mut self) {
        self.released = true;
        // If we've already created a buffer, we don't need the raw data any more.
        if self.buffer.is_some() && !self.dirty {
            self.free_raw_data();
        }
    }
    fn free_raw_data(&mut self) {
        self.freed_count += self.v.len() + self.host_count();
        self.v = Vec::new();
        #[cfg(target_family = "wasm")]
        {
            self.host = None;
        }
    }
    /// The CPU copy of the indexes, empty once it is freed after upload or
    /// while the indexes are in a JS array.
    #[must_use]
    pub fn data(&self) -> &[u16] {
        &self.v
    }
    #[must_use]
    pub fn vector(&self) -> &Vec<u16> {
        &self.v
    }
}
