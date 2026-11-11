// Ports src/mln/tile/vector_mlt_tile_data.hpp and
// src/mln/tile/vector_mlt_tile_data.cpp.
//
// The tile is decoded by `mlt::MapLibreTile::decode` (crate::mlt, on the
// mlt-core crate) where C++ calls `mlt::Decoder(fastPFOREnabled).decode`.
//
// Exceptions: reading a property can throw (`PresentProperties::getProperty`,
// "Missing property value", on malformed tiles). `try_get_value` and
// `try_get_properties` return `Result<_, mlt::Error>`; the trait methods
// return null or empty and keep the exception for
// `GeometryTileFeature::error`, as VectorMVTTileFeature does.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use feature::{Identifier, PropertyMap, Value};
use mln_tile_id::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileData,
    GeometryTileFeature, GeometryTileLayer,
};
use mln_util::constants::EXTENT;
use mln_util::exception::ExceptionPtr;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};
use mln_util::sync::MutexExt;

use crate::mlt::{self, Coordinate, MapLibreTile, Property, Shape};

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();

/// `PropertyVisitor`: the `mln::Value` of an MLT property (32-bit integers
/// widen to `int64_t`/`uint64_t`, `float` to `double`).
fn property_value(property: Property<'_>) -> Value {
    match property {
        Property::Bool(b) => Value::Bool(b),
        Property::I32(i) => Value::Int(i64::from(i)),
        Property::U32(u) => Value::Uint(u64::from(u)),
        Property::I64(i) => Value::Int(i),
        Property::U64(u) => Value::Uint(u),
        Property::F32(f) => Value::Double(f64::from(f)),
        Property::F64(d) => Value::Double(d),
        Property::Str(s) => Value::String(s.to_string()),
    }
}

/// `PointConverter`: `static_cast<int16_t>(std::round(coord * scale))`, the
/// cast as clang compiles it on x86-64 (out of range: the low 16 bits of
/// cvttsd2si).
fn convert_point(scale: f64, coord: &Coordinate) -> GeometryCoordinate {
    GeometryCoordinate::new(
        libstdcxx::cvt::f64_to_i16((f64::from(coord.x) * scale).round()),
        libstdcxx::cvt::f64_to_i16((f64::from(coord.y) * scale).round()),
    )
}

fn convert_coords(scale: f64, coords: &[Coordinate]) -> GeometryCoordinates {
    coords.iter().map(|c| convert_point(scale, c)).collect()
}

/// `VectorMLTTileFeature`.
pub struct VectorMLTTileFeature<'l> {
    layer: &'l mlt::Layer,
    feature: &'l mlt::Feature,
    extent: u32,
    // lazy init
    lines: OnceCell<GeometryCollection>,
    properties: OnceCell<PropertyMap>,
    /// The first exception of an accessor (`GeometryTileFeature::error`).
    error: OnceCell<ExceptionPtr>,
}

impl<'l> VectorMLTTileFeature<'l> {
    pub fn new(layer: &'l mlt::Layer, feature: &'l mlt::Feature, extent: u32) -> Self {
        VectorMLTTileFeature {
            layer,
            feature,
            extent,
            lines: OnceCell::new(),
            properties: OnceCell::new(),
            error: OnceCell::new(),
        }
    }

    /// Keeps the first exception a trait accessor would have thrown.
    fn keep_error(&self, error: mlt::Error) {
        let _ = self.error.set(Arc::new(error));
    }

    /// The `mlt::Feature` (for the difftest).
    pub fn feature(&self) -> &'l mlt::Feature {
        self.feature
    }

    /// `getValue`: `nullopt` for an unknown key or an absent value.
    pub fn try_get_value(&self, key: &str) -> Result<Option<Value>, mlt::Error> {
        Ok(self.feature.get_property(key, self.layer)?.map(property_value))
    }

    /// `getProperties`: every column, `null_value` for an absent value. If a
    /// column throws, the C++ map is already engaged: the next call returns
    /// the columns read before it (here in wire order; C++ in hash order).
    pub fn try_get_properties(&self) -> Result<&PropertyMap, mlt::Error> {
        if let Some(properties) = self.properties.get() {
            return Ok(properties);
        }
        let mut properties = PropertyMap::new();
        for (key, column) in self.layer.get_properties() {
            match column.get_property(self.feature.get_index()) {
                Ok(value) => {
                    properties.entry(key.clone()).or_insert_with(|| value.map_or(Value::Null, property_value));
                }
                Err(e) => {
                    let _ = self.properties.set(properties);
                    return Err(e);
                }
            }
        }
        Ok(self.properties.get_or_init(|| properties))
    }
}

