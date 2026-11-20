//! The GLSL that `mln_glsl` parses with naga's GLSL front end and the
//! Vulkan and D3D12 backends write as SPIR-V and HLSL: MapLibre Native's
//! Vulkan GLSL, whose buffers and textures bind at `mln_glsl`'s descriptor
//! set layout.

use crate::gfx_types::AttributeDataType;

/// A program's GLSL, and the tables of its attributes and textures.
#[derive(Debug)]
pub struct Source {
    pub glsl: &'static Glsl,
    pub attributes: &'static [Attribute],
    pub instance_attributes: &'static [Attribute],
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
    pub vertex: &'static str,
    pub fragment: &'static str,
}

/// The vertex or instance attribute `id` is the input at
/// `layout(location = index)`. Every attribute, instanced or not, is a
/// vertex input.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    pub index: usize,
    /// The format of the input.
    pub data_type: AttributeDataType,
    pub id: usize,
}

/// Texture `id` binds in the image set at `mln_glsl`'s image texture binding
/// of `index`, and its sampler at its image sampler binding of `index`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub index: usize,
    pub id: usize,
}
