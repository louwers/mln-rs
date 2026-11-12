//! Ports the C++ OpenGL backend (`src/mln/gl`, `include/mln/gl`,
//! `include/mln/shaders/gl`, `src/mln/shaders/gl`, and the headless backend
//! from platform/default) onto the `glow` bindings, which call OpenGL ES 3.0
//! natively and WebGL2 in a browser.
//!
//! The renderer drives every backend the way C++ drives Metal and WebGPU, so
//! this backend differs from the C++ one where that one has code of its own
//! in the renderer:
//!
//! - A layer's per-drawable UBOs arrive as one buffer of every drawable's
//!   (C++: `MLN_UBO_CONSOLIDATION`, which its OpenGL build leaves off).
//!   OpenGL ES 3.0 has no storage buffers to index in the shader, so a
//!   drawable binds its element of that buffer as the uniform block
//!   (`uniform_buffer::UniformBuffer`).
//! - The tile clipping masks are drawn by the context
//!   (`Context::render_tile_clipping_masks`), as on Metal.
//! - Scissor rectangles arrive with a top-left origin and are flipped here.
//!
//! The GLSL comes from the C++ headers, with the port's changes
//! (`shaders::glsl_sources`).
//!
//! A [`RendererBackend`] renders on a [`Device`], an EGL share group, into
//! an `OffscreenTarget`'s framebuffer. A `SurfaceTarget` presents to a
//! Wayland or X11 window through EGL from a context of the window's display,
//! and a `CanvasTarget` draws into the canvas of a WebGL2 context.

// OpenGL through glow is the FFI here.
#![allow(unsafe_code)]

pub mod command_encoder;
pub mod context;
pub mod draw_target;
pub mod drawable;
pub mod drawable_builder;
pub mod enums;
pub mod layer_group;
pub mod offscreen_texture;
pub mod render_pass;
pub mod renderer_backend;
pub mod shaders;
pub mod state;
pub mod texture2d;
pub mod types;
pub mod uniform_buffer;
pub mod upload_pass;
pub mod vertex_array;
pub mod vertex_buffer_resource;

#[cfg(not(target_arch = "wasm32"))]
pub mod borrowed_target;
#[cfg(target_arch = "wasm32")]
pub mod canvas_target;
#[cfg(not(target_arch = "wasm32"))]
pub mod device;
#[cfg(not(target_arch = "wasm32"))]
mod egl;
#[cfg(not(target_arch = "wasm32"))]
pub mod offscreen_target;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
pub mod surface_target;

#[cfg(not(target_arch = "wasm32"))]
pub use borrowed_target::{BorrowedTarget, BorrowedTextures, HostSurface, HostSurfaceTarget};
#[cfg(target_arch = "wasm32")]
pub use canvas_target::CanvasTarget;
pub use context::Context;
#[cfg(not(target_arch = "wasm32"))]
pub use device::Device;
#[cfg(not(target_arch = "wasm32"))]
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
pub use surface_target::SurfaceTarget;
