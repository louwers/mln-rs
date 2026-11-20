//! GLSL ES 3.00, which `mln_gl` compiles, as `#version 330 core` on desktop
//! OpenGL.
//!
//! Every uniform block a program's GLSL declares, the prelude's too, has an
//! entry in its table, and a draw binds a buffer to each: WebGL2 rejects a
//! draw whose program has a block without a buffer, whether or not the
//! shader reads it.

/// A program's OpenGL source: its GLSL, and the tables of where its uniform
/// blocks, attributes and textures bind.
#[derive(Debug)]
pub struct Source {
    /// The shader.
    pub glsl: &'static Glsl,
    /// Its uniform blocks.
    pub uniform_blocks: &'static [UniformBlock],
    /// The attributes that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The attributes that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [Texture],
}

/// A program's GLSL.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the backend's prelude of that
/// stage and the shader's.
#[derive(Debug)]
pub struct Glsl {
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The uniform block `name` binds to the binding point `id`, where the
/// buffer of that id is bound.
#[derive(Clone, Copy, Debug)]
pub struct UniformBlock {
    /// The block's name in the GLSL.
    pub name: &'static str,
    /// The id of the buffer it reads.
    pub id: usize,
    /// The size of the block's UBO, which `mln_gl`'s compile test checks
    /// against the block the GLSL declares.
    pub size: usize,
    /// The size of an element of the layer's buffer that holds this block for
    /// each of its drawables, or 0 if the renderer writes no such buffer.
    pub element_size: usize,
}

impl UniformBlock {
    /// The block `name` at `id`, whose UBO is a `T`.
    #[must_use]
    pub const fn new<T>(name: &'static str, id: usize) -> UniformBlock {
        UniformBlock { name, id, size: size_of::<T>(), element_size: 0 }
    }

    /// The block `name` at `id`, whose UBO is a `T`, one in each `Element` of
    /// a layer's buffer.
    #[must_use]
    pub const fn element<T, Element>(name: &'static str, id: usize) -> UniformBlock {
        assert!(size_of::<T>() <= size_of::<Element>());
        UniformBlock { name, id, size: size_of::<T>(), element_size: size_of::<Element>() }
    }
}

/// The vertex attribute `name` reads attribute `id`.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    /// The attribute's name in the GLSL.
    pub name: &'static str,
    /// The attribute's id.
    pub id: usize,
}

impl Attribute {
    /// The vertex attribute `name`, which reads attribute `id`.
    #[must_use]
    pub const fn new(name: &'static str, id: usize) -> Attribute {
        Attribute { name, id }
    }
}

/// The sampler `name` reads texture `id`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    /// The sampler's name in the GLSL.
    pub name: &'static str,
    /// The texture's id.
    pub id: usize,
}

impl Texture {
    /// The sampler `name`, which reads texture `id`.
    #[must_use]
    pub const fn new(name: &'static str, id: usize) -> Texture {
        Texture { name, id }
    }
}
