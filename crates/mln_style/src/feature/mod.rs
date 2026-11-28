//! Features and what expressions read of them.
//!
//! - [`Value`], [`Identifier`] and [`Feature`], whose geometry is
//!   `geo_types`': a port of `mapbox::feature` (mapbox/feature.hpp of
//!   geometry.hpp, which `include/mln/util/feature.hpp` re-exports into the
//!   `mln` namespace), and [`geojson`], which converts the `geojson` crate's
//!   features to these. Deliberately not `serde_json::Value`: [`Value`]
//!   keeps the `u64`/`i64`/`f64` distinction that the expression and
//!   conversion code relies on.
//! - The [`GeometryTileFeature`] interface that expressions read, a
//!   feature's geometry in tile coordinates, its state, the tile it's in,
//!   and the conversions of its geometry to and from longitude/latitude.

mod convert;
pub mod geojson;
pub(crate) mod geojson_feature;
mod item;
mod tile_feature;
mod value;

pub use convert::{
    OutOfRange, classify_rings, convert_geometry, convert_geometry_to_tile, convert_geometry_to_world, signed_area,
    to_geometry_collection,
};
pub use item::{Feature, Identifier, PropertyMap};
pub use tile_feature::{GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};
pub use value::{Value, ValueObject};

pub use crate::geometry::FeatureType;

/// A feature's state, as `feature-state` expressions read it.
pub type FeatureState = ValueObject;
