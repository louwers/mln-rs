//! The layer contract: what a crate implements to add a layer type that the
//! renderer draws.
//!
//! A layer type has two halves, which
//! [`RegistryBuilder::add_layer_type::<L>()`](crate::RegistryBuilder::add_layer_type)
//! registers together under the style half's name:
//!
//! - **The style half**, a marker type that implements `mln_style`'s
//!   [`StyleLayerType`](mln_style::layer::StyleLayerType): the type's name,
//!   its kind of source and its properties.
//! - **The render half**, a struct that implements [`Layer`], one value per
//!   style layer of the type, which names its style half
//!   ([`Layer::Style`]).
//!
//! # What a layer type writes
//!
//! 1. What it draws from, its [`Layer::Input`]: a geometry source's tiles,
//!    whose features a [`Layout`] lays out into a [`Bucket`] per tile on a
//!    tile worker ([`Features`]); a raster or image source's tiles, each
//!    tile's image ([`RasterTiles`], [`RasterTile`]); a raster-dem source's
//!    tiles, each tile's elevations ([`DemTiles`], [`DemContents`]); or
//!    nothing, for a layer without a source ([`NoTiles`]). The registration
//!    doesn't compile for an input of another kind of source than the style
//!    half names.
//! 2. Its tile values. [`Layer::tile`] makes what the layer keeps of a frame
//!    tile that has data for it, typically the meshes it draws the tile
//!    with. The engine keeps the value with the tile until the tile shows
//!    the layer something else, leaves the frame, or the layer's draws drop
//!    for a restyle or a switch between Mercator and the globe; it then
//!    hands the next [`tile`](Layer::tile) of the tile the previous value,
//!    so that a layer carries over what it made of contents that are still
//!    the tile's, such as a texture of its image.
//! 3. Its draws. [`Layer::prepare`] pushes the frame's draws of the layer's
//!    meshes into [`Draws`](crate::draw::Draws), with the values of their
//!    programs' blocks: the engine writes, orders, uploads and draws them in
//!    the frame's passes, at the layer's place in the layers' order and
//!    depth, so a layer never sees a pass, a stencil or a uniform buffer. A
//!    layer may draw into targets of its own first
//!    ([`Draws::target`](crate::draw::Draws::target)), or ask for the
//!    backend's own commands ([`Draws::lend`](crate::draw::Draws::lend),
//!    [`Layer::lend`]), and free what it makes with them once the GPU is
//!    done with it ([`AfterGpu`](crate::draw::AfterGpu)).
//!
//! [`Layer::evaluate`] says whether and how the layer draws with its freshly
//! evaluated values ([`Evaluation`]); [`Layer::update`] brings up to date
//! what the layer keeps for all its tiles in the frame's update.
//!
//! # Terrain
//!
//! A type says how its layers meet terrain ([`Layer::TERRAIN`],
//! [`OnTerrain`]). One that drapes draws flat into the textures of the
//! terrain's tiles rather than onto the frame: the engine prepares the layer
//! again for each drape it draws, with the drape's frame
//! ([`LayerFrame`](crate::draw::LayerFrame)) and the tiles that overlap the
//! drape, which draw the same tile values as the map's frame, so a tile's
//! value holds nothing of the frame or the world copy it is first drawn in.
//! The engine draws a drape again only when what it shows changed. One that
//! stands on terrain draws the terrain variant of its programs, whose points
//! read the ground's height, and one that hides behind the ground reads its
//! depth ([`draw::ground`](crate::draw::ground)).
//!
//! # Queries
//!
//! A rendered-feature query finds a tile's features near its geometry in the
//! tile's feature index, padded by the layer's
//! [`query_radius`](Layer::query_radius), and keeps those the layer
//! [`hit`](Layer::hit)s.
//!
//! # Buckets
//!
//! A bucket keeps its vertices and indexes in [`GeometryVec`]s and its
//! data-driven paint values in binders ([`binders`], declared with
//! `paint_property_binders!`), which it names in its
//! [`upload`](Bucket::upload): the engine moves them into the context's
//! stores, where a layer's tile value reads them
//! ([`bucket_vertices`], [`bucket_indexes`], [`paint_vertices`]), and
//! rewrites the binders' values where feature state changes.
//!
//! The layer and every hook but the layout's run on the renderer's thread;
//! the layout and the bucket are `Send`, and a bucket crosses to a tile
//! worker with memory of its own through the worker codec.

pub mod bucket;

/// The binders of a layer type's data-driven paint values: what a bucket
/// keeps of each feature's values for its layers' draws, which feature
/// state rewrites, and the macro that declares a type's binders.
pub mod binders {
    pub use crate::layer::paint_property_binder::{
        BinderValue, CrossFadedPaintPropertyBinder, InterleavedVertexBuffer, InterleavedVertexData, LayerBinders,
        PaintBinders, PaintPropertyBinder, PaintPropertyBinderBase, PatternDependency, PatternLayerMap,
        bind_paint_property, shared_paint,
    };
    pub use crate::layer::paint_property_statistics::StatisticValue;
    pub use crate::paint_property_binders;
}
pub(crate) mod bucket_geometry;
pub(crate) mod bucket_parameters;
pub(crate) mod filter_groups;
pub(crate) mod group_by_layout;
pub(crate) mod layer_drawing;
pub(crate) mod layer_type;
pub(crate) mod layout;
pub(crate) mod paint_property_binder;
pub(crate) mod paint_property_statistics;
pub(crate) mod render_layer;
pub(crate) mod tile_input;
pub(crate) mod tile_layout;

pub use crate::layer::bucket::{Bucket, Upload};
pub use crate::layer::bucket_geometry::{
    BucketGeometry, GeometryVec, bucket_indexes, bucket_vertices, copied_indexes, copied_vertices, derived_vertices,
    paint_vertices,
};
pub use crate::layer::layer_drawing::BucketChanges;
pub use crate::layer::layer_type::EvaluatedLayer;
pub use crate::layer::layout::{BuildCx, Layout, LayoutCx, layers_by_id};
pub use crate::layer::render_layer::{
    EvaluateCx, Evaluation, Layer, LayerValues, OnTerrain, PrepareCx, TileCx, UpdateCx,
};
pub use crate::layer::tile_input::{Features, LayerBucket, NoTiles, RasterTile, TileData, TileInput};
pub use crate::layer::tile_layout::Resolved;
pub use crate::query::feature_index::QueryRegion;
#[doc(hidden)]
pub use crate::render::terrain::SurfaceLight;
pub use crate::render::tile::FrameTile;
pub use crate::source::render_image_source::ImageQuad;
pub use crate::source::render_source::TileOrder;
pub use crate::source::{DemTiles, RasterTiles};
pub use crate::tile_store::dem::DemContents;
