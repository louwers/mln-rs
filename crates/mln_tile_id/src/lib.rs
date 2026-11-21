//! Tile ids (canonical, unwrapped and overscaled), tile ranges, feature
//! geometry with its ring classification and its conversion to and from
//! longitude/latitude, and the interface through which expressions and
//! buckets read a feature.

pub mod geometry_tile_data;
pub mod tile_id;
pub mod tile_range;

pub use geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};
pub use tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
