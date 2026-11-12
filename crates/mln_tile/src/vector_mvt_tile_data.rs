// Ports src/mln/tile/vector_mvt_tile_data.hpp and
// src/mln/tile/vector_mvt_tile_data.cpp, reading the tiles with fast-mvt
// where the C++ uses mapbox::vector_tile.
//
// Errors: fast-mvt decodes a whole tile at once, so a tile it can't decode
// is a `get_layer` error. A feature's tags can still be malformed:
// `try_get_value` and `try_get_properties` return the error, and the
// trait's `get_value` and `get_properties` return null or empty and keep it
// for `GeometryTileFeature::error` (the trait has no other channel).

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

use fast_mvt::{MvtFeatureRef, MvtLayerRef, MvtReaderRef, MvtValueRef};
use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_tile_id::{
    EncodedTileData, FeatureGroups, GeometryCollection, GeometryTileData, GeometryTileFeature, GeometryTileLayer,
};
use mln_util::constants::EXTENT;
use mln_util::error::Error as MlnError;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};
use yoke::{Yoke, Yokeable};

use crate::Error;
use crate::geometry_tile_data::fixup_polygons;

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();
static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

/// The MVT geometry types, as fast-mvt reads a feature's type.
const POINT: i32 = 1;
const LINESTRING: i32 = 2;
const POLYGON: i32 = 3;

/// `VectorMVTTileFeature`.
pub struct VectorMVTTileFeature<'l> {
    layer: MvtLayerRef<'l>,
    feature: MvtFeatureRef<'l>,
    lines: OnceCell<GeometryCollection>,
    properties: OnceCell<PropertyMap>,
    /// The first error of an accessor (`GeometryTileFeature::error`).
    error: OnceCell<MlnError>,
}

