//! The OpenGL backend: the `mln_gfx` traits implemented on the `glow`
//! bindings, which call OpenGL ES 3.0 or desktop OpenGL 3.3 natively and
//! WebGL2 in a browser, with its shaders and render targets.
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
//! On desktop OpenGL 3.3 or later, of either profile, the backend draws as
//! on OpenGL ES 3.0 but where the APIs differ (`Context::is_desktop`): the
//! GLSL compiles as `#version 330 core`, which every such context takes,
//! alpha and luminance textures are red ones with a swizzle, and writes to an
//! sRGB framebuffer are encoded, as OpenGL ES always encodes them. None of it
//! is EGL's, so a desktop context of another platform draws through it too.
//!
//! A [`RendererBackend`] renders on a [`Device`], an EGL share group or one
//! context that its backends take turns in, or on Windows a WGL share group,
//! into an `OffscreenTarget`'s framebuffer. A `SurfaceTarget` presents to a
//! Wayland, X11 or Android window through EGL from a context of the window's
//! display, or to a Win32 window through WGL from a context of its own, and a
//! `CanvasTarget` draws into the canvas of a WebGL2 context. An
//! `ExportedTarget` renders into dmabufs, or on Android into hardware
//! buffers, that it allocates.

// OpenGL through glow is the FFI here.
#![allow(unsafe_code)]

pub mod command_encoder;
pub mod context;
pub mod custom_layer;
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
#[cfg(all(unix, not(target_vendor = "apple")))]
pub mod exported_target;
#[cfg(not(target_arch = "wasm32"))]
pub mod offscreen_target;
#[cfg(all(unix, not(target_vendor = "apple")))]
pub mod surface_target;
#[cfg(windows)]
mod wgl;
#[cfg(windows)]
pub mod wgl_surface_target;

#[cfg(not(target_arch = "wasm32"))]
pub use borrowed_target::{BorrowedTarget, BorrowedTextures, HostSurface, HostSurfaceTarget};
#[cfg(target_arch = "wasm32")]
pub use canvas_target::{CanvasTarget, WebCanvas};
pub use context::Context;
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
#[cfg(not(target_arch = "wasm32"))]
pub use device::{Api, Device, DeviceOptions};
#[cfg(all(unix, not(target_vendor = "apple")))]
pub use exported_target::ExportedTarget;
#[cfg(not(target_arch = "wasm32"))]
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
#[cfg(all(unix, not(target_vendor = "apple")))]
pub use surface_target::SurfaceTarget;
#[cfg(windows)]
pub use wgl_surface_target::SurfaceTarget;
