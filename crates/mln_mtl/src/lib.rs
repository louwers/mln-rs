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
//! A `RendererBackend` renders on a `Device`, into an `OffscreenTarget`
//! or a `SurfaceTarget` built on it.
//!
//! Metal exists on Apple platforms only; elsewhere this crate is empty.

#![cfg(target_vendor = "apple")]
// Metal through objc2 is the FFI here.
#![allow(unsafe_code)]

pub mod borrowed_target;
pub mod buffer_resource;
pub mod command_encoder;
pub mod context;
pub mod device;
pub mod draw_target;
pub mod drawable;
pub mod drawable_builder;
pub mod layer_group;
pub mod mtl_fwd;
pub mod offscreen_target;
pub mod offscreen_texture;
pub mod render_pass;
pub mod renderer_backend;
pub mod shaders;
pub mod surface_target;
pub mod texture2d;
pub mod tile_layer_group;
pub mod uniform_buffer;
pub mod upload_pass;
pub mod vertex_buffer_resource;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, PendingReads};
pub use context::Context;
pub use device::Device;
pub use objc2;
pub use objc2_metal;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Layer, SurfaceTarget};

// MTLCreateSystemDefaultDevice returns nil in a process that doesn't link
// CoreGraphics, such as a command-line tool.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}
