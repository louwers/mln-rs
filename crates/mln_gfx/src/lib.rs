//! The backend-independent graphics layer: the [`gpu::Gpu`] interface a
//! backend implements, and the renderer data drawn through it. `mln_webgpu`
//! implements it on `wgpu`, `mln_mtl` on Metal, `mln_vulkan` on Vulkan and
//! `mln_gl` on OpenGL ES 3.0.
//!
//! A backend's `RendererBackend` hands the renderer its `Gpu`, which owns
//! the renderer's buffers, textures and the groups of buffers its draws
//! bind; the renderer keeps them in `mln_renderer`. Each program's
//! attributes are in `shaders::program_info` and its variants come from
//! `shader_group`.
//!
//! The renderer runs on one thread: shared pointers are `Rc`, mutation
//! through them uses `RefCell`/`Cell`.

pub mod attribute;
pub mod backend;
pub mod buffer;
pub mod color_mode;
pub mod cull_face_mode;
pub mod depth_mode;
pub mod draw_mode;
pub mod export;
pub mod frame;
pub mod gfx_types;
pub mod gpu;
pub mod index_vector;
pub mod offscreen_target;
pub mod polyline_generator;
pub mod projection_variant;
pub mod released;
pub mod renderer_backend;
pub mod rendering_stats;
pub mod scissor_rect;
pub mod shader_group;
pub mod shader_registry;
pub mod shaders;
pub mod stencil_mode;
pub mod texture2d;
pub mod types;
pub mod util_hash;
pub mod vertex_attribute;
pub mod vertex_vector;
pub mod window;

pub use buffer::{Buffer, BufferPtr};
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
pub use renderer_backend::{Hold, RendererBackend};
pub use scissor_rect::ScissorRect;
pub use shader_registry::ShaderRegistry;
pub use shaders::shader_program::{ShaderProgram, ShaderProgramPtr};
pub use stencil_mode::StencilMode;
pub use types::DepthMaskType;
pub use vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
pub use window::Window;
