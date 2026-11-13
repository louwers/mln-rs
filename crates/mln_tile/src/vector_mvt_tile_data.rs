// Tiles are read with fast-mvt's lazy reader, where C++ uses
// `mapbox::vector_tile`. Like that, the lazy reader decodes a layer when it
// is asked for and a feature when it is read, and reads tags and geometry
// from the tile's bytes.
//
// Errors: a tile whose layers can't be found, or a layer that fast-mvt
// can't decode, is a `get_layer` error, and a feature it can't decode a
// `get_feature` error. A feature's tags can still be malformed:
// `try_get_value` and `try_get_properties` return the error, and the
// trait's `get_value` and `get_properties` return null or empty and keep it
// for `GeometryTileFeature::error` (the trait has no other channel).

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use fast_mvt::{MvtLazyFeature, MvtLazyLayer, MvtLazyReader, MvtResult, MvtValueRef};
use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_tile_id::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryTileData, GeometryTileFeature, GeometryTileLayer,
};
use mln_util::constants::EXTENT;
use mln_util::error::Error as MlnError;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};

use crate::Error;
use crate::geometry_tile_data::fixup_polygons;

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();

/// The MVT geometry types, as fast-mvt reads a feature's type.
const POINT: i32 = 1;
const LINESTRING: i32 = 2;
const POLYGON: i32 = 3;

pub struct VectorMVTTileFeature<'l> {
    layer: &'l MvtLazyLayer<'l>,
    feature: MvtLazyFeature<'l, 'l>,
    lines: OnceCell<GeometryCollection>,
    properties: OnceCell<PropertyMap>,
    /// The first error of an accessor (`GeometryTileFeature::error`).
    error: OnceCell<MlnError>,
}

impl<'l> VectorMVTTileFeature<'l> {
    fn new(layer: &'l MvtLazyLayer<'l>, feature: MvtLazyFeature<'l, 'l>) -> Self {
        VectorMVTTileFeature {
            layer,
            feature,
            lines: OnceCell::new(),
            properties: OnceCell::new(),
            error: OnceCell::new(),
        }
    }

    /// Keeps the first error of a trait accessor.
    fn keep_error(&self, error: &Error) {
        let _ = self.error.set(MlnError::native(error.to_string()));
    }

    /// The value of the first tag whose key is `key`; `None` for none or a
    /// null value. Fails for a malformed tag before it.
    pub fn try_get_value(&self, key: &str) -> Result<Option<Value>, Error> {
        for property in self.feature.properties() {
            let (tag_key, value) = property?;
            if tag_key == key {
                return Ok(match value {
                    MvtValueRef::Null => None,
                    value => Some(to_value(value)),
                });
            }
        }
        Ok(None)
    }

    /// Every property, computed once; an error is returned again by the next
    /// call. A repeated key keeps its first value, as in C++.
    pub fn try_get_properties(&self) -> Result<&PropertyMap, Error> {
        if let Some(properties) = self.properties.get() {
            return Ok(properties);
        }
        let mut properties = PropertyMap::new();
        for property in self.feature.properties() {
            let (key, value) = property?;
            properties.entry(key.to_owned()).or_insert_with(|| to_value(value));
        }
        Ok(self.properties.get_or_init(|| properties))
    }
}

impl GeometryTileFeature for VectorMVTTileFeature<'_> {
    fn get_type(&self) -> FeatureType {
        match self.feature.geom_type_value() {
            Some(POINT) => FeatureType::Point,
            Some(LINESTRING) => FeatureType::LineString,
            Some(POLYGON) => FeatureType::Polygon,
            _ => FeatureType::Unknown,
        }
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.try_get_value(key).unwrap_or_else(|e| {
            self.keep_error(&e);
            None
        })
    }

    fn get_properties(&self) -> &PropertyMap {
        match self.try_get_properties() {
            Ok(properties) => properties,
            Err(e) => {
                self.keep_error(&e);
                &EMPTY_PROPERTIES
            }
        }
    }

    fn get_id(&self) -> Identifier {
        self.feature.id().map_or(Identifier::Null, Identifier::Uint)
    }

    /// Scaled to `util::EXTENT`. A malformed geometry is logged and gives
    /// an empty collection. Version 1 polygons go through
    /// [`fixup_polygons`].
    fn get_geometries(&self) -> &GeometryCollection {
        self.lines.get_or_init(|| {
            let scale = EXTENT as f32 / self.layer.extent() as f32;
            let lines = match decode_geometry(self.feature.geometry_commands(), scale) {
                Ok(rings) => GeometryCollection::from(rings),
                Err(what) => {
                    Log::error(Event::ParseTile, format!("Could not get geometries: {what}"));
                    return GeometryCollection::new();
                }
            };
            if self.layer.version() < 2 && self.feature.geom_type_value() == Some(POLYGON) {
                fixup_polygons(&lines)
            } else {
                lines
            }
        })
    }

    fn error(&self) -> Option<MlnError> {
        self.error.get().cloned()
    }
}

