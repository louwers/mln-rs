// Tiles are read with fast-mvt's lazy reader: a layer is decoded when it is
// asked for and a feature when it is read, and tags and geometry are read
// from the tile's bytes.
//
// Errors: a tile whose layers can't be found, or a layer that fast-mvt can't
// decode, fails `MvtTile::layer`; a feature it can't decode, or whose tags
// are malformed, fails `MvtLayer::feature`, so reading a value afterwards
// can't fail; a malformed geometry fails `MvtFeature::geometry`.

#[cfg(test)]
use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use fast_mvt::{MvtLazyFeature, MvtLazyLayer, MvtLazyReader, MvtResult, MvtValueRef};
use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_style::constants::EXTENT;
use mln_style::feature::GeometryCollection;
use mln_style::geometry::FeatureType;
use mln_style::hash::HashMap;

use crate::tile::TileError;
use crate::tile::geometry_tile_data::fixup_polygons;
use crate::tile::tile_data::FeatureGroups;

/// The MVT geometry types, as fast-mvt reads a feature's type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
enum MvtGeometryType {
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

impl MvtGeometryType {
    fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(MvtGeometryType::Point),
            2 => Some(MvtGeometryType::LineString),
            3 => Some(MvtGeometryType::Polygon),
            _ => None,
        }
    }
}

/// A layer of the tile: its name and its message, as ranges of the tile's
/// bytes.
struct LayerEntry {
    name: Range<usize>,
    data: Range<usize>,
}

struct MvtData {
    bytes: Arc<Vec<u8>>,
    /// The tile's layers in tile order, found on the first lookup.
    layers: OnceLock<Vec<LayerEntry>>,
}

/// Mapbox Vector Tile data. Cloning shares the bytes and the layers found in
/// them.
#[derive(Clone)]
pub struct MvtTile(Arc<MvtData>);

impl MvtTile {
    #[must_use]
    pub fn new(bytes: Arc<Vec<u8>>) -> Self {
        MvtTile(Arc::new(MvtData { bytes, layers: OnceLock::new() }))
    }

    /// The tile's bytes.
    #[must_use]
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.0.bytes
    }

    /// The tile's layers. Only a successful search is kept: a tile that
    /// fails is searched again next time.
    fn layers(&self) -> Result<&[LayerEntry], TileError> {
        if let Some(layers) = self.0.layers.get() {
            return Ok(layers);
        }
        let bytes = &self.0.bytes;
        let range = |slice: &[u8]| {
            let start = slice.as_ptr() as usize - bytes.as_ptr() as usize;
            start..start + slice.len()
        };
        let layers = MvtLazyReader::new(bytes)?
            .layers()
            .map(|layer| LayerEntry { name: range(layer.name().as_bytes()), data: range(layer.data()) })
            .collect();
        Ok(self.0.layers.get_or_init(|| layers))
    }

    fn find(&self, name: &str) -> Result<Option<&LayerEntry>, TileError> {
        let bytes = &self.0.bytes;
        Ok(self.layers()?.iter().find(|entry| bytes.get(entry.name.clone()) == Some(name.as_bytes())))
    }

    /// The first layer named `name`, decoded; `None` if there is none.
    pub(crate) fn layer(&self, name: &str) -> Result<Option<MvtLayer<'_>>, TileError> {
        let Some(data) = self.find(name)?.and_then(|entry| self.0.bytes.get(entry.data.clone())) else {
            return Ok(None);
        };
        Ok(Some(MvtLayer { layer: MvtLazyLayer::decode(data)? }))
    }

    /// Whether the tile has a layer named `name`, found without decoding it.
    pub(crate) fn has_layer(&self, name: &str) -> Result<bool, TileError> {
        Ok(self.find(name)?.is_some())
    }

    /// The names of the tile's layers, each once, in byte order.
    #[cfg(test)]
    pub(crate) fn layer_names(&self) -> Result<Vec<String>, TileError> {
        let bytes = &self.0.bytes;
        let names: BTreeSet<&[u8]> = self.layers()?.iter().filter_map(|entry| bytes.get(entry.name.clone())).collect();
        Ok(names.into_iter().map(|name| String::from_utf8_lossy(name).into_owned()).collect())
    }
}

/// A layer of an [`MvtTile`], decoded.
pub struct MvtLayer<'a> {
    layer: MvtLazyLayer<'a>,
}

