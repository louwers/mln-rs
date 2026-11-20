//! WGSL, which `mln_webgpu` compiles.
//!
//! A buffer binds at the `@binding` of its id. A texture's sampler binds at
//! the `@binding` of its [`Texture::index`] and the texture at the one
//! after.

/// A program's WGSL, and its per-vertex inputs, per-instance inputs and
/// textures.
#[derive(Debug)]
pub struct Source {
    /// The shader.
    pub wgsl: &'static Wgsl,
    /// The inputs that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The inputs that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [Texture],
}

/// The WGSL of a shader: its stages, and the constants and structs of its
/// own that both stages read.
///
/// A stage compiles from the backend's prelude, `prelude` and the stage's
/// source.
#[derive(Debug)]
pub struct Wgsl {
    /// The vertex stage, or `None` for an empty one.
    pub vertex: Option<&'static str>,
    /// The fragment stage, or `None` for an empty one.
    pub fragment: Option<&'static str>,
    /// What both stages read, or `None` for nothing.
    pub prelude: Option<&'static str>,
}

/// A vertex input of a shader: its shader location, and the attribute id the
/// renderer names it by.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    /// The input's `@location`.
    pub index: usize,
    /// The attribute's id.
    pub id: usize,
}

impl Attribute {
    /// The attribute `id` at `@location(index)`.
    #[must_use]
    pub const fn new(index: usize, id: usize) -> Attribute {
        Attribute { index, id }
    }
}

/// A texture of a shader: the binding of its sampler, the texture id the
/// renderer names it by, and whether its format can be filtered, which an
/// `Rgba32Float` one can't.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    /// The `@binding` of its sampler, one before the texture's.
    pub index: usize,
    /// The texture's id.
    pub id: usize,
    /// Whether it binds with a filtering sampler.
    pub filterable: bool,
}

impl Texture {
    /// The filterable texture `id` whose sampler binds at `index`.
    #[must_use]
    pub const fn new(index: usize, id: usize) -> Texture {
        Texture { index, id, filterable: true }
    }

    /// The unfilterable texture `id` whose sampler binds at `index`.
    #[must_use]
    pub const fn unfilterable(index: usize, id: usize) -> Texture {
        Texture { index, id, filterable: false }
    }
}
