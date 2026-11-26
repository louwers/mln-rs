use std::borrow::Cow;

use feature::{Feature, Identifier, PropertyMap, Value};

use super::{GeometryCollection, GeometryTileFeature, convert_geometry_to_tile};
use crate::error::Error;
use crate::geometry::{FeatureType, to_feature_type};
use crate::tile_id::CanonicalTileID;

/// A `GeometryTileFeature` view of a GeoJSON feature, with its geometry in
/// the coordinates of a tile, or none.
pub(crate) struct GeoJSONFeature<'a> {
    feature: &'a Feature<f64>,
    geometry: GeometryCollection,
}

impl<'a> GeoJSONFeature<'a> {
    /// The view without a geometry.
    pub(crate) fn new(feature: &'a Feature<f64>) -> Self {
        GeoJSONFeature { feature, geometry: GeometryCollection::default() }
    }

    /// The view with its geometry in `canonical`'s coordinates.
    pub(crate) fn with_canonical(feature: &'a Feature<f64>, canonical: &CanonicalTileID) -> Self {
        let geometry = feature
            .geometry
            .as_ref()
            .map_or_else(GeometryCollection::default, |g| convert_geometry_to_tile(g, canonical));
        GeoJSONFeature { feature, geometry }
    }
}

impl GeometryTileFeature for GeoJSONFeature<'_> {
    fn get_type(&self) -> FeatureType {
        to_feature_type(self.feature.geometry.as_ref())
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&self.feature.properties)
    }

    fn get_id(&self) -> Identifier {
        self.feature.id.clone()
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.feature.properties.get(key).cloned()
    }

    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}
