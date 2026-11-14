//! The backend-independent graphics layer: the traits and types the renderer
//! draws through, the backend-independent shader code (uniform buffer
//! layouts, defines, shader groups), and the drawables and layer groups that
//! the backends implement. `mln_webgpu` implements the traits on `wgpu`,
//! `mln_mtl` on Metal, `mln_vulkan` on Vulkan and `mln_gl` on OpenGL ES 3.0.
//!
//! The layer groups (`renderer::layer_group`) and the render pass bitmask
//! (`renderer::render_pass`) live here, and the renderer's paint parameters
//! are seen through the `paint_parameters::PaintParameters` trait, because
//! the backends cannot depend on `mln_renderer`. `LayerTweaker` stays in the
//! renderer.
//!
//! The renderer runs on one thread: shared pointers are `Rc`, mutation
//! through them uses `RefCell`/`Cell`.
//!
//! The `gpu` module is the id-based interface the backends move onto: a
//! backend owns its resources, and the renderer records passes of plain
//! commands. The renderer's buffers and textures are resources of the
//! backend's `Gpu` already (`buffer`, `texture2d`).

pub mod attribute;
pub mod backend;
pub mod backend_scope;
pub mod buffer;
pub mod color_mode;
pub mod command_encoder;
pub mod context;
pub mod cull_face_mode;
pub mod debug_group;
pub mod depth_mode;
pub mod draw_mode;
pub mod drawable;
pub mod drawable_builder;
pub mod drawable_tweaker;
pub mod export;
pub mod frame;
pub mod gfx_types;
pub mod gpu;
pub mod index_vector;
pub mod offscreen_texture;
pub mod paint_parameters;
pub mod polyline_generator;
pub mod projection_variant;
pub mod released;
pub mod render_pass;
pub mod renderer;
pub mod renderer_backend;
pub mod rendering_stats;
pub mod scissor_rect;
pub mod shader;
pub mod shader_group;
pub mod shader_registry;
pub mod shaders;
pub mod stencil_mode;
pub mod texture2d;
pub mod types;
pub mod uniform_buffer;
pub mod upload;
pub mod util_hash;
pub mod vertex_attribute;
pub mod vertex_vector;
pub mod window;

pub use backend_scope::{BackendScope, ScopeType};
pub use buffer::{Buffer, BufferPtr};
pub use color_mode::ColorMode;
pub use command_encoder::CommandEncoder;
pub use context::Context;
pub use cull_face_mode::CullFaceMode;
pub use depth_mode::DepthMode;
pub use draw_mode::DrawMode;
pub use drawable::{Drawable, UniqueDrawable};
pub use drawable_builder::{DrawableBuilder, UniqueDrawableBuilder};
pub use drawable_tweaker::{DrawableTweaker, DrawableTweakerPtr};
pub use export::{
    DepthStencilFormat, DepthStencilFormats, DmaBufFormat, DmaBufPlane, ExportKind, ExportKinds, ExportedFrame,
    ExportedImage, ExportedRing, FrameDepth, NativeTexture,
};
pub use frame::{ConsumerSync, DrawTarget, FrameTarget, GpuSync, Handoff, SyncKind, SyncKinds};
pub use gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
pub use offscreen_texture::OffscreenTexture;
pub use paint_parameters::PaintParameters;
pub use render_pass::{RenderPass, RenderPassDescriptor};
pub use renderer::layer_group::{LayerGroup, LayerGroupBase, LayerGroupBasePtr, TileLayerGroup};
pub use renderer_backend::RendererBackend;
pub use scissor_rect::ScissorRect;
pub use shader_registry::ShaderRegistry;
pub use shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
pub use stencil_mode::StencilMode;
pub use types::DepthMaskType;
pub use uniform_buffer::{UniformBufferArray, UniformBufferPtr};
pub use vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
pub use window::Window;
