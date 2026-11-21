//! Tile data: the layers and features of vector tiles, and the geometry
//! functions that work on them.
//!
//! - [`TileData`]: a tile's data, which is an MVT tile read lazily with
//!   fast-mvt, an MLT tile decoded with mlt-core, or features already in
//!   tile coordinates (GeoJSON and custom geometry sources). Its layers
//!   ([`TileLayer`]) and features ([`Feature`]) are views that borrow it.
//! - [`geometry_tile_data`]: hole limits, polygon fixup on `i_overlay`,
//!   feature conversion and corner rounding. Ring classification, coordinate
//!   conversion and the feature interface that expressions read are in
//!   `mln_tile_id`.
//!
//! This crate is low in the crate graph (no style or renderer dependencies),
//! so the buckets can use the geometry functions. The tiles themselves, and
//! their workers, depend on the renderer and live in a higher crate.

pub mod cut;
mod error;
pub mod geometry_tile_data;
mod mlt;
mod mvt;
mod tile_data;

pub use error::TileError;
pub use geometry_tile_data::{
    convert_feature, is_boundary_edge, is_entirely_outside, limit_holes, round_polygon_corners,
};
pub use mlt::{MltFeature, MltLayer, MltTile};
pub use mvt::{MvtFeature, MvtLayer, MvtTile};
pub use tile_data::{Feature, FeatureGroups, GeoJsonFeature, TileData, TileLayer};

#[cfg(test)]
mod malformed_tile_test;
#[cfg(test)]
mod vector_tile_test;
