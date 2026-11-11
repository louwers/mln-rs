// Ports vendor/vector-tile/include/mapbox/vector_tile.hpp.

use std::collections::BTreeMap;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use protozero::{ConstVarintIterator, DataView, IteratorRange, PbfReader, decode_zigzag32};

use crate::config::{CommandType, FeatureType, GeomType, LayerType, TileType, ValueType};
use crate::error::{Error, at};

/// `point_type`: `mapbox::geometry::point<std::int16_t>`.
pub type PointType = Point<i16>;
/// `points_array_type`.
pub type PointsArrayType = Vec<PointType>;
/// `points_arrays_type`.
pub type PointsArraysType = Vec<PointsArrayType>;

/// `feature::properties_type` (`mapbox::feature::property_map`).
pub type PropertiesType = PropertyMap;

/// The `coordinate_type` of a `GeometryCollectionType` passed to
/// [`Feature::get_geometries`].
pub trait Coordinate: Copy {
    /// `static_cast<float>(std::numeric_limits<coordinate_type>::max())`.
    const MAX_COORD: f32;
    /// `static_cast<float>(std::numeric_limits<coordinate_type>::min())`.
    const MIN_COORD: f32;
    /// `static_cast<coordinate_type>(float)` of a rounded value that passed
    /// the range check (or NaN, which passes every check).
    fn from_f32(v: f32) -> Self;
}

impl Coordinate for i16 {
    const MAX_COORD: f32 = i16::MAX as f32;
    const MIN_COORD: f32 = i16::MIN as f32;
    fn from_f32(v: f32) -> Self {
        // NaN (a NaN or infinite `scale`): undefined in C++; x86-64 converts
        // with cvttss2si to int32 (0x80000000) and keeps the low 16 bits,
        // i.e. 0, which is also what Rust's `as` gives.
        v as i16
    }
}

/// `packed_iterator_type`.
type PackedIteratorType<'a> = IteratorRange<ConstVarintIterator<'a, u32>>;

/// `mapbox::vector_tile::feature`: a view of one feature of a [`Layer`].
#[derive(Clone, Debug)]
pub struct Feature<'l, 'a> {
    layer: &'l Layer<'a>,
    id: Identifier,
    type_: GeomType,
    tags_iter: PackedIteratorType<'a>,
    geometry_iter: PackedIteratorType<'a>,
}

/// `mapbox::vector_tile::layer`.
#[derive(Clone, Debug)]
pub struct Layer<'a> {
    name: Vec<u8>,
    version: u32,
    extent: u32,
    // C++ also has `std::map<std::string, uint32_t> keysMap`, which only
    // de-duplicates the storage of the key strings that `keys` refers to;
    // `keys` holds views of the same bytes.
    keys: Vec<DataView<'a>>,
    values: Vec<DataView<'a>>,
    features: Vec<DataView<'a>>,
}

/// `mapbox::vector_tile::buffer`: the layers of a tile by name.
#[derive(Clone, Debug, Default)]
pub struct Buffer<'a> {
    layers: BTreeMap<Vec<u8>, DataView<'a>>,
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `static mapbox::feature::value parseValue(protozero::data_view const&)`:
/// the last known field wins; no known field gives null.
pub fn parse_value(value_view: DataView<'_>) -> Result<Value, Error> {
    let mut value = Value::Null;
    let mut value_reader = PbfReader::new(value_view);
    while value_reader.next()? {
        match value_reader.tag() {
            ValueType::STRING => value = Value::String(value_reader.get_string()?),
            ValueType::FLOAT => value = Value::Double(f64::from(value_reader.get_float()?)),
            ValueType::DOUBLE => value = Value::Double(value_reader.get_double()?),
            ValueType::INT => value = Value::Int(value_reader.get_int64()?),
            ValueType::UINT => value = Value::Uint(value_reader.get_uint64()?),
            ValueType::SINT => value = Value::Int(value_reader.get_sint64()?),
            ValueType::BOOL => value = Value::Bool(value_reader.get_bool()?),
            _ => value_reader.skip()?,
        }
    }
    Ok(value)
}

