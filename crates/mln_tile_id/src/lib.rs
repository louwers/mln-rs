//! Ports `include/mln/tile/tile_id.hpp` (with `tile_id_io.cpp`,
//! `tile_id_hash.cpp`) and the interfaces of
//! `src/mln/tile/geometry_tile_data.hpp`, and `src/mln/util/tile_range.hpp`.

pub mod geometry_tile_data;
pub mod tile_id;
pub mod tile_range;

pub use geometry_tile_data::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileData,
    GeometryTileFeature, GeometryTileLayer,
};
pub use tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
