//! MSL, which `mln_mtl` compiles.

use crate::gfx_types::AttributeDataType;

/// A program's MSL, and its per-vertex inputs, per-instance inputs and
/// textures.
#[derive(Debug)]
pub struct Source {
    pub msl: &'static Msl,
    pub attributes: &'static [Attribute],
    pub instance_attributes: &'static [Attribute],
    pub textures: &'static [Texture],
}

/// The MSL of a shader. Its library compiles from the backend's prelude,
/// `prelude` and `source`.
#[derive(Debug)]
pub struct Msl {
    pub vertex_main_function: &'static str,
    pub fragment_main_function: &'static str,
    /// The shader's own prelude constant (its UBO ids and structs), "" if none.
    pub prelude: &'static str,
    pub source: &'static str,
}

/// The vertex attribute `id` is `[[attribute(index)]]`, read from vertex
/// buffer `[[buffer(buffer_index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    pub index: usize,
    /// A format the attribute is read in, which reflecting the shader's
    /// bindings and the tests build pipelines with.
    pub data_type: AttributeDataType,
    pub buffer_index: usize,
    pub id: usize,
}

/// Texture `id` binds to `[[texture(index)]]` and its sampler to
/// `[[sampler(index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub index: usize,
    pub id: usize,
}
