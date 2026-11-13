//! The WebGPU backend: the `mln_gfx` traits implemented on the `wgpu` crate,
//! with its shaders and render targets.
//!
//! A [`RendererBackend`] renders on a [`Device`], into an [`OffscreenTarget`]
//! or a [`SurfaceTarget`] built on it.
//!
//! The C++ backend runs on Dawn or wgpu-native through the WebGPU C API; this
//! port makes the same calls through wgpu's Rust API. Raw `WGPU*` handles
//! become wgpu objects (reference counted, so "release" is a drop), and the
//! `Context&` members of resources become `Rc<Context>`.
//!
//! The WGSL comes from the C++ headers, with the port's changes
//! (`shaders::wgsl_sources`).

mod bind_groups;
pub mod borrowed_target;
pub mod buffer_resource;
pub mod command_encoder;
pub mod context;
pub mod custom_layer;
pub mod device;
pub mod draw_target;
pub mod drawable;
pub mod drawable_builder;
pub mod layer_group;
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
mod writes;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures};
pub use context::Context;
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use surface_target::{SurfaceOptions, SurfaceTarget};
