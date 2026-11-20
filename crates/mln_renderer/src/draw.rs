//! The draw services: what a layer type draws with, the other half of the
//! [layer type contract](crate::layer).
//!
//! A layer keeps the GPU objects of its draws between frames
//! ([`DrawObjects`]), and makes them again only when what they are made of
//! changes. It makes them in the renderer's [`Context`], whose stores keep
//! what draws share: vertices, indexes and textures ([`SharedVertices`],
//! [`SharedIndexes`], [`SharedTexture`]). A bucket's geometry, which the
//! engine moves into those stores when it uploads the bucket
//! ([`BucketGeometry`]), is read with [`bucket_vertices`],
//! [`bucket_indexes`] and [`paint_vertices`]. A draw names its program's
//! shader by the key the context gives ([`Context::shader_key`]); the layer
//! type adds its programs' sources to the registry
//! ([`LayerType::add_shader_sources`](crate::layer::LayerType::add_shader_sources)).
//!
//! Each frame, [`LayerType::make_draws`](crate::layer::LayerType::make_draws)
//! makes the layer's draws of the frame from its
//! [`DrawContext`](crate::layer::DrawContext): [`FrameDraw`]s with their
//! uniform values, in a [`DrawList`], which [`DrawList::finish`] orders and
//! whose values a [`UniformWriter`] writes to the buffers of the layer's
//! [`LayerDraws`]. The [`DrawFrame`] holds what the draws' uniforms read of
//! the frame, and [`projection_data`] projects a tile's geometry in it.
//!
//! The layer's [`LayerDrawing`] keeps its draws, those of its tiles in a
//! [`TileMap`], and gives the engine its draw groups ([`DrawGroup`]). The
//! engine places each group in the layers' order and depth, and uploads and
//! draws it through [`LayerDrawing::upload`] and [`LayerDrawing::render`],
//! which hand the draws [`PaintParameters`], the frame's encode.
//! [`TileDraws`] is the drawing of a layer that draws one group of its
//! tiles' draws, and [`SingleDraw`] is one draw with its buffers. Draws that
//! draw onto the frame as one surface say how with a [`Composite`]; a layer
//! that draws into textures of its own before the frame's passes does in
//! its [`OffscreenPass`](crate::layer::OffscreenPass), with a
//! [`RenderTarget`].

pub use crate::bucket_geometry::{
    BucketGeometry, bucket_indexes, bucket_vertices, copied_vertices, derived_vertices, paint_vertices,
};
pub use crate::draw_objects::{DrawGeometry, DrawObjects, Textures, copy_shared_attributes, raw_vertices};
pub use crate::frame_projection::zero_to_one_clip_z;
pub use crate::gfx::atlas_binding::AtlasBinding;
pub use crate::gfx::context::Context;
pub use crate::gfx::indexes::{DrawIndexes, Indexes};
pub use crate::gfx::store::{SharedIndexes, SharedTexture};
pub use crate::gfx::texture2d::{Texture2D, TextureMut};
pub use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
pub use crate::gfx::vertices::{SharedVertices, Vertices};
pub use crate::layer_composite::Composite;
pub use crate::layer_drawing::{
    BucketChanges, DrawDepth, DrawGroup, DrawList, DrawObjectsMut, DrawOrder, DrawPriority, FrameDraw, FrameDraws,
    KeptDraw, LayerDrawing, LayerDraws, SingleDraw, TileDraws, TileEntry, TileMap, TileObjects, UniformWriter,
    finish_tiles, tile_objects,
};
pub use crate::paint_parameters::{DrawFrame, PaintParameters};
pub use crate::render_light::EvaluatedLight;
pub use crate::render_static_data::RenderStaticData;
pub use crate::render_target::{RenderTarget, TargetPasses};
pub use crate::tile_matrix::{
    cast_matrix, globe_extrude_scale, projection_data, tile_unit_translation, to_projection_ubo,
};
pub use crate::tile_render_data::TileAtlasTextures;