impl<'l> VectorMVTTileFeature<'l> {
    fn new(layer: MvtLayerRef<'l>, feature: MvtFeatureRef<'l>) -> Self {
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

    /// `getValue`: the value of the first tag whose key is `key`; `None`
    /// for none or a null value. Errors: a malformed tag before it.
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

    /// `getProperties` (computed once; an error is returned again by the
    /// next call). A repeated key keeps its first value, as in C++.
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

    /// `getGeometries`: scaled to `util::EXTENT`. A malformed geometry is
    /// logged and gives an empty collection. Version 1 polygons go through
    /// `fixupPolygons`; if that fails, the unfixed lines are kept for the
    /// next call, as in C++.
    pub fn try_get_geometries(&self) -> Result<&GeometryCollection, Error> {
        if let Some(lines) = self.lines.get() {
            return Ok(lines);
        }

        let scale = EXTENT as f32 / self.layer.extent() as f32;

        let mut lines = match decode_geometry(self.feature.geometry_commands(), scale) {
            Ok(rings) => GeometryCollection::from(rings),
            Err(what) => {
                Log::error(Event::ParseTile, format!("Could not get geometries: {what}"));
                GeometryCollection::new()
            }
        };

        if self.layer.version() < 2 && self.feature.geom_type_value() == Some(POLYGON) {
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

    fn get_geometries(&self) -> &GeometryCollection {
        match self.try_get_geometries() {
            Ok(lines) => lines,
            Err(e) => {
                self.keep_error(&e);
                &EMPTY_GEOMETRIES
            }
        }
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
/// of a multipoint is a ring of its own), a `ClosePath` repeats the ring's first
/// point, and no commands give one empty ring. The error is C++'s
/// `std::runtime_error` text.
fn decode_geometry(commands: &[u32], scale: f32) -> Result<Vec<Vec<Point<i16>>>, &'static str> {
    const MOVE_TO: u32 = 1;
    const LINE_TO: u32 = 2;
    const CLOSE_PATH: u32 = 7;

    let (min, max) = (f32::from(i16::MIN), f32::from(i16::MAX));
    let mut rings = Vec::new();
    let mut ring: Vec<Point<i16>> = Vec::new();
    let (mut x, mut y) = (0i64, 0i64);
    let mut rest = commands;
    while let Some((&command, after)) = rest.split_first() {
        rest = after;
        let count = (command >> 3) as usize;
        match command & 0x7 {
            id @ (MOVE_TO | LINE_TO) => {
                // At most 2^30 parameters: no overflow, even with a 32-bit usize.
                let Some((parameters, after)) = rest.split_at_checked(count * 2) else {
                    return Err("geometry command parameters past the end");
                };
                rest = after;
                if id == LINE_TO {
                    // Room for the ClosePath of a polygon ring.
                    ring.reserve(count + 1);
                }
                for &[dx, dy] in parameters.as_chunks::<2>().0 {
                    if id == MOVE_TO && !ring.is_empty() {
                        rings.push(std::mem::take(&mut ring));
                    }
                    x = x.wrapping_add(i64::from(zigzag(dx)));
                    y = y.wrapping_add(i64::from(zigzag(dy)));
                    let px = (x as f32 * scale).round();
                    let py = (y as f32 * scale).round();
                    if px > max || px < min || py > max || py < min {
                        return Err("paths outside valid range of coordinate_type");
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
            _ => return Err("unknown command"),
        }
    }
    rings.push(ring);
    Ok(rings)
}

fn zigzag(value: u32) -> i32 {
    (value >> 1) as i32 ^ -((value & 1) as i32)
}

/// `VectorMVTTileLayer`: a layer of a [`VectorMVTTileData`]'s decoded tile.
pub struct VectorMVTTileLayer<'d> {
    layer: MvtLayerRef<'d>,
    /// fast-mvt iterates a layer's features; the trait indexes them.
    features: Vec<MvtFeatureRef<'d>>,
}

impl<'d> VectorMVTTileLayer<'d> {
    fn new(layer: MvtLayerRef<'d>) -> Self {
        VectorMVTTileLayer { layer, features: layer.features().collect() }
    }

    /// `getFeature(i)`: `std::out_of_range` past the end.
    pub fn try_get_feature(&self, i: usize) -> Result<VectorMVTTileFeature<'d>, Error> {
        let feature = self.features.get(i).ok_or_else(|| Error::vector_range_check(i, self.features.len()))?;
        Ok(VectorMVTTileFeature::new(self.layer, *feature))
    }
}

impl GeometryTileLayer for VectorMVTTileLayer<'_> {
    fn feature_count(&self) -> usize {
        self.features.len()
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
        let count = self.features.len();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let value_count = self.layer.values().len();
        let slots: Vec<Option<usize>> =
            (self.layer.keys().iter()).map(|key| keys.iter().position(|k| k == key)).collect();
        let mut index: HashMap<Box<[u32]>, u32> = HashMap::new();
        let mut codes = vec![ABSENT; keys.len() + 1];
        let mut groups = FeatureGroups { of_feature: Vec::with_capacity(count), first: Vec::new() };
        for (i, feature) in self.features.iter().enumerate() {
            codes.fill(ABSENT);
            codes[keys.len()] = feature.geom_type_value().unwrap_or(0) as u32;
            let tags = feature.tags();
            if tags.len() % 2 != 0 {
                return None;
            }
            for &[key, value] in tags.as_chunks::<2>().0 {
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

/// A tile as fast-mvt decodes it, borrowing the tile's bytes.
#[derive(Yokeable)]
struct DecodedTile<'a>(MvtReaderRef<'a>);

/// `VectorMVTTileData`: the tile is decoded lazily, on the first
/// `getLayer`, so the object can be made on the main thread cheaply.
pub struct VectorMVTTileData {
    /// C++ `std::shared_ptr<const std::string>`.
    data: Arc<Vec<u8>>,
    /// The decoded tile, which borrows `data`: the yoke holds its own
    /// reference to it. `OnceLock` (not `OnceCell`): the tile data is shared
    /// with the tile worker thread (`mln_renderer`'s
    /// `Arc<dyn GeometryTileData + Send + Sync>`).
    tile: OnceLock<Yoke<DecodedTile<'static>, Arc<Vec<u8>>>>,
}

impl VectorMVTTileData {
    #[must_use]
    pub fn new(data: Arc<Vec<u8>>) -> Self {
        VectorMVTTileData { data, tile: OnceLock::new() }
    }

    pub fn data(&self) -> &Arc<Vec<u8>> {
        &self.data
    }

    /// The decoded tile. Only a successful decode is kept, as C++ sets
    /// `parsed` after it: a tile that fails is decoded again next time.
    fn tile(&self) -> Result<&MvtReaderRef<'_>, Error> {
        if let Some(tile) = self.tile.get() {
            return Ok(&tile.get().0);
        }
        let tile =
            Yoke::try_attach_to_cart(self.data.clone(), |data: &Vec<u8>| MvtReaderRef::new(data).map(DecodedTile))?;
        Ok(&self.tile.get_or_init(|| tile).get().0)
    }

    /// `getLayer`: the first layer named `name`, `None` for none. Errors:
    /// the tile's decoding.
    pub fn try_get_layer(&self, name: &str) -> Result<Option<VectorMVTTileLayer<'_>>, Error> {
        Ok(self.tile()?.layers().find(|layer| layer.name() == name).map(VectorMVTTileLayer::new))
    }

    /// `layerNames()`: each name once, in byte order (C++ lists the keys of
    /// a `std::map`).
    pub fn layer_names(&self) -> Result<Vec<String>, Error> {
        let names: BTreeSet<&str> = self.tile()?.layers().map(MvtLayerRef::name).collect();
        Ok(names.into_iter().map(str::to_owned).collect())
    }
}

impl GeometryTileData for VectorMVTTileData {
    /// `clone()`: shares the data, not the decoded tile.
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
        let rings = decode_geometry(commands, scale).unwrap();
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
        assert_eq!(decode_geometry(&[3], 1.0), Err("unknown command"));
        assert_eq!(decode_geometry(&[17, 2, 4, 2], 1.0), Err("geometry command parameters past the end"));
        assert_eq!(decode_geometry(&[9, 2, 0], 40000.0), Err("paths outside valid range of coordinate_type"));
    }
}
