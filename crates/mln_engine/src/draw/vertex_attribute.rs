//! The vertex attributes of a draw, by the names of the program's
//! attributes they bind to: each reads either values of its own, which the
//! draw owns and uploads to a buffer of its own, or vertices of the context's
//! store that other draws share.

use std::sync::atomic::{AtomicU64, Ordering};

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{BufferUsage, Gpu, GpuError};
use mln_gfx::shaders::bindings::MAX_ATTRIBUTES;

use crate::draw::buffer::Buffer;
use crate::draw::context::Context;
use crate::draw::upload::{AttributeBinding, AttributeDescriptor};
use crate::draw::vertices::{SharedVertices, Vertices};

/// Where an attribute reads vertices of the context's store: at `offset`
/// in each of `stride` bytes, from the vertex `vertex_offset` on, as
/// `data_type`.
#[derive(Clone)]
pub(crate) struct SharedAttribute {
    pub(crate) vertices: SharedVertices,
    pub(crate) data_type: AttributeDataType,
    pub(crate) offset: u32,
    pub(crate) vertex_offset: u32,
    pub(crate) stride: u32,
}

impl SharedAttribute {
    /// The attribute that reads `vertices` at `offset` in each of `stride`
    /// bytes, from the first vertex on, as `data_type`.
    pub(crate) fn new(vertices: SharedVertices, data_type: AttributeDataType, offset: u32, stride: u32) -> Self {
        SharedAttribute { vertices, data_type, offset, vertex_offset: 0, stride }
    }

    /// The attribute, reading from the vertex `vertex_offset` on.
    pub(crate) fn starting_at(self, vertex_offset: u32) -> Self {
        SharedAttribute { vertex_offset, ..self }
    }
}

/// Where an attribute's own values are: in their bytes until the first
/// upload moves them into a buffer, which the draw owns.
pub(crate) enum OwnValues {
    Bytes(Vec<u8>),
    Buffer(Buffer),
}

/// What a vertex attribute reads.
pub(crate) enum VertexAttribute {
    /// Values of its own, of `data_type`, `count` of them.
    Own { data_type: AttributeDataType, count: usize, values: OwnValues },
    /// Vertices of the context's store.
    Shared(SharedAttribute),
}

impl VertexAttribute {
    /// The number of values the attribute reads.
    pub(crate) fn count(&self) -> usize {
        match self {
            VertexAttribute::Own { count, .. } => *count,
            VertexAttribute::Shared(shared) => shared.vertices.count(),
        }
    }

    /// Where the attribute reads vertices of the context's store, if it
    /// does.
    pub(crate) fn shared(&self) -> Option<&SharedAttribute> {
        match self {
            VertexAttribute::Shared(shared) => Some(shared),
            VertexAttribute::Own { .. } => None,
        }
    }

    /// Whether the next upload writes what the attribute reads: its own
    /// values before their first upload, or shared vertices that are new
    /// or updated since theirs.
    pub(crate) fn needs_upload(&self, context: &Context) -> bool {
        match self {
            VertexAttribute::Own { values, .. } => matches!(values, OwnValues::Bytes(_)),
            VertexAttribute::Shared(shared) => context.vertices(&shared.vertices).is_some_and(Vertices::needs_upload),
        }
    }

    /// The attribute's binding, of the buffer it reads, which this uploads
    /// to `gpu` where [`needs_upload`](Self::needs_upload) says; `None`
    /// where there are no values.
    pub(crate) fn binding(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
    ) -> Result<Option<AttributeBinding>, GpuError> {
        match self {
            VertexAttribute::Own { data_type, values, .. } => {
                let buffer = match values {
                    OwnValues::Buffer(buffer) => buffer.id(),
                    OwnValues::Bytes(bytes) => {
                        let buffer = context.device().buffer(gpu, BufferUsage::Vertex, bytes)?;
                        let id = buffer.id();
                        *values = OwnValues::Buffer(buffer);
                        id
                    }
                };
                Ok(Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: *data_type, offset: 0 },
                    vertex_stride: data_type.stride() as u32,
                    buffer,
                    vertex_offset: 0,
                    held: None,
                }))
            }
            VertexAttribute::Shared(shared) => {
                let Some(buffer) = context.upload_vertices(gpu, &shared.vertices)? else {
                    return Ok(None);
                };
                debug_assert!(
                    context
                        .vertices(&shared.vertices)
                        .is_none_or(|vertices| (shared.stride as usize) * (shared.vertex_offset as usize)
                            < vertices.vertex_size() * vertices.count())
                );
                Ok(Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: shared.data_type, offset: shared.offset },
                    vertex_stride: shared.stride,
                    buffer,
                    vertex_offset: shared.vertex_offset,
                    held: Some(shared.vertices.held()),
                }))
            }
        }
    }
}

/// The paint properties a draw reads from their uniforms instead, by name.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Uniforms {
    names: [&'static str; MAX_ATTRIBUTES],
    len: usize,
}

