//! How a mesh reads its vertices: each of its [`VertexBinding`]s reads one
//! [`VertexBuffer`] as the attributes of a layout, which a vertex type
//! declares ([`Vertex`], [`vertex_attributes!`](crate::vertex_attributes))
//! or a layer makes, as the paint attributes are.

use std::borrow::Cow;

use mln_gfx::gfx_types::AttributeDataType;

use crate::draw::vertex_attribute::{SharedAttribute, VertexAttributeArray};
use crate::draw::vertices::VertexBuffer;

/// An attribute of a vertex layout: the program attribute `name`, read as
/// `format` at `offset` bytes into each vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexAttribute {
    pub name: &'static str,
    pub format: AttributeDataType,
    pub offset: u32,
}

/// A `#[repr(C)]` vertex type, and the attributes a program reads of it.
/// [`vertex_attributes!`](crate::vertex_attributes) declares them.
pub trait Vertex: bytemuck::Pod {
    const ATTRIBUTES: &'static [VertexAttribute];
}

/// Declares the attributes of a `#[repr(C)]` vertex type: each field, the
/// program attribute it binds to and the format the program reads it as.
///
/// ```ignore
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
/// struct LineVertex {
///     pos_normal: [i16; 2],
///     data: [u8; 4],
/// }
/// mln_engine::vertex_attributes!(LineVertex { pos_normal => "pos_normal": Short2, data => "data": UByte4 });
/// ```
#[macro_export]
macro_rules! vertex_attributes {
    ($vertex:ty { $($field:ident => $name:literal: $format:ident),+ $(,)? }) => {
        impl $crate::draw::Vertex for $vertex {
            const ATTRIBUTES: &'static [$crate::draw::VertexAttribute] = &[$(
                $crate::draw::VertexAttribute {
                    name: $name,
                    format: $crate::__private::mln_gfx::gfx_types::AttributeDataType::$format,
                    offset: ::core::mem::offset_of!($vertex, $field) as u32,
                },
            )+];
        }
    };
}

/// A vertex buffer a mesh reads, and how: its vertices of `stride` bytes,
/// from the vertex `first_vertex` on, as `attributes`. Clones read the same
/// buffer, which meshes share.
#[derive(Clone, Debug)]
pub struct VertexBinding {
    pub buffer: VertexBuffer,
    pub stride: u32,
    pub first_vertex: u32,
    pub attributes: Cow<'static, [VertexAttribute]>,
}

impl VertexBinding {
    /// `buffer`, of `stride`-byte vertices, read as `attributes`.
    #[must_use]
    pub fn new(buffer: &VertexBuffer, stride: u32, attributes: impl Into<Cow<'static, [VertexAttribute]>>) -> Self {
        VertexBinding { buffer: buffer.clone(), stride, first_vertex: 0, attributes: attributes.into() }
    }

    /// `buffer`, of vertices of the type `V`, read as its attributes.
    #[must_use]
    pub fn of<V: Vertex>(buffer: &VertexBuffer) -> Self {
        VertexBinding::new(buffer, size_of::<V>() as u32, V::ATTRIBUTES)
    }

    /// The binding, read from the vertex `first_vertex` on.
    #[must_use]
    pub fn starting_at(self, first_vertex: u32) -> Self {
        VertexBinding { first_vertex, ..self }
    }

    /// The binding, read from `buffer`, in its layout.
    #[must_use]
    pub fn with_buffer(self, buffer: &VertexBuffer) -> Self {
        VertexBinding { buffer: buffer.clone(), ..self }
    }

    /// The attribute named `name`, if the binding has it.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&VertexAttribute> {
        self.attributes.iter().find(|attribute| attribute.name == name)
    }

    /// Binds the attributes in `attrs`, in place of those of their names.
    pub(crate) fn bind(&self, attrs: &mut VertexAttributeArray) {
        for (name, shared) in self.shared() {
            attrs.set_shared(name, shared);
        }
    }

    /// The attributes, as an attribute array holds them.
    fn shared(&self) -> impl Iterator<Item = (&'static str, SharedAttribute)> + '_ {
        self.attributes.iter().map(|attribute| {
            let shared = SharedAttribute::new(self.buffer.clone(), attribute.format, attribute.offset, self.stride)
                .starting_at(self.first_vertex);
            (attribute.name, shared)
        })
    }
}

/// Makes the attributes of `bindings` those of `attrs`, in place of what it
/// had.
pub(crate) fn replace_attributes<'b>(
    attrs: &mut VertexAttributeArray,
    bindings: impl IntoIterator<Item = &'b VertexBinding>,
) {
    attrs.replace(bindings.into_iter().flat_map(VertexBinding::shared));
}

/// The attributes of `bindings`.
pub(crate) fn attributes_of(bindings: &[VertexBinding]) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    for binding in bindings {
        binding.bind(&mut attrs);
    }
    attrs
}
