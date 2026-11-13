//! Tile data: the decoded layers and features of vector tiles, and the
//! geometry functions that work on them.
//!
//! - [`vector_mvt_tile_data`]: Mapbox Vector Tile data (`VectorMVTTileData`,
//!   `VectorMVTTileLayer`, `VectorMVTTileFeature`) on the fast-mvt reader.
//! - [`vector_mlt_tile_data`]: MapLibre Tile data (`VectorMLTTileData`,
//!   `VectorMLTTileLayer`, `VectorMLTTileFeature`) on [`mlt`], the C++ MLT
//!   decoder's output model filled by the mlt-core crate.
//! - [`geometry_tile_data`]: ring classification, hole limits, polygon fixup
//!   on `i_overlay`, coordinate conversion and corner rounding. The feature
//!   interfaces are in `mln_tile_id`.
//!
//! This crate is deliberately low in the crate graph (no style, renderer or
//! expression dependencies), so `mln_expression` and the buckets can use
//! the geometry functions. The tiles themselves, and their workers, depend
//! on the renderer and live in a higher crate.

mod error;
pub mod geometry_tile_data;
pub mod mlt;
pub mod vector_mlt_tile_data;
pub mod vector_mvt_tile_data;

pub use error::Error;
pub use geometry_tile_data::{
    classify_rings, convert_feature, convert_geometry, convert_geometry_to_tile, fixup_polygons, limit_holes,
    round_polygon_corners, to_geometry_collection,
};
pub use vector_mlt_tile_data::{VectorMLTTileData, VectorMLTTileFeature, VectorMLTTileLayer};
pub use vector_mvt_tile_data::{VectorMVTTileData, VectorMVTTileFeature, VectorMVTTileLayer};

#[cfg(test)]
mod malformed_tile_test;
#[cfg(test)]
mod vector_tile_test;
