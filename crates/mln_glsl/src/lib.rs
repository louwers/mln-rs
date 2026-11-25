//! The GLSL of the Vulkan and D3D12 backends, and naga's front end for it:
//! each backend writes the module naga parses as its own shading language.
//!
//! The GLSL (`sources`) is MapLibre Native's Vulkan GLSL, written for naga's
//! GLSL front end ([`Module::parse`]). Its clip space is y-up, as naga's IR
//! is: a back end for a y-down clip space flips Y. A program's declarations,
//! what each stage reads of its buffers, textures and attributes, come from
//! the program ([`ShaderSource::compose`]).
//!
//! The descriptor sets ([`layout`]) are the global uniform buffers, a layer's
//! buffers, a drawable's uniform buffers with the vertex buffers its shader
//! reads as storage buffers, and a drawable's textures. The renderer's buffer
//! ids are packed: a layer's uniform buffer can have the id of another layer's
//! storage buffer, so a buffer's binding follows from its id and from whether
//! it is a storage buffer.
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg_attr(
    any(target_vendor = "apple", target_family = "wasm"),
    expect(rustdoc::broken_intra_doc_links, reason = "the crate docs link to items that these targets compile out")
)]
#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]

mod declarations;
mod front_end;
pub mod layout;
mod program;
/// MapLibre Native's Vulkan GLSL, written for naga's GLSL front end.
#[rustfmt::skip]
mod sources;
#[cfg(any(test, feature = "variants"))]
pub mod variants;

pub use front_end::{Module, ResourceUse, Stage};
pub use program::{BUILT_INS, ShaderSource};

#[cfg(test)]
mod tests;
