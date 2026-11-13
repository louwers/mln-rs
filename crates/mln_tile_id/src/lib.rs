//! Tile ids (canonical, unwrapped and overscaled), tile ranges, and the
//! interfaces of tile data: layers, features and their geometry.

pub mod geometry_tile_data;
pub mod tile_id;
pub mod tile_range;

pub use geometry_tile_data::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileData,
    GeometryTileFeature, GeometryTileLayer,
};
pub use tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
