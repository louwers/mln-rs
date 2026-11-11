// Ports src/mln/tile/vector_mvt_tile_data.hpp and
// src/mln/tile/vector_mvt_tile_data.cpp.
//
// Exceptions: the C++ `getLayer`, `getFeature`, `getValue`,
// `getProperties` and `getGeometries` can throw (malformed tiles). Each has
// a `try_*` inherent method returning `Result<_, vector_tile::Error>` with
// the C++ exception. The trait's `get_layer` and `get_feature` return it;
// `get_value`, `get_properties` and `get_geometries` return null or empty
// and keep it for `GeometryTileFeature::error` (the trait has no other
// channel).

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use mln_tile_id::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryTileData, GeometryTileFeature, GeometryTileLayer,
};
use mln_util::constants::EXTENT;
use mln_util::exception::ExceptionPtr;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};
use mln_vec_map::VecMap;
use vector_tile::{Error, GeomType};

use crate::geometry_tile_data::fixup_polygons;

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();
static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

/// `VectorMVTTileFeature`.
pub struct VectorMVTTileFeature<'l> {
    feature: vector_tile::Feature<'l, 'l>,
    lines: OnceCell<GeometryCollection>,
    properties: OnceCell<PropertyMap>,
    /// The first exception of an accessor (`GeometryTileFeature::error`).
    error: OnceCell<ExceptionPtr>,
}

impl<'l> VectorMVTTileFeature<'l> {
    /// `VectorMVTTileFeature(const mapbox::vector_tile::layer&, const protozero::data_view&)`.
    pub fn new(layer: &'l vector_tile::Layer<'l>, view: &'l [u8]) -> Result<Self, Error> {
        Ok(VectorMVTTileFeature {
            feature: vector_tile::Feature::new(view, layer)?,
            lines: OnceCell::new(),
            properties: OnceCell::new(),
            error: OnceCell::new(),
        })
    }

    /// Keeps the first exception a trait accessor would have thrown.
    fn keep_error(&self, error: Error) {
        let _ = self.error.set(Arc::new(error));
    }

    /// `getValue`: `nullopt` for a null value.
    pub fn try_get_value(&self, key: &str) -> Result<Option<Value>, Error> {
        let value = self.feature.get_value(key)?;
        Ok(if value == Value::Null { None } else { Some(value) })
    }

    /// `getProperties` (computed once; a throwing call is retried next time).
    pub fn try_get_properties(&self) -> Result<&PropertyMap, Error> {
        if let Some(properties) = self.properties.get() {
            return Ok(properties);
        }
        let properties = self.feature.get_properties()?;
        Ok(self.properties.get_or_init(|| properties))
    }

    /// `getGeometries`: scaled to `util::EXTENT`; `std::runtime_error`s of
    /// the decoder are logged and give an empty collection, other exceptions
    /// propagate (and the next call retries). Version 1 polygons go through
    /// `fixupPolygons`; if that throws, the unfixed lines are kept for the
    /// next call, as in C++.
    pub fn try_get_geometries(&self) -> Result<&GeometryCollection, Error> {
        if let Some(lines) = self.lines.get() {
            return Ok(lines);
        }

        let scale = EXTENT as f32 / self.feature.get_extent() as f32;

        let mut lines = match self.feature.get_geometries::<i16>(scale) {
            Ok(paths) => GeometryCollection::from(paths),
            Err(Error::Runtime(what)) => {
                Log::error(Event::ParseTile, format!("Could not get geometries: {what}"));
                GeometryCollection::new()
            }
            Err(e) => return Err(e),
        };

        if self.feature.get_version() < 2 && self.feature.get_type() == GeomType::POLYGON {
            match fixup_polygons(&lines) {
                Ok(fixed) => lines = fixed,
                Err(e) => {
                    // C++ has already stored the decoded lines when
                    // fixupPolygons throws: the next call returns them.
                    let _ = self.lines.set(lines);
                    return Err(e);
                }
            }
        }

        Ok(self.lines.get_or_init(|| lines))
    }

