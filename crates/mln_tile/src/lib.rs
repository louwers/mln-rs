//! Tile data: the layers and features of vector tiles, and the geometry
//! functions that work on them.
//!
//! - [`TileData`]: a tile's data, which is an MVT tile read lazily with
//!   fast-mvt, an MLT tile decoded with mlt-core, or features already in
//!   tile coordinates (GeoJSON and custom geometry sources). Its layers
//!   ([`TileLayer`]) and features ([`Feature`]) are views that borrow it.
//! - [`geometry_tile_data`]: ring classification, hole limits, polygon fixup
//!   on `i_overlay`, coordinate conversion and corner rounding. The feature
//!   interface that expressions read is in `mln_tile_id`.
//!
//! This crate is deliberately low in the crate graph (no style, renderer or
//! expression dependencies), so `mln_expression` and the buckets can use
//! the geometry functions. The tiles themselves, and their workers, depend
//! on the renderer and live in a higher crate.

mod error;
pub mod geometry_tile_data;
mod mlt;
mod mvt;
mod tile_data;

pub use error::TileError;
pub use geometry_tile_data::{
    classify_rings, convert_feature, convert_geometry, convert_geometry_to_tile, limit_holes, round_polygon_corners,
    to_geometry_collection,
};
pub use mlt::{MltFeature, MltLayer, MltTile};
pub use mvt::{MvtFeature, MvtLayer, MvtTile};
pub use tile_data::{Feature, FeatureGroups, GeoJsonFeature, TileBytes, TileData, TileFeatures, TileLayer};

#[cfg(test)]
mod malformed_tile_test;
#[cfg(test)]
mod vector_tile_test;
