//! The GLSL that `mln_glsl` parses with naga's GLSL front end and the
//! Vulkan and D3D12 backends write as SPIR-V and HLSL: MapLibre Native's
//! Vulkan GLSL, whose buffers and textures bind at `mln_glsl`'s descriptor
//! set layout.

use crate::gfx_types::AttributeDataType;

/// A program's GLSL, and the tables of its attributes and textures.
#[derive(Debug)]
pub struct Source {
    /// The shader.
    pub glsl: &'static Glsl,
    /// The inputs that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The inputs that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [Texture],
}

/// A program's GLSL.
///
/// A stage compiles from `mln_glsl`'s prelude of the stage, `prelude` and
/// the stage's source.
#[derive(Debug)]
pub struct Glsl {
    /// The shader's own prelude constant (its buffer bindings), "" if none.
    pub prelude: &'static str,
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The vertex or instance attribute `id` is the input at
/// `layout(location = index)`. Every attribute, instanced or not, is a
/// vertex input.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    /// The input's location.
    pub index: usize,
    /// The format of the input.
    pub data_type: AttributeDataType,
    /// The attribute's id.
    pub id: usize,
}

impl Attribute {
    /// The attribute `id` at `layout(location = index)`, read as
    /// `data_type`.
    #[must_use]
    pub const fn new(index: usize, data_type: AttributeDataType, id: usize) -> Attribute {
        Attribute { index, data_type, id }
    }
}

/// Texture `id` binds in the image set at `mln_glsl`'s image texture binding
/// of `index`, and its sampler at its image sampler binding of `index`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    /// The texture's and its sampler's index in the image set.
    pub index: usize,
    /// The texture's id.
    pub id: usize,
}

impl Texture {
    /// The texture `id` at the image set's bindings of `index`.
    #[must_use]
    pub const fn new(index: usize, id: usize) -> Texture {
        Texture { index, id }
    }
}
