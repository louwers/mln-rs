// Implements a simple in-memory Tile type that holds GeoJSON values. A
// GeoJSON tile can only have one layer, and it is always returned
// regardless of which layer is requested.

use std::cell::OnceCell;
use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use mln_tile::to_geometry_collection;
use mln_tile_id::EncodedTileData;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileData, GeometryTileFeature, GeometryTileLayer};
use mln_util::error::Error;
use mln_util::geometry::{FeatureType, to_feature_type};

pub(crate) type TileFeatureCollection = Vec<Feature<i16>>;

pub(crate) struct GeoJSONTileFeature<'a> {
    pub(crate) feature: &'a Feature<i16>,
    geometry: OnceCell<GeometryCollection>,
}

impl<'a> GeoJSONTileFeature<'a> {
    #[must_use]
    pub(crate) fn new(feature: &'a Feature<i16>) -> Self {
        GeoJSONTileFeature { feature, geometry: OnceCell::new() }
    }
}

impl GeometryTileFeature for GeoJSONTileFeature<'_> {
    fn get_type(&self) -> FeatureType {
        to_feature_type(&self.feature.geometry)
    }

    fn get_properties(&self) -> &PropertyMap {
        &self.feature.properties
    }

    fn get_id(&self) -> Identifier {
        self.feature.id.clone()
    }

    fn get_geometries(&self) -> &GeometryCollection {
        self.geometry.get_or_init(|| to_geometry_collection(&self.feature.geometry))
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.feature.properties.get(key).cloned()
    }
}

pub(crate) struct GeoJSONTileLayer {
    features: Arc<TileFeatureCollection>,
}

impl GeoJSONTileLayer {
    #[must_use]
    pub(crate) fn new(features: Arc<TileFeatureCollection>) -> Self {
        GeoJSONTileLayer { features }
    }
}

impl GeometryTileLayer for GeoJSONTileLayer {
    fn feature_count(&self) -> usize {
        self.features.len()
    }

    fn get_feature(&self, i: usize) -> Result<Box<dyn GeometryTileFeature + '_>, Error> {
        Ok(Box::new(GeoJSONTileFeature::new(&self.features[i])))
    }

    fn get_name(&self) -> String {
        String::new()
    }
}

#[derive(Clone)]
pub(crate) struct GeoJSONTileData {
    features: Arc<TileFeatureCollection>,
}

impl GeoJSONTileData {
    #[must_use]
    pub(crate) fn new(features: TileFeatureCollection) -> Self {
        GeoJSONTileData { features: Arc::new(features) }
    }

    #[must_use]
    pub(crate) fn from_shared(features: Arc<TileFeatureCollection>) -> Self {
        GeoJSONTileData { features }
    }
}

impl GeometryTileData for GeoJSONTileData {
    fn clone_data(&self) -> Box<dyn GeometryTileData> {
        Box::new(GeoJSONTileData::from_shared(Arc::clone(&self.features)))
    }

    fn get_layer(&self, _name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, Error> {
        Ok(Some(Box::new(GeoJSONTileLayer::new(Arc::clone(&self.features)))))
    }

    fn encoded(&self) -> Option<EncodedTileData> {
        Some(EncodedTileData::GeoJson(Arc::clone(&self.features)))
    }
}
