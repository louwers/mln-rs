//! Ports the tile data side of `src/mln/tile`:
//!
//! - `vector_mvt_tile_data.{hpp,cpp}` → [`vector_mvt_tile_data`]
//!   (`VectorMVTTileData`, `VectorMVTTileLayer`, `VectorMVTTileFeature` on
//!   `//vendor/vector-tile`),
//! - `vector_mlt_tile_data.{hpp,cpp}` → [`vector_mlt_tile_data`]
//!   (`VectorMLTTileData`, `VectorMLTTileLayer`, `VectorMLTTileFeature`) on
//!   [`mlt`], the C++ MLT decoder's output model filled by the mlt-core crate,
//! - the free functions of `geometry_tile_data.cpp` →
//!   [`geometry_tile_data`] (`classify_rings`, `limit_holes`,
//!   `fixup_polygons` on `//vendor/wagyu`, `convert_geometry`,
//!   `convert_feature`, `to_geometry_collection`); the interfaces are in
//!   `mln_tile_id`.
//!
//! This crate is deliberately low in the crate graph (no style, renderer or
//! expression dependencies), so `mln_expression` and the buckets can use
//! the geometry functions. The rest of `tile/` (Tile, GeometryTile, the
//! workers, ...) depends on the renderer and goes into a higher crate.

pub mod geometry_tile_data;
pub mod mlt;
pub mod stl_algo;
pub mod vector_mlt_tile_data;
pub mod vector_mvt_tile_data;

pub use geometry_tile_data::{
    classify_rings, convert_feature, convert_geometry, convert_geometry_to_tile, fixup_polygons, limit_holes,
    to_geometry_collection,
};
pub use vector_mlt_tile_data::{VectorMLTTileData, VectorMLTTileFeature, VectorMLTTileLayer};
pub use vector_mvt_tile_data::{VectorMVTTileData, VectorMVTTileFeature, VectorMVTTileLayer};
pub use vector_tile::Error;

#[cfg(test)]
mod malformed_tile_test;
#[cfg(test)]
mod vector_tile_test;
