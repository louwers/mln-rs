//! What expressions read of a feature: the [`GeometryTileFeature`]
//! interface, its geometry in tile coordinates, its state, the tile it's in,
//! and the conversions of its geometry to and from longitude/latitude.

mod convert;
pub(crate) mod geojson_feature;
mod tile_feature;

pub use convert::{
    OutOfRange, classify_rings, convert_geometry, convert_geometry_to_tile, signed_area, to_geometry_collection,
};
pub use tile_feature::{GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};

pub use crate::geometry::FeatureType;

/// A feature's state, as `feature-state` expressions read it.
pub type FeatureState = feature::ValueObject;