    /// The `mapbox::vector_tile::feature` (for the difftest and the worker).
    pub fn feature(&self) -> &vector_tile::Feature<'l, 'l> {
        &self.feature
    }
}

impl GeometryTileFeature for VectorMVTTileFeature<'_> {
    fn get_type(&self) -> FeatureType {
        match self.feature.get_type() {
            GeomType::POINT => FeatureType::Point,
            GeomType::LINESTRING => FeatureType::LineString,
            GeomType::POLYGON => FeatureType::Polygon,
            _ => FeatureType::Unknown,
        }
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.try_get_value(key).unwrap_or_else(|e| {
            self.keep_error(e);
            None
        })
    }

    fn get_properties(&self) -> &PropertyMap {
        match self.try_get_properties() {
            Ok(properties) => properties,
            Err(e) => {
                self.keep_error(e);
                &EMPTY_PROPERTIES
            }
        }
    }

    fn get_id(&self) -> Identifier {
        self.feature.get_id().clone()
    }

    fn get_geometries(&self) -> &GeometryCollection {
        match self.try_get_geometries() {
            Ok(lines) => lines,
            Err(e) => {
                self.keep_error(e);
                &EMPTY_GEOMETRIES
            }
        }
    }

    fn error(&self) -> Option<ExceptionPtr> {
        self.error.get().cloned()
    }
}

/// `VectorMVTTileLayer`. C++ also holds the tile data (`shared_ptr`); here
/// the layer borrows it from the [`VectorMVTTileData`].
pub struct VectorMVTTileLayer<'d> {
    layer: vector_tile::Layer<'d>,
}

impl<'d> VectorMVTTileLayer<'d> {
    /// `VectorMVTTileLayer(std::shared_ptr<const std::string>, const protozero::data_view&)`.
    pub fn new(view: &'d [u8]) -> Result<Self, Error> {
        Ok(VectorMVTTileLayer { layer: vector_tile::Layer::new(view)? })
    }

    /// `getFeature(i)`: `std::out_of_range` past the end, protozero
    /// exceptions for a malformed feature.
    pub fn try_get_feature(&self, i: usize) -> Result<VectorMVTTileFeature<'_>, Error> {
        VectorMVTTileFeature::new(&self.layer, self.layer.get_feature(i)?)
    }

    /// The `mapbox::vector_tile::layer`.
    pub fn layer(&self) -> &vector_tile::Layer<'d> {
        &self.layer
    }
}

impl GeometryTileLayer for VectorMVTTileLayer<'_> {
    fn feature_count(&self) -> usize {
        self.layer.feature_count()
    }

    fn get_feature(&self, index: usize) -> Result<Box<dyn GeometryTileFeature + '_>, ExceptionPtr> {
        match self.try_get_feature(index) {
            Ok(feature) => Ok(Box::new(feature)),
            Err(e) => Err(Arc::new(e)),
        }
    }

    fn get_name(&self) -> String {
        self.layer.get_name()
    }

    /// Groups by geometry type and by the value-table index of each key's
    /// first tag, as `getValue` reads it: equal indices are equal values.
    /// Equal values at distinct indices only split a group. `None` for a
    /// malformed feature, leaving its errors to the accessors.
    fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        /// A key the feature has no tag for.
        const ABSENT: u32 = u32::MAX;
        let layer = &self.layer;
        let count = layer.feature_count();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let slots: Vec<Option<usize>> =
            (layer.keys().iter()).map(|key| keys.iter().position(|k| k.as_bytes() == *key)).collect();
        let mut index: HashMap<Box<[u32]>, u32> = HashMap::new();
        let mut codes = vec![ABSENT; keys.len() + 1];
        let mut groups = FeatureGroups { of_feature: Vec::with_capacity(count), first: Vec::new() };
        for i in 0..count {
            let feature = vector_tile::Feature::new(layer.get_feature(i).ok()?, layer).ok()?;
            codes.fill(ABSENT);
            codes[keys.len()] = u32::from(feature.get_type().0);
            let mut valid = true;
            feature
                .for_each_tag(|key, value| {
                    valid &= value != ABSENT;
                    if let Some(slot) = slots[key as usize]
                        && codes[slot] == ABSENT
                    {
                        codes[slot] = value;
                    }
                })
                .ok()?;
            if !valid {
                return None;
            }
            let group = match index.get(codes.as_slice()) {
                Some(&group) => group,
                None => {
                    if groups.first.len() == limit {
                        return None;
                    }
                    let group = groups.first.len() as u32;
                    index.insert(codes.as_slice().into(), group);
                    groups.first.push(i);
                    group
                }
            };
            groups.of_feature.push(group);
        }
        Some(groups)
    }
}