impl<'l, 'a> Feature<'l, 'a> {
    /// `feature(protozero::data_view const&, layer const&)`. Repeated fields:
    /// the last one wins. The type is `static_cast<GeomType>(get_enum())`,
    /// i.e. the int32 truncated to 8 bits.
    pub fn new(feature_view: DataView<'a>, l: &'l Layer<'a>) -> Result<Self, Error> {
        let mut feature = Feature {
            layer: l,
            id: Identifier::Null,
            type_: GeomType::UNKNOWN,
            tags_iter: PackedIteratorType::default(),
            geometry_iter: PackedIteratorType::default(),
        };
        let mut feature_pbf = PbfReader::new(feature_view);
        while feature_pbf.next()? {
            match feature_pbf.tag() {
                FeatureType::ID => feature.id = Identifier::Uint(feature_pbf.get_uint64()?),
                FeatureType::TAGS => feature.tags_iter = feature_pbf.get_packed_uint32()?,
                FeatureType::TYPE => feature.type_ = GeomType(feature_pbf.get_enum()? as u8),
                FeatureType::GEOMETRY => feature.geometry_iter = feature_pbf.get_packed_uint32()?,
                _ => feature_pbf.skip()?,
            }
        }
        Ok(feature)
    }

    pub fn get_type(&self) -> GeomType {
        self.type_
    }

    /// `getValue(std::string const&)`: the value of the first tag pair whose
    /// key is `key`, or null. Throws for an odd number of tag ids, a value
    /// index out of range (checked before the key) and a key index out of
    /// range (`keys.at`).
    pub fn get_value(&self, key: &str) -> Result<Value, Error> {
        let values_count = self.layer.values.len();
        let mut start_itr = self.tags_iter.begin();
        let end_itr = self.tags_iter.end();
        while start_itr != end_itr {
            let tag_key: u32 = start_itr.post_increment()?.deref()?;

            if start_itr == end_itr {
                return Err(Error::Runtime("uneven number of feature tag ids".to_owned()));
            }

            let tag_val: u32 = start_itr.post_increment()?.deref()?;
            if values_count <= tag_val as usize {
                return Err(Error::Runtime("feature referenced out of range value".to_owned()));
            }

            if *at(&self.layer.keys, tag_key as usize)? == key.as_bytes() {
                return parse_value(self.layer.values[tag_val as usize]);
            }
        }

        Ok(Value::Null)
    }

    /// `getProperties()`. As in C++ the loop only runs when the packed tags
    /// contain at least one byte without the continuation bit
    /// (`std::distance` on varint iterators), and a repeated key keeps its
    /// first value (`unordered_map::emplace`). The `emplace` arguments are
    /// evaluated left to right, as clang does: `keys.at`, `values.at`,
    /// `parseValue`.
    pub fn get_properties(&self) -> Result<PropertiesType, Error> {
        let mut start_itr = self.tags_iter.begin();
        let end_itr = self.tags_iter.end();
        let mut properties = PropertiesType::new();
        let iter_len = self.tags_iter.size();
        if iter_len > 0 {
            while start_itr != end_itr {
                let tag_key: u32 = start_itr.post_increment()?.deref()?;
                if start_itr == end_itr {
                    return Err(Error::Runtime("uneven number of feature tag ids".to_owned()));
                }
                let tag_val: u32 = start_itr.post_increment()?.deref()?;
                let key = *at(&self.layer.keys, tag_key as usize)?;
                let value_view = *at(&self.layer.values, tag_val as usize)?;
                let value = parse_value(value_view)?;
                properties.entry(lossy(key)).or_insert(value);
            }
        }
        Ok(properties)
    }

    /// Port-only: calls `visit` with the key and value indices of each tag
    /// pair, in order. Throws if any pair would make `getValue` throw: an
    /// odd number of tag ids, or a key or value index out of range.
    pub fn for_each_tag(&self, mut visit: impl FnMut(u32, u32)) -> Result<(), Error> {
        let mut start_itr = self.tags_iter.begin();
        let end_itr = self.tags_iter.end();
        while start_itr != end_itr {
            let tag_key: u32 = start_itr.post_increment()?.deref()?;
            if start_itr == end_itr {
                return Err(Error::Runtime("uneven number of feature tag ids".to_owned()));
            }
            let tag_val: u32 = start_itr.post_increment()?.deref()?;
            if self.layer.values.len() <= tag_val as usize {
                return Err(Error::Runtime("feature referenced out of range value".to_owned()));
            }
            at(&self.layer.keys, tag_key as usize)?;
            visit(tag_key, tag_val);
        }
        Ok(())
    }

    pub fn get_id(&self) -> &Identifier {
        &self.id
    }

    pub fn get_extent(&self) -> u32 {
        self.layer.get_extent()
    }

