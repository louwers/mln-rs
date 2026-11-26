//! Tiles' ids and data: unwrapped and overscaled tile ids around
//! `mln_style::tile_id`'s canonical one, and tile ranges ([`tile_id`],
//! [`tile_range`]); the layers and features of vector tiles, and the
//! geometry functions that work on them; and DEM data (`dem_data`).
//!
//! - [`TileData`]: a tile's data, which is an MVT tile read lazily with
//!   fast-mvt, an MLT tile decoded with mlt-core, or features already in
//!   tile coordinates (GeoJSON and custom geometry sources). Its layers
//!   ([`TileLayer`]) and features ([`Feature`]) are views that borrow it.
//! - [`geometry_tile_data`]: hole limits, polygon fixup on `i_overlay`,
//!   feature conversion and corner rounding. The feature interface that the
//!   tiles implement, ring classification and coordinate conversion are in
//!   `mln_style::feature`.

pub mod cut;
pub mod dem;
pub(crate) mod dem_data; // dem-heatmap
mod error;
pub mod feature;
pub mod geometry_tile_data;
mod mlt;
mod mvt;
mod tile_data;
pub mod tile_id;
pub mod tile_range;

pub use error::TileError;
pub use geometry_tile_data::{
    convert_feature, is_boundary_edge, is_entirely_outside, limit_holes, round_polygon_corners,
};
pub use mlt::{MltFeature, MltLayer, MltTile};
pub use mvt::{MvtFeature, MvtLayer, MvtTile};
pub use tile_data::{Feature, FeatureGroups, GeoJsonFeature, TileData, TileLayer};
pub use tile_id::{OverscaledTileID, UnwrappedTileID};

#[cfg(test)]
mod malformed_tile_test;

#[cfg(test)]
mod vector_tile_test;
