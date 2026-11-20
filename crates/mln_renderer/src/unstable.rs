//! Unstable: the engine internals that the layer types of `mln_layers` use,
//! which aren't yet part of the [layer type contract](crate::layer). Anything
//! here can change or go in any release.
//!
//! The engine types that the contract's hooks name are public too, for the
//! contract to be, but outside the engine they're unnameable or
//! `#[doc(hidden)]`.

pub use crate::bucket_geometry::{
    BucketGeometry, bucket_indexes, bucket_vertices, copied_vertices, derived_vertices, paint_vertices,
};
pub use crate::draw::{DrawGeometry, DrawObjects, Textures, copy_shared_attributes, raw_vertices};
pub use crate::frame_projection::zero_to_one_clip_z;
pub use crate::gfx::atlas_binding::AtlasBinding;
pub use crate::gfx::context::Context;
pub use crate::gfx::indexes::{DrawIndexes, Indexes};
pub use crate::gfx::line_mesh::{LineLayoutVertex, LineOptions, LineScratch, add_line};
pub use crate::gfx::mesh::{Indexed, add_point_quads};
pub use crate::gfx::store::{SharedIndexes, SharedTexture};
pub use crate::gfx::texture2d::Texture2D;
pub use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
pub use crate::gfx::vertices::{SharedVertices, Vertices};
pub use crate::globe_tile_mesh::{GlobeTileMesh, GlobeTileMeshCache, raw_globe_tile_mesh};
pub use crate::layer_composite::Composite;
pub use crate::layer_drawing::{
    BucketChanges, DrawDepth, DrawGroup, DrawList, DrawObjectsMut, DrawOrder, DrawPriority, FrameDraw, FrameDraws,
    KeptDraw, LayerDrawing, LayerDraws, SingleDraw, TileDraws, TileMap, TileObjects, UniformWriter, finish_tiles,
    tile_objects,
};
pub use crate::line_atlas::{DashAtlas, DashEntry, DashPatternTexture, LineAtlas, LinePatternCap, LinePatternPos};
pub use crate::paint_parameters::PaintParameters;
pub use crate::paint_property_binder::{
    BinderValue, InterleavedVertexBuffer, PaintPropertyBinderBase, PatternDependency, bind_paint_property,
};
pub use crate::paint_property_statistics::StatisticValue;
pub use crate::render_layer::Atlases;
pub use crate::render_light::EvaluatedLight;
pub use crate::render_static_data::RenderStaticData;
pub use crate::render_target::{RenderTarget, TargetPasses};
pub use crate::render_tile::RenderTile;
pub use crate::tile_matrix::{cast_matrix, globe_extrude_scale, projection_data, to_projection_ubo};
pub use crate::tile_render_data::TileAtlasTextures;
pub use crate::util::subdivision::{
    SubdividedPolygon, TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon, subdivide_vertex_line,
};
pub use crate::util::tessellator::Tessellator;
pub use crate::util::tile_cover::{TileCoverParameters, tile_cover};
