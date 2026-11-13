// The WebGPU subclasses (include/mln/webgpu/vertex_attribute.hpp) add no
// data, so `create()`/`copy()` are not virtual here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_util::time::Instant;

use crate::gfx_types::AttributeDataType;
use crate::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER;
use crate::vertex_buffer::UniqueVertexBufferResource;
use crate::vertex_vector::VertexVectorBasePtr;

/// The value of one vertex attribute element, in C++'s variant order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ElementType {
    Int(i32),
    UShort8([u16; 8]),
    Int2([i32; 2]),
    Int3([i32; 3]),
    Int4([i32; 4]),
    Float(f32),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
    MatF3([f32; 9]),
    MatF4([f32; 16]),
}

impl Default for ElementType {
    /// A value-initialized variant holds its first alternative.
    fn default() -> Self {
        ElementType::Int(0)
    }
}

pub struct VertexAttribute {
    index: i32,
    stride: usize,
    dirty: Cell<bool>,
    data_type: AttributeDataType,
    items: Vec<ElementType>,
    raw_data: Vec<u8>,
    shared_raw_data: Option<VertexVectorBasePtr>,
    shared_type: AttributeDataType,
    shared_offset: u32,
    shared_vertex_offset: u32,
    shared_stride: u32,
    buffer: Option<UniqueVertexBufferResource>,
}

impl VertexAttribute {
    #[must_use]
    pub fn with_stride(index: i32, data_type: AttributeDataType, count: usize, stride: usize) -> Self {
        VertexAttribute {
            index,
            stride,
            dirty: Cell::new(true),
            data_type,
            items: vec![ElementType::default(); count],
            raw_data: Vec::new(),
            shared_raw_data: None,
            shared_type: AttributeDataType::Invalid,
            shared_offset: 0,
            shared_vertex_offset: 0,
            shared_stride: 0,
            buffer: None,
        }
    }

    #[must_use]
    pub fn new(index: i32, data_type: AttributeDataType, count: usize) -> Self {
        Self::with_stride(index, data_type, count, Self::get_stride_of(data_type))
    }

    /// The C++ copy constructor: everything but the raw data and the buffer.
    #[must_use]
    pub fn copy(&self) -> Self {
        VertexAttribute {
            index: self.index,
            stride: self.stride,
            dirty: Cell::new(true),
            data_type: self.data_type,
            items: self.items.clone(),
            raw_data: Vec::new(),
            shared_raw_data: self.shared_raw_data.clone(),
            shared_type: self.shared_type,
            shared_offset: self.shared_offset,
            shared_vertex_offset: self.shared_vertex_offset,
            shared_stride: self.shared_stride,
            buffer: None,
        }
    }

    #[must_use]
    pub fn get_stride_of(type_: AttributeDataType) -> usize {
        use AttributeDataType::{
            Byte, Byte2, Byte3, Byte4, Float, Float2, Float3, Float4, Int, Int2, Int3, Int4, Invalid, Short, Short2,
            Short3, Short4, UByte, UByte2, UByte3, UByte4, UByte4Normalized, UInt, UInt2, UInt3, UInt4, UShort,
            UShort2, UShort3, UShort4, UShort8,
        };
        match type_ {
            Byte | UByte => 1,
            Byte2 | UByte2 | Short | UShort => 2,
            Byte3 | UByte3 => 3,
            Byte4 | UByte4 | UByte4Normalized | Short2 | UShort2 | Int | UInt | Float => 4,
            Short3 | UShort3 => 6,
            Short4 | UShort4 | Int2 | UInt2 | Float2 => 8,
            Int3 | UInt3 | Float3 => 12,
            UShort8 | Int4 | UInt4 | Float4 => 16,
            Invalid => 0,
        }
    }

    pub fn get_index(&self) -> i32 {
        self.index
    }
    pub fn set_index(&mut self, value: i32) {
        self.index = value;
    }
    pub fn get_stride(&self) -> usize {
        self.stride
    }
    pub fn set_stride(&mut self, value: usize) {
        self.stride = value;
    }

    pub fn get_count(&self) -> usize {
        match &self.shared_raw_data {
            Some(shared) => shared.get_raw_count(),
            None => self.items.len(),
        }
    }

    pub fn get_data_type(&self) -> AttributeDataType {
        self.data_type
    }
    pub fn get(&self, i: usize) -> &ElementType {
        &self.items[i]
    }
    pub fn reserve(&mut self, count: usize) {
        self.items.reserve(count);
    }

    /// Sets element `i`, marking the attribute dirty when the value (or its
    /// variant) changes.
    pub fn set(&mut self, i: usize, value: ElementType) -> &ElementType {
        if self.items.len() < i + 1 {
            let n = self.items.len().max(i + 1);
            self.items.resize(n, ElementType::default());
            self.set_dirty(true); // need to rebuild the raw data next time
        }
        if !self.is_dirty() && self.items[i] != value {
            // Covers both "same alternative, different value" and "different
            // alternative" (enum equality compares the discriminant first).
            self.set_dirty(true);
        }
        self.items[i] = value;
        &self.items[i]
    }

    pub fn set_variant(&mut self, i: usize, value: ElementType) -> &ElementType {
        if self.items.len() < i + 1 {
            let n = self.items.len().max(i + 1);
            self.items.resize(n, ElementType::default());
        }
        self.set_dirty(true);
        self.items[i] = value;
        &self.items[i]
    }

