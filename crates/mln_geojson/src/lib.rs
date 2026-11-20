//! A GeoJSON source's features, prepared for drawing: [`cluster`] groups a
//! clustered source's points into clusters for each zoom.

pub mod cluster;
mod mercator;

use std::sync::Arc;

use feature::{Feature, PropertyMap};

/// A tile's features, in tile coordinates. Features that come from one
/// source feature share its property map.
pub type TileFeatures = Vec<Feature<i16, Arc<PropertyMap>>>;
