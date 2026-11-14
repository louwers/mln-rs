//! The WebGPU backend, on the `wgpu` crate: [`WebGpu`], the backend's
//! [`Gpu`](mln_gfx::gpu::Gpu), with its shaders, and the targets the
//! renderer draws frames into.
//!
//! A [`RendererBackend`] renders on a [`Device`], into an
//! [`OffscreenTarget`], a [`SurfaceTarget`] or a [`BorrowedTarget`] built on
//! it.

mod borrowed_target;
mod custom_layer;
mod device;
mod gpu;
mod index_table;
mod renderer_backend;
mod shaders;
mod surface_target;
mod writes;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, depth_stencil_format};
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use gpu::WebGpu;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Overlay, OverlayTarget, SurfaceOptions, SurfaceTarget};

/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;

#[cfg(test)]
mod tests;