fn to_value(value: MvtValueRef<'_>) -> Value {
    match value {
        MvtValueRef::String(value) => Value::String(value.to_owned()),
        MvtValueRef::Float(value) => Value::Double(f64::from(value)),
        MvtValueRef::Double(value) => Value::Double(value),
        MvtValueRef::Int(value) | MvtValueRef::SInt(value) => Value::Int(value),
        MvtValueRef::UInt(value) => Value::Uint(value),
        MvtValueRef::Bool(value) => Value::Bool(value),
        MvtValueRef::Null => Value::Null,
    }
}

/// Decodes a feature's geometry commands into rings, scaling each
/// coordinate by `scale` and rounding it, as `mapbox::vector_tile`'s
/// `feature::getGeometries` does. A `MoveTo` starts a new ring (each point
/// of a multipoint is a ring of its own), a `ClosePath` repeats the ring's
/// first point, and no commands give one empty ring. The errors are C++'s
/// `std::runtime_error` texts, or fast-mvt's for malformed commands.
fn decode_geometry(
    mut commands: impl Iterator<Item = MvtResult<u32>>,
    scale: f32,
) -> Result<Vec<Vec<Point<i16>>>, String> {
    const MOVE_TO: u32 = 1;
    const LINE_TO: u32 = 2;
    const CLOSE_PATH: u32 = 7;
    /// The most points a command reserves room for: its count comes from
    /// the tile, unchecked (C++ caps it the same way).
    const MAX_RESERVE: usize = 1024 * 1024 / 16;

    let mut next = || commands.next().transpose().map_err(|e| e.to_string());
    let (min, max) = (f32::from(i16::MIN), f32::from(i16::MAX));
    let mut rings = Vec::new();
    let mut ring: Vec<Point<i16>> = Vec::new();
    let (mut x, mut y) = (0i64, 0i64);
    while let Some(command) = next()? {
        let count = (command >> 3) as usize;
        match command & 0x7 {
            id @ (MOVE_TO | LINE_TO) => {
                if id == LINE_TO {
                    // Room for the ClosePath of a polygon ring.
                    ring.reserve(count.min(MAX_RESERVE) + 1);
                }
                for _ in 0..count {
                    let (Some(dx), Some(dy)) = (next()?, next()?) else {
                        return Err("geometry command parameters past the end".to_owned());
                    };
                    if id == MOVE_TO && !ring.is_empty() {
                        rings.push(std::mem::take(&mut ring));
                    }
                    x = x.wrapping_add(i64::from(zigzag(dx)));
                    y = y.wrapping_add(i64::from(zigzag(dy)));
                    let px = (x as f32 * scale).round();
                    let py = (y as f32 * scale).round();
                    if px > max || px < min || py > max || py < min {
                        return Err("paths outside valid range of coordinate_type".to_owned());
                    }
                    // A NaN scale passes the range check and becomes 0.
                    ring.push(Point::new(px as i16, py as i16));
                }
            }
            CLOSE_PATH => {
                if let Some(&first) = ring.first() {
                    ring.push(first);
                }
            }
            _ => return Err("unknown command".to_owned()),
        }
    }
    rings.push(ring);
    Ok(rings)
}

fn zigzag(value: u32) -> i32 {
    (value >> 1) as i32 ^ -((value & 1) as i32)
}