    pub fn get_version(&self) -> u32 {
        self.layer.get_version()
    }

    /// `template <typename GeometryCollectionType> getGeometries(float scale)`:
    /// decodes the geometry commands into rings (one empty ring for no
    /// geometry), scaling each coordinate by `scale` with `roundf`.
    pub fn get_geometries<C: Coordinate>(&self, scale: f32) -> Result<Vec<Vec<Point<C>>>, Error> {
        let mut cmd: u8 = 1;
        let mut length: u32 = 0;
        let mut x: i64 = 0;
        let mut y: i64 = 0;

        let mut paths: Vec<Vec<Point<C>>> = Vec::new();

        paths.push(Vec::new());

        let mut start_itr = self.geometry_iter.begin();
        let end_itr = self.geometry_iter.end();
        let mut first = true;
        let mut len_reserve: u32 = 0;
        let mut extra_coords: usize = 0;
        if self.type_ == GeomType::LINESTRING {
            extra_coords = 1;
        } else if self.type_ == GeomType::POLYGON {
            extra_coords = 2;
        }
        let is_point = self.type_ == GeomType::POINT;

        while start_itr != end_itr {
            if length == 0 {
                let cmd_length: u32 = start_itr.post_increment()?.deref()?;
                cmd = (cmd_length & 0x7) as u8;
                length = cmd_length >> 3;
                len_reserve = length;
                // Prevents the creation of vector tiles that would cause
                // a denial of service from massive over allocation. Protection
                // limit is based on the assumption of an int64_t point which is
                // 16 bytes in size and wanting to have a maximum of 1 MB of memory
                // used.
                const MAX_LENGTH: u32 = (1024 * 1024) / 16;
                if len_reserve > MAX_LENGTH {
                    len_reserve = MAX_LENGTH;
                }
            }

            if cmd == CommandType::MOVE_TO || cmd == CommandType::LINE_TO {
                if length == 0 {
                    // If length is still equal to zero after the preceding step, this
                    // represents a command with an invalid command count, so we continue here to
                    // exit appropriately rather than underflow when we decrement length
                    continue;
                }

                length -= 1;

                if is_point {
                    if first && cmd == CommandType::MOVE_TO {
                        reserve_total(&mut paths, len_reserve as usize);
                        first = false;
                    }
                } else if first && cmd == CommandType::LINE_TO {
                    let back = paths.last_mut().expect("paths is never empty");
                    reserve_total(back, len_reserve as usize + extra_coords);
                    first = false;
                }

                if cmd == CommandType::MOVE_TO && !paths.last().expect("paths is never empty").is_empty() {
                    let back = paths.last_mut().expect("paths is never empty");
                    if back.len() < back.capacity() {
                        // Assuming we had an invalid length before
                        // lets shrink to fit, just to make sure
                        // we don't have a large capacity vector
                        // just wasting memory
                        back.shrink_to_fit();
                    }
                    paths.push(Vec::new());
                    if !is_point {
                        first = true;
                    }
                }

                x = x.wrapping_add(i64::from(decode_zigzag32(start_itr.post_increment()?.deref()?)));
                y = y.wrapping_add(i64::from(decode_zigzag32(start_itr.post_increment()?.deref()?)));
                let px = (x as f32 * scale).round();
                let py = (y as f32 * scale).round();
                let max_coord = C::MAX_COORD;
                let min_coord = C::MIN_COORD;

                if px > max_coord || px < min_coord || py > max_coord || py < min_coord {
                    return Err(Error::Runtime("paths outside valid range of coordinate_type".to_owned()));
                } else {
                    paths.last_mut().expect("paths is never empty").push(Point::new(C::from_f32(px), C::from_f32(py)));
                }
            } else if cmd == CommandType::CLOSE {
                let back = paths.last_mut().expect("paths is never empty");
                if !back.is_empty() {
                    let p = back[0];
                    back.push(p);
                }
                length = 0;
            } else {
                return Err(Error::Runtime("unknown command".to_owned()));
            }
        }
        if paths.len() < paths.capacity() {
            // Assuming we had an invalid length before
            // lets shrink to fit, just to make sure
            // we don't have a large capacity vector
            // just wasting memory
            paths.shrink_to_fit();
        }
        Ok(paths)
    }
}

/// `std::vector::reserve(n)`: at least `n` in total.
fn reserve_total<T>(v: &mut Vec<T>, n: usize) {
    if n > v.capacity() {
        v.reserve_exact(n - v.len());
    }
}

