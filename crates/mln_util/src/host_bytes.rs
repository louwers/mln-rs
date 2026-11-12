//! Port-only: bytes that stay in a JS `Uint8Array`, outside wasm memory.
//!
//! A tile worker's reply reaches the map's instance as JS arrays
//! (`serialization::payload`). The vertex and index arrays among them can go
//! from there to the GPU (`GPUQueue.writeBuffer`) without a copy in wasm
//! memory; a reader that needs the bytes themselves copies them in
//! (`to_vec`, `to_pod_vec`).
//!
//! The buckets that hold them are `Send` and `Sync`, and a JS value is
//! neither, so a `HostBytes` is a key into a table of its thread. The
//! instance has only that thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

thread_local! {
    static ARRAYS: RefCell<HashMap<u64, js_sys::Uint8Array>> = RefCell::default();
    static NEXT_KEY: Cell<u64> = const { Cell::new(0) };
}

/// A JS array's bytes, which live as long as this does.
pub struct HostBytes {
    key: u64,
    len: usize,
}

impl HostBytes {
    #[must_use]
    pub fn new(array: js_sys::Uint8Array) -> Self {
        let key = NEXT_KEY.with(|next| next.replace(next.get() + 1));
        let len = array.length() as usize;
        ARRAYS.with(|arrays| arrays.borrow_mut().insert(key, array));
        HostBytes { key, len }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The array, or `None` on a thread other than the one that made it.
    #[must_use]
    pub fn array(&self) -> Option<js_sys::Uint8Array> {
        let array = ARRAYS.with(|arrays| arrays.borrow().get(&self.key).cloned());
        debug_assert!(array.is_some(), "host bytes used on another thread");
        array
    }

    /// A copy of the bytes in wasm memory.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        self.to_pod_vec()
    }

    /// A copy of the bytes in wasm memory, as plain values; `len` must be a
    /// whole number of them.
    #[must_use]
    pub fn to_pod_vec<T: bytemuck::Pod>(&self) -> Vec<T> {
        debug_assert!(self.len.is_multiple_of(size_of::<T>()));
        let mut values = vec![T::zeroed(); self.len / size_of::<T>()];
        if let Some(array) = self.array() {
            array.copy_to(bytemuck::cast_slice_mut(&mut values));
        }
        values
    }
}

impl Clone for HostBytes {
    /// Another key to the same array, whose bytes nothing changes.
    fn clone(&self) -> Self {
        self.array().map_or(HostBytes { key: u64::MAX, len: self.len }, HostBytes::new)
    }
}

impl Drop for HostBytes {
    fn drop(&mut self) {
        // The table can be gone already when this drops as a thread exits.
        let _ = ARRAYS.try_with(|arrays| arrays.borrow_mut().remove(&self.key));
    }
}

impl PartialEq for HostBytes {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.to_vec() == other.to_vec()
    }
}

impl Eq for HostBytes {}

impl std::fmt::Debug for HostBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBytes").field("len", &self.len).finish_non_exhaustive()
    }
}