/// A layer of a [`VectorMVTTileData`], decoded.
pub struct VectorMVTTileLayer<'d> {
    layer: MvtLazyLayer<'d>,
}

impl VectorMVTTileLayer<'_> {
    /// The feature at `i`. Fails with [`Error::OutOfRange`] past the end, as
    /// C++ throws `std::out_of_range`, and with fast-mvt's error for a
    /// feature it can't decode.
    pub fn try_get_feature(&self, i: usize) -> Result<VectorMVTTileFeature<'_>, Error> {
        match self.layer.feature(i) {
            Some(feature) => Ok(VectorMVTTileFeature::new(&self.layer, feature?)),
            None => Err(Error::vector_range_check(i, self.layer.feature_count())),
        }
    }
}

impl GeometryTileLayer for VectorMVTTileLayer<'_> {
    fn feature_count(&self) -> usize {
        self.layer.feature_count()
    }

    fn get_feature(&self, index: usize) -> Result<Box<dyn GeometryTileFeature + '_>, MlnError> {
        match self.try_get_feature(index) {
            Ok(feature) => Ok(Box::new(feature)),
            Err(e) => Err(MlnError::native(e.to_string())),
        }
    }

    fn get_name(&self) -> String {
        self.layer.name().to_owned()
    }

    /// Groups by geometry type and by the value-table index of each key's
    /// first tag, as `getValue` reads it: equal indices are equal values.
    /// Equal values at distinct indices only split a group. `None` for a
    /// malformed feature, leaving its errors to the accessors.
    fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        /// A key the feature has no tag for.
        const ABSENT: u32 = u32::MAX;
        let count = self.layer.feature_count();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let value_count = self.layer.values().len();
        let slots: Vec<Option<usize>> =
            (self.layer.keys().iter()).map(|key| keys.iter().position(|k| k == key)).collect();
        let mut index: HashMap<Box<[u32]>, u32> = HashMap::new();
        let mut codes = vec![ABSENT; keys.len() + 1];
        let mut groups = FeatureGroups { of_feature: Vec::with_capacity(count), first: Vec::new() };
        for (i, feature) in self.layer.features().enumerate() {
            let feature = feature.ok()?;
            codes.fill(ABSENT);
            codes[keys.len()] = feature.geom_type_value().unwrap_or(0) as u32;
            let mut tags = feature.tags();
            while let Some(key) = tags.next() {
                let (key, value) = (key.ok()?, tags.next()?.ok()?);
                let slot = *slots.get(key as usize)?;
                if value as usize >= value_count {
                    return None;
                }
                if let Some(slot) = slot
                    && codes[slot] == ABSENT
                {
                    codes[slot] = value;
                }
            }
            let group = if let Some(&group) = index.get(codes.as_slice()) {
                group
            } else {
                if groups.first.len() == limit {
                    return None;
                }
                let group = groups.first.len() as u32;
                index.insert(codes.as_slice().into(), group);
                groups.first.push(i);
                group
            };
            groups.of_feature.push(group);
        }
        Some(groups)
    }
}

/// A layer of the tile: its name and its message, as ranges of the tile's
/// bytes.
struct LayerEntry {
    name: Range<usize>,
    data: Range<usize>,
}

/// Mapbox Vector Tile data. The layers are found lazily, on the first layer
/// lookup, so the object can be made on the main thread cheaply.
pub struct VectorMVTTileData {
    data: Arc<Vec<u8>>,
    /// The tile's layers in tile order. `OnceLock` (not `OnceCell`): the
    /// tile data is shared with the tile worker thread (`mln_renderer`'s
    /// `Arc<dyn GeometryTileData + Send + Sync>`).
    layers: OnceLock<Vec<LayerEntry>>,
}

impl VectorMVTTileData {
    #[must_use]
    pub fn new(data: Arc<Vec<u8>>) -> Self {
        VectorMVTTileData { data, layers: OnceLock::new() }
    }

    pub fn data(&self) -> &Arc<Vec<u8>> {
        &self.data
    }