impl<'a> Buffer<'a> {
    /// `buffer(std::string const&)`: indexes the layers (field 3) by name;
    /// the first layer of a name wins. Throws "Layer missing name" for a
    /// layer without a name field (the last name field of a layer counts).
    pub fn new(data: DataView<'a>) -> Result<Self, Error> {
        let mut layers = BTreeMap::new();
        let mut data_reader = PbfReader::new(data);
        while data_reader.next_tag(TileType::LAYERS)? {
            let layer_view = data_reader.get_view()?;
            let mut layer_reader = PbfReader::new(layer_view);
            let mut name: Vec<u8> = Vec::new();
            let mut has_name = false;
            while layer_reader.next_tag(LayerType::NAME)? {
                name = layer_reader.get_view()?.to_vec();
                has_name = true;
            }
            if !has_name {
                return Err(Error::Runtime("Layer missing name".to_owned()));
            }
            layers.entry(name).or_insert(layer_view);
        }
        Ok(Buffer { layers })
    }

    /// `layerNames()`: in byte order of the names (std::map order).
    pub fn layer_names(&self) -> Vec<String> {
        self.layers.keys().map(|name| lossy(name)).collect()
    }

    /// `getLayers()`: name bytes → layer view.
    pub fn get_layers(&self) -> BTreeMap<Vec<u8>, DataView<'a>> {
        self.layers.clone()
    }

    /// `getLayer(const std::string&)`.
    pub fn get_layer(&self, name: &str) -> Result<Layer<'a>, Error> {
        match self.layers.get(name.as_bytes()) {
            Some(view) => Layer::new(view),
            None => Err(Error::Runtime(format!("no layer by the name of '{name}'"))),
        }
    }
}

impl<'a> Layer<'a> {
    /// `layer(protozero::data_view const&)`. Defaults: version 1, extent
    /// 4096; but version, extent and name are all required.
    pub fn new(layer_view: DataView<'a>) -> Result<Self, Error> {
        let mut layer = Layer {
            name: Vec::new(),
            version: 1,
            extent: 4096,
            keys: Vec::new(),
            values: Vec::new(),
            features: Vec::new(),
        };
        let mut has_name = false;
        let mut has_extent = false;
        let mut has_version = false;
        let mut layer_pbf = PbfReader::new(layer_view);
        while layer_pbf.next()? {
            match layer_pbf.tag() {
                LayerType::NAME => {
                    layer.name = layer_pbf.get_view()?.to_vec();
                    has_name = true;
                }
                LayerType::FEATURES => {
                    layer.features.push(layer_pbf.get_view()?);
                }
                LayerType::KEYS => {
                    // We want to keep the keys in the order of the vector tile
                    // https://github.com/mapbox/mapbox-gl-native/pull/5183
                    layer.keys.push(layer_pbf.get_view()?);
                }
                LayerType::VALUES => {
                    layer.values.push(layer_pbf.get_view()?);
                }
                LayerType::EXTENT => {
                    layer.extent = layer_pbf.get_uint32()?;
                    has_extent = true;
                }
                LayerType::VERSION => {
                    layer.version = layer_pbf.get_uint32()?;
                    has_version = true;
                }
                _ => {
                    layer_pbf.skip()?;
                }
            }
        }
        if !has_version || !has_name || !has_extent {
            let mut msg = String::from("missing required field:");
            if !has_version {
                msg += " version ";
            }
            if !has_extent {
                msg += " extent ";
            }
            if !has_name {
                msg += " name";
            }
            return Err(Error::Runtime(msg));
        }
        Ok(layer)
    }

    pub fn feature_count(&self) -> usize {
        self.features.len()
    }

    /// `getFeature(std::size_t)`: `features.at(i)`.
    pub fn get_feature(&self, i: usize) -> Result<DataView<'a>, Error> {
        at(&self.features, i).copied()
    }

    /// `getName()` (UTF-8-lossy; see the crate docs).
    pub fn get_name(&self) -> String {
        lossy(&self.name)
    }

    /// Port-only: the key table, which tags index.
    pub fn keys(&self) -> &[DataView<'a>] {
        &self.keys
    }

    /// The name as the bytes of the tile.
    pub fn get_name_bytes(&self) -> &[u8] {
        &self.name
    }

    pub fn get_extent(&self) -> u32 {
        self.extent
    }

    pub fn get_version(&self) -> u32 {
        self.version
    }
}