    pub fn clear(&mut self) {
        if !self.items.is_empty() {
            self.set_dirty(true);
        }
        self.items.clear();
    }

    /// `dirty` is `mutable` in C++.
    pub fn set_dirty(&self, value: bool) {
        self.dirty.set(value);
    }

    pub fn is_modified_after(&self, time: Instant) -> bool {
        if let Some(shared) = &self.shared_raw_data {
            return shared.is_modified_after(time);
        }
        self.dirty.get()
    }

    pub fn get_raw_data(&self) -> &Vec<u8> {
        &self.raw_data
    }
    pub fn get_raw_data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.raw_data
    }
    pub fn set_raw_data(&mut self, value: Vec<u8>) {
        self.raw_data = value;
    }

    pub fn get_shared_raw_data(&self) -> Option<&VertexVectorBasePtr> {
        self.shared_raw_data.as_ref()
    }
    pub fn get_shared_type(&self) -> AttributeDataType {
        self.shared_type
    }
    pub fn get_shared_offset(&self) -> u32 {
        self.shared_offset
    }
    pub fn get_shared_vertex_offset(&self) -> u32 {
        self.shared_vertex_offset
    }
    pub fn get_shared_stride(&self) -> u32 {
        self.shared_stride
    }

    pub fn set_shared_raw_data(
        &mut self,
        data: Option<VertexVectorBasePtr>,
        offset: u32,
        vertex_offset: u32,
        stride: u32,
        type_: AttributeDataType,
    ) {
        self.shared_raw_data = data;
        self.shared_type = type_;
        self.shared_offset = offset;
        self.shared_vertex_offset = vertex_offset;
        self.shared_stride = stride;
    }

    pub fn reset_shared_raw_data(&mut self) {
        self.shared_raw_data = None;
    }

    pub fn get_buffer(&self) -> Option<&UniqueVertexBufferResource> {
        self.buffer.as_ref()
    }
    pub fn set_buffer(&mut self, value: Option<UniqueVertexBufferResource>) {
        self.buffer = value;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }
}

pub type UniqueVertexAttribute = Box<VertexAttribute>;

/// Vertex attributes by id.
#[derive(Default)]
pub struct VertexAttributeArray {
    attrs: [Option<UniqueVertexAttribute>; MAX_ATTRIBUTE_COUNT_PER_SHADER],
}

/// A shared attribute array. Drawables, builders and upload passes mutate it
/// through the shared handle.
pub type VertexAttributeArrayPtr = Rc<RefCell<VertexAttributeArray>>;

impl VertexAttributeArray {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn new_ptr() -> VertexAttributeArrayPtr {
        Rc::new(RefCell::new(Self::new()))
    }

    #[must_use]
    pub fn allocated_size(&self) -> usize {
        self.attrs.len()
    }

    #[must_use]
    pub fn get_total_size(&self) -> usize {
        self.attrs.iter().map(|a| a.as_ref().map_or(0, |a| a.get_stride())).sum()
    }

    #[must_use]
    pub fn get_min_count(&self) -> usize {
        self.attrs.iter().fold(usize::MAX, |acc, attr| acc.min(attr.as_ref().map_or(usize::MAX, |a| a.get_count())))
    }

    #[must_use]
    pub fn get(&self, id: usize) -> Option<&VertexAttribute> {
        self.attrs.get(id).and_then(|a| a.as_deref())
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut VertexAttribute> {
        self.attrs.get_mut(id).and_then(|a| a.as_deref_mut())
    }

    /// `set(id, index = -1, type = Invalid, count = 0)`: replaces the
    /// attribute with a new one.
    pub fn set(
        &mut self,
        id: usize,
        index: i32,
        type_: AttributeDataType,
        count: usize,
    ) -> Option<&mut VertexAttribute> {
        debug_assert!(id < self.attrs.len());
        if id >= self.attrs.len() {
            return None;
        }
        self.attrs[id] = Some(Box::new(VertexAttribute::new(index, type_, count)));
        self.attrs[id].as_deref_mut()
    }

    #[must_use]
    pub fn is_modified_after(&self, time: Instant) -> bool {
        self.attrs.iter().any(|attr| attr.as_ref().is_some_and(|a| a.is_modified_after(time)))
    }

    pub fn clear(&mut self) {
        for attr in &mut self.attrs {
            *attr = None;
        }
    }

    pub fn visit_attributes(&self, mut f: impl FnMut(&VertexAttribute)) {
        for attr in self.attrs.iter().flatten() {
            f(attr);
        }
    }

    /// For each id, calls `found(id, default, override)` when this array has
    /// the attribute, otherwise `missing(id, override)` when only the
    /// override exists. The override is mutable, as through C++'s
    /// `unique_ptr` deref.
    pub fn resolve(
        &self,
        overrides: &mut VertexAttributeArray,
        mut found: impl FnMut(usize, &VertexAttribute, Option<&mut VertexAttribute>),
        mut missing: impl FnMut(usize, &mut VertexAttribute),
    ) {
        for id in 0..self.attrs.len() {
            let override_ = overrides.attrs[id].as_deref_mut();
            if let Some(attr) = self.attrs[id].as_deref() {
                found(id, attr, override_);
            } else if let Some(o) = override_ {
                missing(id, o);
            }
        }
    }
}
