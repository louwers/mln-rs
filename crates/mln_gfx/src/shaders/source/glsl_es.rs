//! GLSL ES 3.00, which `mln_gl` compiles, as `#version 330 core` on desktop
//! OpenGL.

/// A program's OpenGL source: its GLSL, and the tables of where its uniform
/// blocks, attributes and textures bind.
#[derive(Debug)]
pub struct Source {
    pub glsl: &'static Glsl,
    pub uniform_blocks: &'static [UniformBlock],
    pub attributes: &'static [Attribute],
    /// The attributes that step once an instance.
    pub instance_attributes: &'static [Attribute],
    pub textures: &'static [Texture],
}

/// A program's GLSL.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the backend's prelude of that
/// stage and the shader's.
#[derive(Debug)]
pub struct Glsl {
    pub vertex: &'static str,
    pub fragment: &'static str,
}

/// The uniform block `name` binds to the binding point `id`, where the
/// buffer of that id is bound.
#[derive(Clone, Copy, Debug)]
pub struct UniformBlock {
    pub name: &'static str,
    pub id: usize,
    /// The size of the block's UBO.
    pub size: usize,
    /// The size of an element of the layer's buffer that holds this block for
    /// each of its drawables, or 0 if the renderer writes no such buffer.
    pub element_size: usize,
}

/// The vertex attribute `name` reads attribute `id`.
#[derive(Clone, Copy, Debug)]
pub struct Attribute {
    pub name: &'static str,
    pub id: usize,
}

/// The sampler `name` reads texture `id`.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub name: &'static str,
    pub id: usize,
}
