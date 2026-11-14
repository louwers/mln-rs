//! The WebGPU backend, on the `wgpu` crate: the `mln_gfx` traits the
//! renderer draws through, with their shaders and render targets, and
//! [`WebGpu`], the backend's [`Gpu`](mln_gfx::gpu::Gpu).
//!
//! A [`RendererBackend`] renders on a [`Device`], into an [`OffscreenTarget`]
//! or a [`SurfaceTarget`] built on it.
//!
//! The WGSL is in `shaders::wgsl_sources`.

mod bind_groups;
mod borrowed_target;
mod command_encoder;
mod context;
mod custom_layer;
mod device;
mod draw_target;
mod drawable;
mod drawable_builder;
mod gpu;
mod layer_group;
mod offscreen_target;
mod offscreen_texture;
mod render_pass;
mod renderer_backend;
mod shaders;
mod surface_target;
mod tile_layer_group;
mod writes;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, depth_stencil_format};
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use gpu::WebGpu;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Overlay, OverlayTarget, SurfaceOptions, SurfaceTarget};

#[cfg(test)]
mod tests;
