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
mod borrowed_target;
mod buffer_resource;
mod command_encoder;
mod context;
mod custom_layer;
mod device;
mod draw_target;
mod drawable;
mod drawable_builder;
mod layer_group;
mod offscreen_target;
mod offscreen_texture;
mod render_pass;
mod renderer_backend;
mod shaders;
mod surface_target;
mod texture2d;
mod tile_layer_group;
mod uniform_buffer;
mod upload_pass;
mod vertex_buffer_resource;
mod writes;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, depth_stencil_format};
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Overlay, OverlayTarget, SurfaceOptions, SurfaceTarget};
pub use upload_pass::UploadPass;

#[cfg(test)]
mod tests;
