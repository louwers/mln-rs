//! Unstable: the engine internals that the layer types of `mln_layers` use,
//! which aren't yet a contract for layer types. Anything here can change or
//! go in any release.
//!
//! What it holds: a layer's values and its update (`EvaluatedLayer`,
//! `LayerValues`, `RenderLayerBase`, `Parts`, `PartsMut`,
//! `LayerUpdateParameters`, `Atlases`); the contents a type without a source
//! or a layout names (`NoBucket`, `NoLayout`, `NoTileBucket`); the tiles
//! that cover the view (`tile_cover`); and drawing: the frame's
//! `PaintParameters`, its draws and their GPU objects.
//!
//! The engine types that the hooks of [`LayerType`](crate::LayerType),
//! [`TypedLayer`](crate::TypedLayer) and [`RenderLayer`](crate::RenderLayer)
//! name are public too, for those traits to be, but nothing names them
//! outside the engine.

pub use crate::bucket::NoBucket;
pub use crate::draw::{DrawGeometry, DrawObjects, Textures, raw_vertices};
pub use crate::gfx::indexes::DrawIndexes;
pub use crate::gfx::vertex_attribute::Uniforms;
pub use crate::globe_tile_mesh::raw_globe_tile_mesh;
pub use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, LayerDrawing, TileDraws};
pub use crate::layer_type::EvaluatedLayer;
pub use crate::layout::layout::NoLayout;
pub use crate::paint_parameters::PaintParameters;
pub use crate::render_layer::{Atlases, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayerBase};
pub use crate::render_pass::RenderPass;
pub use crate::render_static_data::RenderStaticData;
pub use crate::render_tile::NoTileBucket;
pub use crate::tile_matrix::{projection_data, to_projection_ubo};
pub use crate::util::tile_cover::{TileCoverParameters, tile_cover};
