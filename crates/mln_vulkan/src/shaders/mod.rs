//! The Vulkan shaders: their GLSL and its compiler, the descriptor set
//! layout, and the shader sources.
//!
//! The GLSL (`glsl_sources`) is written for naga's GLSL front end and SPIR-V
//! back end (`compiler`), which compile a program when it is first used.
//!
//! The descriptor sets (`layout`) are the global uniform buffers, a layer's
//! buffers, a drawable's uniform buffers and a drawable's textures. The
//! renderer's buffer ids are packed: a layer's uniform buffer can have the id
//! of another layer's storage buffer, so a buffer's binding follows from its
//! id and from whether it is a storage buffer.

pub(crate) mod compiler;
/// MapLibre Native's Vulkan GLSL, written for naga's GLSL front end.
#[rustfmt::skip]
pub(crate) mod glsl_sources;
pub(crate) mod layout;
pub(crate) mod shader_source;
