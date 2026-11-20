//! MSL, which `mln_mtl` compiles.
//!
//! A buffer binds at the `[[buffer(n)]]` of its id. A draw's vertex buffers
//! bind one after the other from the lowest [`Attribute::buffer_index`] of
//! the program's tables, which is past the indexes of its buffers.

use crate::gfx_types::AttributeDataType;

/// A program's MSL, and its per-vertex inputs, per-instance inputs and
/// textures.
#[derive(Debug)]
pub struct Source {
    /// The shader.
    pub msl: &'static Msl,
    /// The inputs that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The inputs that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [Texture],
}

/// The MSL of a shader. Its library compiles from the backend's prelude,
/// `prelude` and `source`.
#[derive(Debug)]
pub struct Msl {
    /// The name of the vertex function.
    pub vertex_main_function: &'static str,
    /// The name of the fragment function.
    pub fragment_main_function: &'static str,
    /// The shader's own prelude constant (its UBO ids and structs), "" if none.
    pub prelude: &'static str,
    /// The functions.
    pub source: &'static str,
}

/// The vertex attribute `id` is `[[attribute(index)]]`, read from vertex
/// buffer `[[buffer(buffer_index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    /// The input's `[[attribute]]`.
    pub index: usize,
    /// A format the attribute is read in, which reflecting the shader's
    /// bindings and the tests build pipelines with.
    pub data_type: AttributeDataType,
    /// The vertex buffer it reads.
    pub buffer_index: usize,
    /// The attribute's id.
    pub id: usize,
}

impl Attribute {
    /// The attribute `id` at `[[attribute(index)]]`, read as `data_type` from
    /// `[[buffer(buffer_index)]]`.
    #[must_use]
    pub const fn new(index: usize, data_type: AttributeDataType, buffer_index: usize, id: usize) -> Attribute {
        Attribute { index, data_type, buffer_index, id }
    }
}

/// Texture `id` binds to `[[texture(index)]]` and its sampler to
/// `[[sampler(index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    /// The texture's and its sampler's index.
    pub index: usize,
    /// The texture's id.
    pub id: usize,
}

impl Texture {
    /// The texture `id` at `[[texture(index)]]` and `[[sampler(index)]]`.
    #[must_use]
    pub const fn new(index: usize, id: usize) -> Texture {
        Texture { index, id }
    }
}
