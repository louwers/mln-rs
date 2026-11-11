//! Ports the C++ Metal backend (`src/mln/mtl`, `include/mln/mtl`,
//! `include/mln/shaders/mtl`, `src/mln/shaders/mtl`, and the headless
//! backend from platform/default) onto the `objc2-metal` bindings.
//!
//! The C++ backend calls Metal through metal-cpp; this port makes the same
//! calls through objc2. `NS::SharedPtr`s become `Retained` objects, and the
//! `Context&` members of resources become `Rc<Context>`.
//!
//! The MSL comes from the C++ headers, with the port's changes
//! (`shaders::msl_sources`).
//!
//! Metal exists on Apple platforms only; elsewhere this crate is empty.

#![cfg(target_vendor = "apple")]
// Metal through objc2 is the FFI here.
#![allow(unsafe_code)]

pub mod buffer_resource;
pub mod command_encoder;
pub mod context;
pub mod drawable;
pub mod drawable_builder;
pub mod headless_backend;
pub mod layer_group;
pub mod mtl_fwd;
pub mod offscreen_texture;
pub mod render_pass;
pub mod renderable_resource;
pub mod renderer_backend;
pub mod shaders;
pub mod surface_backend;
pub mod texture2d;
pub mod tile_layer_group;
pub mod uniform_buffer;
pub mod upload_pass;
pub mod vertex_buffer_resource;

pub use context::Context;
pub use headless_backend::HeadlessBackend;
pub use renderer_backend::RendererBackend;
pub use surface_backend::SurfaceBackend;

// MTLCreateSystemDefaultDevice returns nil in a process that doesn't link
// CoreGraphics, such as a command-line tool.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}