/// `VectorMVTTileData`: the layers are indexed lazily, on the first
/// `getLayer`, so the object can be made on the main thread cheaply.
pub struct VectorMVTTileData {
    /// C++ `std::shared_ptr<const std::string>`.
    data: Arc<Vec<u8>>,
    /// C++ `mutable bool parsed` + `mutable std::map<std::string, const
    /// protozero::data_view> layers`; views as (offset, length) into `data`.
    // OnceLock (not OnceCell): the tile data is shared with the tile
    // worker thread (mln_renderer, `Arc<dyn GeometryTileData + Send + Sync>`).
    layers: std::sync::OnceLock<VecMap<Vec<u8>, (usize, usize)>>,
}

impl VectorMVTTileData {
    pub fn new(data: Arc<Vec<u8>>) -> Self {
        VectorMVTTileData { data, layers: std::sync::OnceLock::new() }
    }

    pub fn data(&self) -> &Arc<Vec<u8>> {
        &self.data
    }

    /// `getLayer`: `None` for an unknown layer. Errors: the
    /// `mapbox::vector_tile::buffer` errors (only the first successful parse
    /// is cached, as C++ sets `parsed` after it) and the
    /// `mapbox::vector_tile::layer` constructor errors.
    pub fn try_get_layer(&self, name: &str) -> Result<Option<VectorMVTTileLayer<'_>>, Error> {
        let layers = match self.layers.get() {
            Some(layers) => layers,
            None => {
                // We're parsing this lazily so that we can construct VectorTileData
                // objects on the main thread without incurring the overhead of parsing
                // immediately.
                let base = self.data.as_ptr() as usize;
                let layers = vector_tile::Buffer::new(&self.data)?
                    .get_layers()
                    .into_iter()
                    .map(|(name, view)| (name, (view.as_ptr() as usize - base, view.len())))
                    .collect();
                self.layers.get_or_init(|| layers)
            }
        };

        match layers.get(name.as_bytes()) {
            Some(&(offset, len)) => Ok(Some(VectorMVTTileLayer::new(&self.data[offset..offset + len])?)),
            None => Ok(None),
        }
    }

    /// `layerNames()` (parses the buffer again, as C++ does).
    pub fn layer_names(&self) -> Result<Vec<String>, Error> {
        Ok(vector_tile::Buffer::new(&self.data)?.layer_names())
    }
}

impl GeometryTileData for VectorMVTTileData {
    /// `clone()`: shares the data, not the parsed index.
    fn clone_data(&self) -> Box<dyn GeometryTileData> {
        Box::new(VectorMVTTileData::new(self.data.clone()))
    }

    fn get_layer(&self, name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, ExceptionPtr> {
        match self.try_get_layer(name) {
            Ok(layer) => Ok(layer.map(|layer| Box::new(layer) as Box<dyn GeometryTileLayer + '_>)),
            Err(e) => Err(Arc::new(e)),
        }
    }

    fn encoded(&self) -> Option<EncodedTileData> {
        Some(EncodedTileData::Mvt(self.data.clone()))
    }
}
