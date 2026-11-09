//! Ports the backend-independent `gfx/` abstractions of the C++ core
//! (`include/mln/gfx`, `src/mln/gfx`), the backend-independent `shaders/`
//! code and the drawable-path files from `renderer/` that the backends
//! implement. `mln_webgpu` implements the traits on `wgpu`.
//!
//! Crate cut (see PLAN.md, "Crate map"): `renderer/layer_group.hpp` and
//! `renderer/render_pass.hpp` live here (`renderer::layer_group`,
//! `renderer::render_pass`), and `mln::PaintParameters` is seen through the
//! `paint_parameters::PaintParameters` trait, because `mln_webgpu` cannot
//! depend on `mln_renderer`. `LayerTweaker` stays in the renderer.
//!
//! Ownership follows the single render thread of the C++ renderer: shared
//! pointers are `Rc`, mutation through them uses `RefCell`/`Cell`.

pub mod attribute;
pub mod backend;
pub mod backend_scope;
pub mod color_mode;
pub mod command_encoder;
pub mod context;
pub mod context_observer;
pub mod cull_face_mode;
pub mod debug_group;
pub mod depth_mode;
pub mod draw_mode;
pub mod drawable;
pub mod drawable_builder;
pub mod drawable_tweaker;
pub mod gfx_types;
pub mod headless_backend;
pub mod index_vector;
pub mod offscreen_texture;
pub mod paint_parameters;
pub mod polyline_generator;
pub mod render_pass;
pub mod renderable;
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
pub mod upload_pass;
pub mod util_hash;
pub mod vertex_attribute;
pub mod vertex_buffer;
pub mod vertex_vector;

pub use backend_scope::{BackendScope, ScopeType};
pub use color_mode::ColorMode;
pub use command_encoder::CommandEncoder;
pub use context::Context;
pub use cull_face_mode::CullFaceMode;
pub use depth_mode::DepthMode;
pub use draw_mode::DrawMode;
pub use drawable::{Drawable, UniqueDrawable};
pub use drawable_builder::{DrawableBuilder, UniqueDrawableBuilder};
pub use drawable_tweaker::{DrawableTweaker, DrawableTweakerPtr};
pub use gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
pub use headless_backend::HeadlessBackend;
pub use offscreen_texture::OffscreenTexture;
pub use paint_parameters::PaintParameters;
pub use render_pass::{RenderPass, RenderPassDescriptor};
pub use renderable::{Renderable, RenderableResource, SwapBehaviour};
pub use renderer::layer_group::{LayerGroup, LayerGroupBase, LayerGroupBasePtr, TileLayerGroup};
pub use renderer_backend::{ContextMode, RendererBackend};
pub use scissor_rect::ScissorRect;
pub use shader_registry::ShaderRegistry;
pub use shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
pub use stencil_mode::StencilMode;
pub use types::DepthMaskType;
pub use uniform_buffer::{UniformBuffer, UniformBufferArray, UniformBufferPtr};
pub use upload_pass::UploadPass;
pub use vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
