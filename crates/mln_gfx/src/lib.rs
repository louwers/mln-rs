//! The backend-independent graphics layer: the [`gpu::Gpu`] interface a
//! backend implements, and the renderer data drawn through it. `mln_webgpu`
//! implements it on `wgpu`, `mln_mtl` on Metal, `mln_vulkan` on Vulkan and
//! `mln_gl` on OpenGL ES 3.0.
//!
//! A backend's `RendererBackend` owns its `Gpu` and lends it, to the
//! renderer and to the `FrameTarget` that the renderer draws into; the `Gpu`
//! owns the renderer's buffers, textures and the groups of buffers its
//! draws bind, which the renderer keeps in `mln_render`. The targets that
//! draw into rings of the host's textures, or of images they export, are
//! generic over each backend's part, in [`ring`]. What each program reads is
//! in `shaders::program`, and the engine's programs in `shaders::programs`.

pub mod backend;
pub mod color_mode;
pub mod cull_face_mode;
pub mod depth_mode;
pub mod draw_mode;
pub mod export;
pub mod frame;
pub mod gfx_types;
pub mod gpu;
pub mod offscreen_target;
pub mod projection_variant;
pub mod renderer_backend;
pub mod rendering_stats;
pub mod ring;
pub mod scissor_rect;
pub mod shaders;
pub mod stencil_mode;
pub mod texture2d;
pub mod types;
pub mod util_hash;
pub mod window;

pub use color_mode::ColorMode;
pub use cull_face_mode::CullFaceMode;
pub use depth_mode::DepthMode;
pub use draw_mode::DrawMode;
pub use export::{
    DepthStencilFormat, DepthStencilFormats, DmaBufFormat, DmaBufPlane, ExportKind, ExportKinds, ExportedFrame,
    ExportedImage, ExportedRing, FrameDepth, NativeTexture,
};
pub use frame::{ConsumerSync, FrameImage, FrameTarget, GpuSync, Handoff, SyncKind, SyncKinds};
pub use gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
pub use renderer_backend::RendererBackend;
pub use scissor_rect::ScissorRect;
pub use stencil_mode::StencilMode;
pub use types::DepthMaskType;
pub use window::Window;
