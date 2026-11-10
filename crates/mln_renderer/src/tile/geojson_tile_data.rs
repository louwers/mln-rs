// Ports src/mln/tile/geojson_tile_data.hpp.
//
// Implements a simple in-memory Tile type that holds GeoJSON values. A
// GeoJSON tile can only have one layer, and it is always returned
// regardless of which layer is requested.

use std::cell::OnceCell;
use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use mln_tile::{fixup_polygons, to_geometry_collection};
use mln_tile_id::EncodedTileData;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileData, GeometryTileFeature, GeometryTileLayer};
use mln_util::exception::ExceptionPtr;
use mln_util::geometry::{FeatureType, to_feature_type};

/// `mapbox::feature::feature_collection<int16_t>`.
pub type TileFeatureCollection = Vec<Feature<i16>>;

/// `GeoJSONTileFeature`.
pub struct GeoJSONTileFeature<'a> {
    pub feature: &'a Feature<i16>,
    /// `mutable std::optional<GeometryCollection> geometry`.
    geometry: OnceCell<GeometryCollection>,
    /// The wagyu exception of `getGeometries` (`GeometryTileFeature::error`).
    error: OnceCell<ExceptionPtr>,
}

static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

impl<'a> GeoJSONTileFeature<'a> {
    pub fn new(feature: &'a Feature<i16>) -> Self {
        GeoJSONTileFeature { feature, geometry: OnceCell::new(), error: OnceCell::new() }
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
        if let Some(geometry) = self.geometry.get() {
            return geometry;
        }
        let mut geometry = to_geometry_collection(&self.feature.geometry);

        // https://github.com/mapbox/geojson-vt-cpp/issues/44
        if self.get_type() == FeatureType::Polygon {
            // C++ lets a wagyu exception propagate (the worker reports a
            // tile error) and retries on the next call.
            match fixup_polygons(&geometry) {
                Ok(fixed) => geometry = fixed,
                Err(e) => {
                    let _ = self.error.set(Arc::new(e));
                    return &EMPTY_GEOMETRIES;
                }
            }
        }
        self.geometry.get_or_init(|| geometry)
    }

    fn error(&self) -> Option<ExceptionPtr> {
        self.error.get().cloned()
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.feature.properties.get(key).cloned()
    }
}

/// `GeoJSONTileLayer`.
pub struct GeoJSONTileLayer {
    features: Arc<TileFeatureCollection>,
}

impl GeoJSONTileLayer {
    pub fn new(features: Arc<TileFeatureCollection>) -> Self {
        GeoJSONTileLayer { features }
    }
}

impl GeometryTileLayer for GeoJSONTileLayer {
    fn feature_count(&self) -> usize {
        self.features.len()
    }

    fn get_feature(&self, i: usize) -> Result<Box<dyn GeometryTileFeature + '_>, ExceptionPtr> {
        Ok(Box::new(GeoJSONTileFeature::new(&self.features[i])))
    }

    fn get_name(&self) -> String {
        String::new()
    }
}

/// `GeoJSONTileData`.
#[derive(Clone)]
pub struct GeoJSONTileData {
    features: Arc<TileFeatureCollection>,
}

impl GeoJSONTileData {
    pub fn new(features: TileFeatureCollection) -> Self {
        GeoJSONTileData { features: Arc::new(features) }
    }

    pub fn from_shared(features: Arc<TileFeatureCollection>) -> Self {
        GeoJSONTileData { features }
    }

    pub fn features(&self) -> &Arc<TileFeatureCollection> {
        &self.features
    }
}

impl GeometryTileData for GeoJSONTileData {
    fn clone_data(&self) -> Box<dyn GeometryTileData> {
        Box::new(GeoJSONTileData::from_shared(self.features.clone()))
    }

    fn get_layer(&self, _name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, ExceptionPtr> {
        Ok(Some(Box::new(GeoJSONTileLayer::new(self.features.clone()))))
    }

    fn encoded(&self) -> Option<EncodedTileData> {
        Some(EncodedTileData::GeoJson(self.features.clone()))
    }
}