    /// The tile's layers. Only a successful search is kept, as C++ sets
    /// `parsed` after it: a tile that fails is searched again next time.
    fn layers(&self) -> Result<&[LayerEntry], Error> {
        if let Some(layers) = self.layers.get() {
            return Ok(layers);
        }
        let range = |bytes: &[u8]| {
            let start = bytes.as_ptr() as usize - self.data.as_ptr() as usize;
            start..start + bytes.len()
        };
        let layers = MvtLazyReader::new(&self.data)?
            .layers()
            .map(|layer| LayerEntry { name: range(layer.name().as_bytes()), data: range(layer.data()) })
            .collect();
        Ok(self.layers.get_or_init(|| layers))
    }

    /// The first layer named `name`, decoded; `None` if there is none.
    /// Fails if the tile's layers can't be found or the layer can't be
    /// decoded.
    pub fn try_get_layer(&self, name: &str) -> Result<Option<VectorMVTTileLayer<'_>>, Error> {
        let entry = self.layers()?.iter().find(|entry| self.data.get(entry.name.clone()) == Some(name.as_bytes()));
        let Some(data) = entry.and_then(|entry| self.data.get(entry.data.clone())) else {
            return Ok(None);
        };
        Ok(Some(VectorMVTTileLayer { layer: MvtLazyLayer::decode(data)? }))
    }

    /// The names of the tile's layers, each once, in byte order (C++ lists
    /// the keys of a `std::map`).
    pub fn layer_names(&self) -> Result<Vec<String>, Error> {
        let names: BTreeSet<&[u8]> =
            self.layers()?.iter().filter_map(|entry| self.data.get(entry.name.clone())).collect();
        Ok(names.into_iter().map(|name| String::from_utf8_lossy(name).into_owned()).collect())
    }
}

impl GeometryTileData for VectorMVTTileData {
    /// Shares the data, not the layers found in it.
    fn clone_data(&self) -> Box<dyn GeometryTileData> {
        Box::new(VectorMVTTileData::new(self.data.clone()))
    }

    fn get_layer(&self, name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, MlnError> {
        match self.try_get_layer(name) {
            Ok(layer) => Ok(layer.map(|layer| Box::new(layer) as Box<dyn GeometryTileLayer + '_>)),
            Err(e) => Err(MlnError::native(e.to_string())),
        }
    }

    fn encoded(&self) -> Option<EncodedTileData> {
        Some(EncodedTileData::Mvt(self.data.clone()))
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;

    use super::decode_geometry;

    fn rings(commands: &[u32], scale: f32) -> Vec<Vec<(i16, i16)>> {
        let rings = decode_geometry(commands.iter().copied().map(Ok), scale).unwrap();
        rings.iter().map(|ring| ring.iter().map(|p: &Point<i16>| (p.x, p.y)).collect()).collect()
    }

    #[test]
    fn decodes_rings() {
        // No commands: one empty ring.
        assert_eq!(rings(&[], 1.0), vec![vec![]]);
        // MoveTo(2): each point is a ring.
        assert_eq!(rings(&[17, 2, 4, 2, 2], 1.0), vec![vec![(1, 2)], vec![(2, 3)]]);
        // MoveTo, LineTo(2), MoveTo, LineTo(1): two lines.
        assert_eq!(
            rings(&[9, 0, 0, 18, 2, 0, 0, 2, 9, 2, 2, 10, 1, 1], 1.0),
            vec![vec![(0, 0), (1, 0), (1, 1)], vec![(2, 2), (1, 1)]]
        );
        // ClosePath repeats the first point; the scale rounds.
        assert_eq!(rings(&[9, 2, 2, 18, 2, 0, 0, 2, 15], 2.5), vec![vec![(3, 3), (5, 3), (5, 5), (3, 3)]]);
    }

    #[test]
    fn rejects_malformed_commands() {
        let error = |commands: &[u32], scale| decode_geometry(commands.iter().copied().map(Ok), scale).unwrap_err();
        assert_eq!(error(&[3], 1.0), "unknown command");
        assert_eq!(error(&[17, 2, 4, 2], 1.0), "geometry command parameters past the end");
        assert_eq!(error(&[9, 2, 0], 40000.0), "paths outside valid range of coordinate_type");
    }
}
