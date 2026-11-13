//! The Vulkan shaders: their GLSL and its compiler, the descriptor set
//! layout, and the shader sources, programs and groups.
//!
//! C++ compiles its GLSL with glslang when a shader is first used; the port
//! compiles it with naga's GLSL front end and SPIR-V back end (`compiler`),
//! which the vendored GLSL is written for (`glsl_sources`).
//!
//! The descriptor sets are C++'s (`layout`): the global uniform buffers, a
//! layer's buffers, a drawable's uniform buffers and a drawable's textures.
//! C++ gives every buffer of a Vulkan build an id of its own across the sets
//! (`getEnumValue(packed, unpacked)` in `shader_defines.hpp` picks the
//! unpacked one), so an id is a binding. The port's renderer uses the packed
//! ids of the WebGPU and Metal builds, where a layer's uniform buffer can
//! have the id of another layer's storage buffer, so a buffer's binding
//! follows from its id and from whether it is a storage buffer.

pub(crate) mod compiler;
/// The C++ core's GLSL, with the port's changes.
#[rustfmt::skip]
pub(crate) mod glsl_sources;
pub(crate) mod layout;
pub(crate) mod shader_group;
pub(crate) mod shader_program;
pub(crate) mod shader_source;