/// `VectorMLTTileFeature::getType`.
fn feature_type(feature: &mlt::Feature) -> FeatureType {
    match feature.get_geometry().shape {
        Shape::Point(_) => FeatureType::Point,
        // C++ reports multi-points as LineString (the MVT path: Point).
        Shape::MultiPoint(_) | Shape::MultiLineString(_) | Shape::LineString(_) => FeatureType::LineString,
        Shape::Polygon(_) | Shape::MultiPolygon(_) => FeatureType::Polygon,
    }
}

impl GeometryTileFeature for VectorMLTTileFeature<'_> {
    fn get_type(&self) -> FeatureType {
        feature_type(self.feature)
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

    fn error(&self) -> Option<ExceptionPtr> {
        self.error.get().cloned()
    }

    fn get_id(&self) -> Identifier {
        match self.feature.get_id() {
            Some(id) => Identifier::Uint(id),
            None => Identifier::Null,
        }
    }

    fn get_geometries(&self) -> &GeometryCollection {
        self.lines.get_or_init(|| {
            let scale = f64::from(EXTENT) / f64::from(self.extent);
            let geometry = self.feature.get_geometry();
            let mut lines = match &geometry.shape {
                Shape::Point(coord) => GeometryCollection::from(vec![vec![convert_point(scale, coord)]]),
                Shape::MultiPoint(coords) | Shape::LineString(coords) => {
                    GeometryCollection::from(vec![convert_coords(scale, coords)])
                }
                Shape::Polygon(rings) | Shape::MultiLineString(rings) => {
                    GeometryCollection::from(rings.iter().map(|ring| convert_coords(scale, ring)).collect::<Vec<_>>())
                }
                Shape::MultiPolygon(polygons) => {
                    // C++ `lines.emplace(<ring count>)` makes that many empty
                    // rings, and the rings are then appended after them
                    // (std::back_inserter). Empty rings have no area, so
                    // classifyRings drops them; ported as is.
                    let count = polygons.iter().map(Vec::len).sum();
                    let mut rings = vec![GeometryCoordinates::new(); count];
                    rings.extend(polygons.iter().flatten().map(|ring| convert_coords(scale, ring)));
                    GeometryCollection::from(rings)
                }
            };
            if matches!(geometry.shape, Shape::Polygon(_) | Shape::MultiPolygon(_))
                && let (Some(owner), Some(range)) = (self.layer.triangle_buffer(), &geometry.triangles)
            {
                lines.set_triangles(owner.clone(), range.clone());
            }
            lines
        })
    }
}

/// `VectorMLTTileLayer`: holds the decoded tile, as C++ holds its `shared_ptr`.
pub struct VectorMLTTileLayer {
    tile: Arc<MapLibreTile>,
    layer: usize,
}

impl VectorMLTTileLayer {
    /// The `mlt::Layer`.
    pub fn layer(&self) -> &mlt::Layer {
        &self.tile.get_layers()[self.layer]
    }

    /// `getFeature(index)`. C++ indexes the feature vector unchecked; an index
    /// past the end panics here.
    pub fn feature(&self, index: usize) -> VectorMLTTileFeature<'_> {
        let layer = self.layer();
        VectorMLTTileFeature::new(layer, &layer.get_features()[index], layer.get_extent())
    }
}

impl GeometryTileLayer for VectorMLTTileLayer {
    fn feature_count(&self) -> usize {
        self.layer().get_features().len()
    }