impl Uniforms {
    pub(crate) fn insert(&mut self, name: &'static str) {
        if self.contains(name) {
            return;
        }
        debug_assert!(self.len < self.names.len(), "more uniforms than attributes");
        if let Some(slot) = self.names.get_mut(self.len) {
            *slot = name;
            self.len += 1;
        }
    }

    #[must_use]
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.names[..self.len].contains(&name)
    }
}

impl FromIterator<&'static str> for Uniforms {
    fn from_iter<I: IntoIterator<Item = &'static str>>(names: I) -> Uniforms {
        let mut uniforms = Uniforms::default();
        for name in names {
            uniforms.insert(name);
        }
        uniforms
    }
}

/// Vertex attributes by the names of the program's attributes they bind to,
/// each in a slot of the array, which [`slot_of`](Self::slot_of) finds.
#[derive(Default)]
pub(crate) struct VertexAttributeArray {
    attrs: [Option<Box<(&'static str, VertexAttribute)>>; MAX_ATTRIBUTES],
    /// Which names its slots hold ([`layout`](Self::layout)).
    layout: u64,
}

/// A layout of no other array: 0 is every empty array's.
fn new_layout() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl VertexAttributeArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The fewest values any of the attributes reads, `usize::MAX` without
    /// attributes.
    #[must_use]
    pub(crate) fn min_count(&self) -> usize {
        self.iter().map(|(_, attr)| attr.count()).min().unwrap_or(usize::MAX)
    }

    /// The slot of the attribute `name`.
    #[must_use]
    pub(crate) fn slot_of(&self, name: &str) -> Option<usize> {
        self.attrs.iter().position(|attr| attr.as_ref().is_some_and(|attr| attr.0 == name))
    }

    #[must_use]
    pub(crate) fn get(&self, name: &str) -> Option<&VertexAttribute> {
        self.at(self.slot_of(name)?)
    }

    /// The attribute in `slot`.
    #[must_use]
    pub(crate) fn at(&self, slot: usize) -> Option<&VertexAttribute> {
        self.attrs.get(slot)?.as_ref().map(|attr| &attr.1)
    }

    /// The attribute in `slot`, to change.
    pub(crate) fn at_mut(&mut self, slot: usize) -> Option<&mut VertexAttribute> {
        self.attrs.get_mut(slot)?.as_mut().map(|attr| &mut attr.1)
    }

    /// Which names the slots hold: arrays of one layout hold the same names
    /// in the same slots. An array keeps its layout while it only gives
    /// attributes of its names other values, and takes one of no other
    /// array's when it gives a slot a name.
    #[must_use]
    pub(crate) fn layout(&self) -> u64 {
        self.layout
    }

    /// Makes the attribute `name` read `bytes`, its own values of
    /// `data_type`.
    pub(crate) fn set_own(&mut self, name: &'static str, data_type: AttributeDataType, bytes: Vec<u8>) {
        let count = bytes.len().checked_div(data_type.stride()).unwrap_or(0);
        self.set(name, VertexAttribute::Own { data_type, count, values: OwnValues::Bytes(bytes) });
    }

    /// Makes the attribute `name` read vertices of the context's store as
    /// `shared` says.
    pub(crate) fn set_shared(&mut self, name: &'static str, shared: SharedAttribute) {
        self.set(name, VertexAttribute::Shared(shared));
    }

    /// Makes `attribute` the one of `name`, in its slot or the first empty
    /// one.
    fn set(&mut self, name: &'static str, attribute: VertexAttribute) {
        if let Some(slot) = self.slot_of(name).and_then(|slot| self.attrs.get_mut(slot)) {
            *slot = Some(Box::new((name, attribute)));
            return;
        }
        let slot = self.attrs.iter_mut().find(|slot| slot.is_none());
        debug_assert!(slot.is_some(), "more attributes than a shader has");
        if let Some(slot) = slot {
            *slot = Some(Box::new((name, attribute)));
            self.layout = new_layout();
        }
    }

    /// Where the attributes that read vertices of the context's store read
    /// them, to change.
    pub(crate) fn shared_mut(&mut self) -> impl Iterator<Item = &mut SharedAttribute> {
        self.attrs.iter_mut().filter_map(|attr| match &mut attr.as_mut()?.1 {
            VertexAttribute::Shared(shared) => Some(shared),
            VertexAttribute::Own { .. } => None,
        })
    }

    /// Whether an attribute has values of its own, in a buffer the draw
    /// owns.
    pub(crate) fn has_own(&self) -> bool {
        self.iter().any(|(_, attr)| matches!(attr, VertexAttribute::Own { .. }))
    }

    /// The attributes, with their names.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'static str, &VertexAttribute)> {
        self.attrs.iter().filter_map(|attr| attr.as_ref().map(|attr| (attr.0, &attr.1)))
    }
}
