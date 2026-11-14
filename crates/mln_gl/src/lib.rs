//! The OpenGL backend: [`GlGpu`], the backend's `mln_gfx::gpu::Gpu`, on the
//! `glow` bindings, which call OpenGL ES 3.0 or desktop OpenGL 3.3 natively
//! and WebGL2 in a browser, with its shaders and the targets the renderer
//! draws frames into.
//!
//! The renderer drives every backend the way C++ drives Metal and WebGPU, so
//! this backend differs from the C++ one where that one has code of its own
//! in the renderer:
//!
//! - A layer's per-drawable UBOs arrive as one buffer of every drawable's
//!   (C++: `MLN_UBO_CONSOLIDATION`, which its OpenGL build leaves off).
//!   OpenGL ES 3.0 has no storage buffers to index in the shader, so a
//!   draw binds its element of that buffer as the uniform block.
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
//! into an `OffscreenTarget`'s textures. A `SurfaceTarget` presents to a
//! Wayland, X11 or Android window through EGL from a context of the window's
//! display, or to a Win32 window through WGL from a context of its own, and a
//! `CanvasTarget` draws into the canvas of a WebGL2 context. An
//! `ExportedTarget` renders into dmabufs, or on Android into hardware
//! buffers, that it allocates.
//!
//! A backend's context hosts a `GlGpu`, which owns the renderer's
//! resources and draws its frames; a `GlGpu` can also run on a context of
//! its own in a device's share group.

#![expect(unsafe_code, reason = "OpenGL through glow is the FFI here")]

mod context;
mod custom_layer;
mod enums;
mod frame_image;
mod gpu;
mod renderer_backend;
mod shaders;
mod state;
mod types;

#[cfg(not(target_arch = "wasm32"))]
mod borrowed_target;
#[cfg(target_arch = "wasm32")]
pub mod canvas_target;
#[cfg(not(target_arch = "wasm32"))]
mod device;
#[cfg(not(target_arch = "wasm32"))]
mod egl;
#[cfg(all(unix, not(target_vendor = "apple")))]
pub mod exported_target;
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
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
#[cfg(not(target_arch = "wasm32"))]
pub use device::{Api, Device, DeviceOptions};
#[cfg(all(unix, not(target_vendor = "apple")))]
pub use exported_target::ExportedTarget;
pub use gpu::{FramebufferTextures, GlGpu};
/// Frames drawn into textures of the library's and read back.
#[cfg(not(target_arch = "wasm32"))]
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;
pub use renderer_backend::RendererBackend;
#[cfg(all(unix, not(target_vendor = "apple")))]
pub use surface_target::SurfaceTarget;
#[cfg(windows)]
pub use wgl_surface_target::SurfaceTarget;

#[cfg(test)]
mod tests;