impl MvtLayer<'_> {
    pub(crate) fn name(&self) -> &str {
        self.layer.name()
    }

    pub(crate) fn extent(&self) -> u32 {
        self.layer.extent()
    }

    pub(crate) fn len(&self) -> usize {
        self.layer.feature_count()
    }

    /// The feature at `i`, with its tags checked. Fails with
    /// [`TileError::OutOfRange`] past the end, and with fast-mvt's error for
    /// a feature it can't decode or a malformed tag.
    pub(crate) fn feature(&self, i: usize) -> Result<MvtFeature<'_>, TileError> {
        let feature = match self.layer.feature(i) {
            Some(feature) => feature?,
            None => return Err(TileError::vector_range_check(i, self.layer.feature_count())),
        };
        for property in feature.properties() {
            property?;
        }
        Ok(MvtFeature { layer: &self.layer, feature })
    }

    /// Groups by geometry type and by the value-table index of each key's
    /// first tag, as `get_value` reads it: equal indices are equal values.
    /// Equal values at distinct indices only split a group. `None` for a
    /// malformed feature, leaving its error to `feature`.
    pub(crate) fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        /// A key the feature has no tag for.
        const ABSENT: u32 = u32::MAX;
        let count = self.layer.feature_count();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let value_count = self.layer.values().len();
        let slots: Vec<Option<usize>> =
            (self.layer.keys().iter()).map(|key| keys.iter().position(|k| k == key)).collect();
        let mut index: HashMap<Box<[u32]>, u32> = HashMap::default();
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

/// A feature of an [`MvtLayer`], whose tags are known to be well formed.
pub struct MvtFeature<'a> {
    layer: &'a MvtLazyLayer<'a>,
    feature: MvtLazyFeature<'a, 'a>,
}

impl MvtFeature<'_> {
    /// The tags; [`MvtLayer::feature`] checked them.
    fn properties(&self) -> impl Iterator<Item = (&str, MvtValueRef<'_>)> {
        self.feature.properties().map_while(Result::ok)
    }

    pub(crate) fn get_type(&self) -> FeatureType {
        match self.feature.geom_type_value().and_then(MvtGeometryType::from_i32) {
            Some(MvtGeometryType::Point) => FeatureType::Point,
            Some(MvtGeometryType::LineString) => FeatureType::LineString,
            Some(MvtGeometryType::Polygon) => FeatureType::Polygon,
            None => FeatureType::Unknown,
        }
    }

    pub(crate) fn get_id(&self) -> Identifier {
        self.feature.id().map_or(Identifier::Null, Identifier::Uint)
    }

    /// The value of the first tag whose key is `key`; `None` for none or a
    /// null value.
    pub(crate) fn get_value(&self, key: &str) -> Option<Value> {
        let (_, value) = self.properties().find(|&(tag_key, _)| tag_key == key)?;
        match value {
            MvtValueRef::Null => None,
            value => Some(to_value(value)),
        }
    }

    /// Every property. A repeated key keeps its first value. The tags are
    /// sorted by key before they are copied, so that the map is built by
    /// appending and a repeated key is copied once.
    pub(crate) fn get_properties(&self) -> PropertyMap {
        // A feature seldom has a key twice: room for one tag per key.
        let mut tags = Vec::with_capacity(self.layer.keys().len());
        tags.extend(self.properties());
        // Stable, so the first of a repeated key stays first and is kept.
        tags.sort_by_key(|&(key, _)| key);
        tags.dedup_by(|(a, _), (b, _)| a == b);
        tags.into_iter().map(|(key, value)| (key.to_owned(), to_value(value))).collect()
    }

    /// The geometry, scaled to `util::EXTENT`. Version 1 polygons go
    /// through [`fixup_polygons`].
    pub(crate) fn geometry(&self) -> Result<GeometryCollection, TileError> {
        let scale = EXTENT as f32 / self.layer.extent() as f32;
        let lines = GeometryCollection::from(
            decode_geometry(self.feature.geometry_commands(), scale).map_err(TileError::Geometry)?,
        );
        Ok(
            if self.layer.version() < 2
                && self.feature.geom_type_value().and_then(MvtGeometryType::from_i32) == Some(MvtGeometryType::Polygon)
            {
                fixup_polygons(&lines)
            } else {
                lines
            },
        )
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
    #[derive(Clone, Copy, PartialEq, Eq)]
    #[repr(u32)]
    enum MvtCommand {
        MoveTo = 1,
        LineTo = 2,
        ClosePath = 7,
    }

    impl MvtCommand {
        fn from_u32(value: u32) -> Option<Self> {
            match value {
                1 => Some(MvtCommand::MoveTo),
                2 => Some(MvtCommand::LineTo),
                7 => Some(MvtCommand::ClosePath),
                _ => None,
            }
        }
    }

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
        match MvtCommand::from_u32(command & 0x7) {
            Some(id @ (MvtCommand::MoveTo | MvtCommand::LineTo)) => {
                if id == MvtCommand::LineTo {
                    // Room for the ClosePath of a polygon ring.
                    ring.reserve(count.min(MAX_RESERVE) + 1);
                }
                for _ in 0..count {
                    let (Some(dx), Some(dy)) = (next()?, next()?) else {
                        return Err("geometry command parameters past the end".to_owned());
                    };
                    if id == MvtCommand::MoveTo && !ring.is_empty() {
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
            Some(MvtCommand::ClosePath) => {
                if let Some(&first) = ring.first() {
                    ring.push(first);
                }
            }
            None => return Err("unknown command".to_owned()),
        }
    }
    rings.push(ring);
    Ok(rings)
}

fn zigzag(value: u32) -> i32 {
    (value >> 1) as i32 ^ -((value & 1) as i32)
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
