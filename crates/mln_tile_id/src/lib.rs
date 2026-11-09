//! Ports `include/mln/tile/tile_id.hpp` (with `tile_id_io.cpp`,
//! `tile_id_hash.cpp`) and the interfaces of
//! `src/mln/tile/geometry_tile_data.hpp`.

pub mod geometry_tile_data;
pub mod tile_id;

pub use geometry_tile_data::{
    EncodedTileData, GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileData,
    GeometryTileFeature, GeometryTileLayer,
};
pub use tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
