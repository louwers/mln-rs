// Ports src/mln/gfx/vertex_vector.hpp

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_util::time::Instant;

/// `gfx::VertexBufferBase`: backend buffer attached to a shared vertex vector.
pub trait VertexBufferBase: Any {
    fn as_any(&self) -> &dyn Any;
}

/// The data members of `gfx::VertexVectorBase`. Interior mutability because
/// the vector is shared (`VertexVectorBasePtr`) and the upload pass attaches
/// buffers and modification times through the shared pointer.
pub struct VertexVectorBaseData {
    buffer: RefCell<Option<Box<dyn VertexBufferBase>>>,
    dirty: Cell<bool>,
    released: Cell<bool>,
    last_modified: Cell<Instant>,
}

impl Default for VertexVectorBaseData {
    fn default() -> Self {
        VertexVectorBaseData {
            buffer: RefCell::new(None),
            dirty: Cell::new(true),
            released: Cell::new(false),
            last_modified: Cell::new(Instant::now()),
        }
    }
}

/// `gfx::VertexVectorBase`
pub trait VertexVectorBase: Any {
    fn vertex_vector_base(&self) -> &VertexVectorBaseData;
    /// The raw bytes of all vertices (`getRawData()`, `getRawCount() * getRawSize()` bytes).
    fn get_raw_data(&self) -> &[u8];
    /// `sizeof(Vertex)`
    fn get_raw_size(&self) -> usize;
    fn get_raw_count(&self) -> usize;

    fn get_buffer(&self) -> std::cell::Ref<'_, Option<Box<dyn VertexBufferBase>>> {
        self.vertex_vector_base().buffer.borrow()
    }
    fn set_buffer(&self, value: Option<Box<dyn VertexBufferBase>>) {
        *self.vertex_vector_base().buffer.borrow_mut() = value;
    }
    fn get_last_modified(&self) -> Instant {
        self.vertex_vector_base().last_modified.get()
    }
    fn is_modified_after(&self, t: Instant) -> bool {
        t < self.vertex_vector_base().last_modified.get()
    }
    fn update_modified(&self, force: bool) {
        let base = self.vertex_vector_base();
        if base.dirty.get() || force {
            base.last_modified.set(Instant::now());
            base.dirty.set(false);
        }
    }
    /// Indicates that the owner/producer will not modify this again
    fn is_released(&self) -> bool {
        self.vertex_vector_base().released.get()
    }
}

pub type VertexVectorBasePtr = Rc<dyn VertexVectorBase>;

/// `gfx::VertexVector<V>` for plain-old-data vertex types.
pub struct VertexVector<V: bytemuck::Pod> {
    base: VertexVectorBaseData,
    v: Vec<V>,
}

impl<V: bytemuck::Pod> Default for VertexVector<V> {
    fn default() -> Self {
        VertexVector { base: VertexVectorBaseData::default(), v: Vec::new() }
    }
}

impl<V: bytemuck::Pod> VertexVector<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A vector holding `vertices` (C++ builds one with `emplace_back`; the
    /// renderer copies a bucket's vertices into a shared vector this way,
    /// see mln_renderer::bucket).
    pub fn from_vec(vertices: Vec<V>) -> Self {
        VertexVector { base: VertexVectorBaseData::default(), v: vertices }
    }

    pub fn emplace_back(&mut self, vertex: V) {
        debug_assert!(!self.base.released.get());
        self.v.push(vertex);
        self.base.dirty.set(true);
    }

    pub fn extend(&mut self, n: usize, val: V) {
        debug_assert!(!self.base.released.get());
        self.v.resize(self.v.len() + n, val);
        self.base.dirty.set(true);
    }

    pub fn at_mut(&mut self, n: usize) -> &mut V {
        debug_assert!(!self.base.released.get());
        self.base.dirty.set(true);
        &mut self.v[n]
    }

    pub fn at(&self, n: usize) -> &V {
        &self.v[n]
    }

    pub fn elements(&self) -> usize {
        self.v.len()
    }

    pub fn bytes(&self) -> usize {
        self.v.len() * std::mem::size_of::<V>()
    }

    pub fn empty(&self) -> bool {
        self.v.is_empty()
    }

    pub fn clear(&mut self) {
        self.base.dirty.set(true);
        self.v.clear();
    }

    pub fn reserve(&mut self, count: usize) {
        self.v.reserve(count);
    }

    /// Indicate that this shared vertex vector instance will no longer be updated.
    pub fn release(&mut self) {
        // If we've already created a buffer, we don't need the raw data any more.
        if self.base.buffer.borrow().is_some() {
            self.v.clear();
        }
        self.base.released.set(true);
    }

    pub fn data(&self) -> &[V] {
        &self.v
    }

    pub fn vector(&self) -> &Vec<V> {
        &self.v
    }
}

impl<V: bytemuck::Pod> VertexVectorBase for VertexVector<V> {
    fn vertex_vector_base(&self) -> &VertexVectorBaseData {
        &self.base
    }
    fn get_raw_data(&self) -> &[u8] {
        bytemuck::cast_slice(&self.v)
    }
    fn get_raw_size(&self) -> usize {
        std::mem::size_of::<V>()
    }
    fn get_raw_count(&self) -> usize {
        self.v.len()
    }
}