    fn get_feature(&self, index: usize) -> Result<Box<dyn GeometryTileFeature + '_>, ExceptionPtr> {
        Ok(Box::new(self.feature(index)))
    }

    fn get_name(&self) -> String {
        self.layer().get_name().to_string()
    }

    /// Groups by geometry type and by each key's value code
    /// ([`mlt::PresentProperties::codes`]): equal codes are equal values,
    /// and equal values with distinct codes only split a group. A key
    /// without a column is absent from every feature. `None` for a value
    /// that would throw, leaving its error to the accessors.
    fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        let layer = self.layer();
        let features = layer.get_features();
        let count = features.len();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let columns: Vec<mlt::ValueCodes<'_>> = keys
            .iter()
            .filter_map(|key| layer.get_property_column(key))
            .map(|column| column.codes())
            .collect::<Option<_>>()?;
        let mut index: HashMap<Box<[Option<u64>]>, u32> = HashMap::new();
        let mut codes = vec![None; columns.len() + 1];
        let mut groups = FeatureGroups { of_feature: Vec::with_capacity(count), first: Vec::new() };
        for (i, feature) in features.iter().enumerate() {
            codes[columns.len()] = Some(feature_type(feature) as u64);
            for (code, column) in codes.iter_mut().zip(&columns) {
                *code = column.get(feature.get_index()).ok()?;
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

/// `VectorMLTTileData::Impl`'s mutable state.
#[derive(Clone, Default)]
struct State {
    data: Option<Arc<Vec<u8>>>,
    tile: Option<Arc<MapLibreTile>>,
}

/// `VectorMLTTileData`: the tile is decoded on the first `getLayer` (or
/// `layerNames`); a decoding error is logged and leaves it without layers.
pub struct VectorMLTTileData {
    state: Mutex<State>,
    fast_pfor_enabled: bool,
}

impl VectorMLTTileData {
    /// `VectorMLTTileData(std::shared_ptr<const std::string>, bool fastPFOREnabled)`.
    pub fn new(data: Arc<Vec<u8>>, fast_pfor_enabled: bool) -> Self {
        VectorMLTTileData { state: Mutex::new(State { data: Some(data), tile: None }), fast_pfor_enabled }
    }

    /// The decoded tile (`Impl::getLayer`'s decoding step): decodes once,
    /// then drops the raw data.
    pub fn tile(&self) -> Option<Arc<MapLibreTile>> {
        let mut state = self.state.lock_unpoisoned();
        if let Some(data) = &state.data
            && state.tile.is_none()
        {
            match MapLibreTile::decode(data, self.fast_pfor_enabled) {
                Ok(tile) => state.tile = Some(Arc::new(tile)),
                Err(e) => Log::warning(Event::ParseTile, format!("MLT parse failed: {e}")),
            }
            // We don't need the raw data anymore
            state.data = None;
        }
        state.tile.clone()
    }

    /// `getLayer`, with its concrete type.
    pub fn get_mlt_layer(&self, name: &str) -> Option<VectorMLTTileLayer> {
        let tile = self.tile()?;
        let layer = tile.get_layers().iter().position(|layer| layer.get_name() == name)?;
        Some(VectorMLTTileLayer { tile, layer })
    }

    /// `layerNames()`: in tile order (MVT: sorted), duplicates included.
    pub fn layer_names(&self) -> Vec<String> {
        let decode = {
            let state = self.state.lock_unpoisoned();
            state.data.as_ref().is_some_and(|data| !data.is_empty()) && state.tile.is_none()
        };
        let tile = if decode { self.tile() } else { self.state.lock_unpoisoned().tile.clone() };
        match tile {
            Some(tile) => tile.get_layers().iter().map(|layer| layer.get_name().to_string()).collect(),
            None => Vec::new(),
        }
    }
}

impl Clone for VectorMLTTileData {
    /// The C++ copy constructor: shares the data and the decoded tile.
    fn clone(&self) -> Self {
        VectorMLTTileData {
            state: Mutex::new(self.state.lock_unpoisoned().clone()),
            fast_pfor_enabled: self.fast_pfor_enabled,
        }
    }
}

impl GeometryTileData for VectorMLTTileData {
    fn clone_data(&self) -> Box<dyn GeometryTileData> {
        Box::new(self.clone())
    }

    fn get_layer(&self, name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, ExceptionPtr> {
        Ok(self.get_mlt_layer(name).map(|layer| Box::new(layer) as Box<dyn GeometryTileLayer + '_>))
    }

    /// The raw data, until the first decoding drops it (a tile sends its
    /// data to its worker before anything decodes it).
    fn encoded(&self) -> Option<EncodedTileData> {
        let data = self.state.lock_unpoisoned().data.clone()?;
        Some(EncodedTileData::Mlt { data, fast_pfor_enabled: self.fast_pfor_enabled })
    }
}
