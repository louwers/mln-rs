//! WGSL, which `mln_webgpu` compiles.

/// A program's WGSL, and its per-vertex inputs, per-instance inputs and
/// textures.
#[derive(Debug)]
pub struct Source {
    pub wgsl: &'static Wgsl,
    pub attributes: &'static [Attribute],
    pub instance_attributes: &'static [Attribute],
    pub textures: &'static [Texture],
}

/// The WGSL of a shader: its stages, and the constants and structs of its
/// own that both stages read.
#[derive(Debug)]
pub struct Wgsl {
    pub vertex: Option<&'static str>,
    pub fragment: Option<&'static str>,
    pub prelude: Option<&'static str>,
}

/// A vertex input of a shader: its shader location, and the attribute id the
/// renderer names it by.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    pub index: usize,
    pub id: usize,
}

/// A texture of a shader: the binding of its sampler, the texture id the
/// renderer names it by, and whether its format can be filtered, which an
/// `Rgba32Float` one can't.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub index: usize,
    pub id: usize,
    pub filterable: bool,
}
