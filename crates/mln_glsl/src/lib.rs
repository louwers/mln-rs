//! The GLSL of the Vulkan and D3D12 backends, and naga's front end for it:
//! each backend writes the module naga parses as its own shading language.
//!
//! The GLSL (`sources`) is MapLibre Native's Vulkan GLSL, written for naga's
//! GLSL front end ([`Module::parse`]). Its clip space is y-up, as naga's IR
//! is: a back end for a y-down clip space flips Y.
//!
//! The descriptor sets ([`layout`]) are the global uniform buffers, a layer's
//! buffers, a drawable's uniform buffers and a drawable's textures. The
//! renderer's buffer ids are packed: a layer's uniform buffer can have the id
//! of another layer's storage buffer, so a buffer's binding follows from its
//! id and from whether it is a storage buffer.
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]

mod front_end;
mod program;
/// MapLibre Native's Vulkan GLSL, written for naga's GLSL front end.
#[rustfmt::skip]
mod sources;
#[cfg(any(test, feature = "variants"))]
pub mod variants;

pub use front_end::{Module, ResourceUse, Stage};
pub use mln_shaders::layout;
pub use program::{AttributeInfo, PROGRAMS, ShaderSource, TextureInfo, shader_source};

#[cfg(test)]
mod tests;
