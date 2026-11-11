// Ports the output model of the C++ MLT decoder that
// src/mln/tile/vector_mlt_tile_data.cpp reads (vendor/maplibre-tile-spec/cpp:
// include/mlt/{tile,layer,feature,geometry,coordinate,properties}.hpp,
// src/mlt/{feature,properties}.cpp, the per-feature topology walk of
// src/mlt/geometry_vector.cpp and the checks of src/mlt/decoder.cpp).
//
// The wire format is decoded by the `mlt-core` crate, not by a port of the
// C++ decoder (about 70k lines), and is assumed correct. This module turns mlt-core's
// columnar result into the C++ model, with the C++ behaviour that
// VectorMLTTileData can observe:
//
// - a layer with an unknown tag is skipped; any error fails the whole tile;
// - with `supportFastPFOR == false`, an integer stream with the FastPFOR
//   physical technique fails the tile ("FastPFOR decoding is not enabled");
// - coordinates are `float`s converted from the int32 vertices;
// - a polygon ring gets its first vertex appended only when the first and
//   last differ (compared as floats);
// - an empty line or ring at the start of the vertex buffer fails the tile
//   ("vertexBuffer underflow": the C++ bounds check wraps);
// - a multi-point without points is dropped from the layer (the C++ adds no
//   geometry for it), which shifts the property index of later features;
//   an id column whose length then differs fails the tile;
// - the triangles of a tessellated (multi-)polygon are its slice of the index
//   buffer, counted from the feature's first vertex as the tile stores them
//   (mlt-core rebases them to the layer; this undoes that);
// - property columns keep their presence: a value of an optional column is
//   absent past its end, and a non-optional column shorter than the layer
//   throws "Missing property value" when read (PresentProperties::getProperty);
// - 8-bit integer columns fail the tile; strings of string columns lose a
//   leading UTF-8 byte order mark (StringDecoder::view), those of shared
//   dictionaries keep it; of two columns with the same name the first wins
//   (std::unordered_map::insert).
//
// Where mlt-core rejects a tile that the C++ decoder reads (or the other way
// round), the tile fails here too.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use mlt_core::wire::{Analyze as _, ColumnDecl, LogicalEncoding, PhysicalEncoding};
use mlt_core::{GeometryType as WireGeometryType, GeometryValues, ParsedLayer01, ParsedProperty};

/// A C++ exception from the decoder or from reading a property (all are
/// `std::runtime_error`s); `what()` is the message. Messages of tiles that
/// mlt-core rejects are mlt-core's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error { message: message.into() }
    }

    pub fn what(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<mlt_core::MltError> for Error {
    fn from(e: mlt_core::MltError) -> Self {
        Error::new(e.to_string())
    }
}

/// `mlt::Coordinate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinate {
    pub x: f32,
    pub y: f32,
}

impl Coordinate {
    /// `coord(std::int32_t x, std::int32_t y)`: `static_cast<float>`.
    fn from_vertex(x: i32, y: i32) -> Self {
        Coordinate { x: x as f32, y: y as f32 }
    }
}

/// `mlt::CoordVec`.
pub type CoordVec = Vec<Coordinate>;

/// `mlt::metadata::tileset::GeometryType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryType {
    Point = 0,
    LineString = 1,
    Polygon = 2,
    MultiPoint = 3,
    MultiLineString = 4,
    MultiPolygon = 5,
}

/// The data of the `mlt::geometry::Geometry` subclasses.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// `Point::getCoordinate()`
    Point(Coordinate),
    /// `MultiPoint::getCoordinates()`
    MultiPoint(CoordVec),
    /// `LineString` (a `MultiPoint` subclass): `getCoordinates()`
    LineString(CoordVec),
    /// `Polygon::getRings()`
    Polygon(Vec<CoordVec>),
    /// `MultiLineString::getLineStrings()`
    MultiLineString(Vec<CoordVec>),
    /// `MultiPolygon::getPolygons()`
    MultiPolygon(Vec<Vec<CoordVec>>),
}

/// `mlt::geometry::Geometry`: its type, data and triangle span.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub shape: Shape,
    /// `getTriangles()`, as a range of the layer's triangle buffer
    /// ([`Layer::triangle_buffer`]); `None` for an empty span.
    pub triangles: Option<Range<usize>>,
}

impl Geometry {
    /// `Geometry::type`.
    pub fn type_(&self) -> GeometryType {
        match self.shape {
            Shape::Point(_) => GeometryType::Point,
            Shape::MultiPoint(_) => GeometryType::MultiPoint,
            Shape::LineString(_) => GeometryType::LineString,
            Shape::Polygon(_) => GeometryType::Polygon,
            Shape::MultiLineString(_) => GeometryType::MultiLineString,
            Shape::MultiPolygon(_) => GeometryType::MultiPolygon,
        }
    }
}

/// `mlt::metadata::tileset::ScalarType` of a property column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarType {
    Boolean,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Float,
    Double,
    String,
}

/// `mlt::Property`: the alternatives `PresentProperties::getProperty`
/// produces (the `std::optional<T>` and `nullptr_t` ones never are).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Property<'a> {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(&'a str),
}

/// One value per feature (`None`: absent), the C++ `PropertyVec` with the
/// present bits applied.
#[derive(Clone, Debug, PartialEq)]
enum PropertyVec {
    Bool(Vec<Option<bool>>),
    I32(Vec<Option<i32>>),
    U32(Vec<Option<u32>>),
    I64(Vec<Option<i64>>),
    U64(Vec<Option<u64>>),
    F32(Vec<Option<f32>>),
    F64(Vec<Option<f64>>),
    /// The strings back to back, and each feature's byte range in them.
    Str {
        data: String,
        spans: Vec<Option<(u32, u32)>>,
    },
}

impl PropertyVec {
    fn len(&self) -> usize {
        match self {
            PropertyVec::Bool(v) => v.len(),
            PropertyVec::I32(v) => v.len(),
            PropertyVec::U32(v) => v.len(),
            PropertyVec::I64(v) => v.len(),
            PropertyVec::U64(v) => v.len(),
            PropertyVec::F32(v) => v.len(),
            PropertyVec::F64(v) => v.len(),
            PropertyVec::Str { spans, .. } => spans.len(),
        }
    }

    fn get(&self, i: usize) -> Option<Property<'_>> {
        match self {
            PropertyVec::Bool(v) => v[i].map(Property::Bool),
            PropertyVec::I32(v) => v[i].map(Property::I32),
            PropertyVec::U32(v) => v[i].map(Property::U32),
            PropertyVec::I64(v) => v[i].map(Property::I64),
            PropertyVec::U64(v) => v[i].map(Property::U64),
            PropertyVec::F32(v) => v[i].map(Property::F32),
            PropertyVec::F64(v) => v[i].map(Property::F64),
            PropertyVec::Str { data, spans } => {
                spans[i].map(|(start, end)| Property::Str(&data[start as usize..end as usize]))
            }
        }
    }

    fn strings<'a>(len: usize, get: impl Fn(usize) -> Option<&'a str>, strip_bom: bool) -> PropertyVec {
        let mut data = String::new();
        let mut spans = Vec::with_capacity(len);
        for i in 0..len {
            spans.push(get(i).map(|s| {
                // StringDecoder::view: "Drop the useless codepoint produced
                // when a UTF-16 Byte-Order-Mark is included in the conversion
                // to UTF-8".
                let s = if strip_bom { s.strip_prefix('\u{feff}').unwrap_or(s) } else { s };
                let start = data.len() as u32;
                data.push_str(s);
                (start, data.len() as u32)
            }));
        }
        PropertyVec::Str { data, spans }
    }
}

/// `mlt::PresentProperties`: a property column.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentProperties {
    type_: ScalarType,
    /// The column has a present stream (C++ `physicalIndexes` is set).
    optional: bool,
    values: PropertyVec,
    /// Port-only: the codes of a string column ([`PresentProperties::codes`]).
    string_codes: StringCodes,
}

/// Each string's code, numbered by first use; [`StringCodes::ABSENT`] for an
/// absent value. `None` if the strings are too varied to be worth coding.
/// Made on first use; a cache, so it never makes two columns unequal.
#[derive(Clone, Debug, Default)]
struct StringCodes(OnceLock<Option<Box<[u32]>>>);

impl StringCodes {
    const ABSENT: u32 = u32::MAX;

    fn get_or_init(&self, spans: &[Option<(u32, u32)>], data: &str) -> Option<&[u32]> {
        let codes = self.0.get_or_init(|| {
            // Past about a quarter as many strings as values, the codes would
            // group little and cost about as much as reading every value.
            let limit = spans.len() / 4 + 8;
            let mut index: HashMap<&str, u32> = HashMap::new();
            let mut codes = Vec::with_capacity(spans.len());
            for span in spans {
                codes.push(match *span {
                    Some((start, end)) => {
                        let next = index.len() as u32;
                        let code = *index.entry(&data[start as usize..end as usize]).or_insert(next);
                        if index.len() > limit {
                            return None;
                        }
                        code
                    }
                    None => Self::ABSENT,
                });
            }
            Some(codes.into_boxed_slice())
        });
        codes.as_deref()
    }
}

impl PartialEq for StringCodes {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Port-only: [`PresentProperties::codes`].
#[derive(Clone, Copy)]
pub struct ValueCodes<'a> {
    column: &'a PresentProperties,
    strings: &'a [u32],
}

impl ValueCodes<'_> {
    /// The code of the value at `logical_index`, `None` for an absent value;
    /// throws where [`PresentProperties::get_property`] does.
    pub fn get(&self, logical_index: u32) -> Result<Option<u64>, Error> {
        let i = logical_index as usize;
        let column = self.column;
        if i >= column.values.len() {
            return if column.optional { Ok(None) } else { Err(Error::new("Missing property value")) };
        }
        Ok(match &column.values {
            PropertyVec::Bool(v) => v[i].map(u64::from),
            PropertyVec::I32(v) => v[i].map(|x| u64::from(x as u32)),
            PropertyVec::U32(v) => v[i].map(u64::from),
            PropertyVec::I64(v) => v[i].map(|x| x as u64),
            PropertyVec::U64(v) => v[i],
            PropertyVec::F32(v) => v[i].map(|x| u64::from(x.to_bits())),
            PropertyVec::F64(v) => v[i].map(f64::to_bits),
            PropertyVec::Str { .. } => match self.strings[i] {
                StringCodes::ABSENT => None,
                code => Some(u64::from(code)),
            },
        })
    }
}

impl PresentProperties {
    pub fn get_type(&self) -> ScalarType {
        self.type_
    }

    pub fn is_boolean(&self) -> bool {
        self.type_ == ScalarType::Boolean
    }

    /// `getProperty(logicalIndex)`: `None` for an absent value. Past the end
    /// of a column without a present stream the C++ throws
    /// `std::runtime_error("Missing property value")` (after a failing
    /// `assert` in debug builds).
    /// Port-only: a code for each value, equal codes for equal values (equal
    /// values may differ in code). Numbers and booleans are their bits;
    /// strings are numbered, on the first call. `None` for a string column
    /// with more distinct strings than about a quarter of its values.
    pub fn codes(&self) -> Option<ValueCodes<'_>> {
        let strings = match &self.values {
            PropertyVec::Str { data, spans } => self.string_codes.get_or_init(spans, data)?,
            _ => &[],
        };
        Some(ValueCodes { column: self, strings })
    }

    pub fn get_property(&self, logical_index: u32) -> Result<Option<Property<'_>>, Error> {
        let i = logical_index as usize;
        if i < self.values.len() {
            Ok(self.values.get(i))
        } else if self.optional {
            Ok(None)
        } else {
            Err(Error::new("Missing property value"))
        }
    }
}

/// `mlt::Feature`.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    id: Option<u64>,
    index: u32,
    geometry: Geometry,
}

impl Feature {
    pub fn get_id(&self) -> Option<u64> {
        self.id
    }

    /// The index of the feature's values in the property columns.
    pub fn get_index(&self) -> u32 {
        self.index
    }

    pub fn get_geometry(&self) -> &Geometry {
        &self.geometry
    }

    /// `getProperty(key, layer)`: `None` for an unknown key or an absent value.
    pub fn get_property<'l>(&self, key: &str, layer: &'l Layer) -> Result<Option<Property<'l>>, Error> {
        match layer.get_property_column(key) {
            Some(column) => column.get_property(self.index),
            None => Ok(None),
        }
    }
}

/// `mlt::Layer`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    name: String,
    extent: u32,
    features: Vec<Feature>,
    /// C++ `PropertyVecMap` (an `unordered_map`); here in wire order.
    properties: Vec<(String, PresentProperties)>,
    property_index: HashMap<String, usize>,
    /// The tile's triangle indices, per feature (see [`Geometry::triangles`]).
    triangles: Option<Arc<[u32]>>,
}

impl Layer {
    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn get_extent(&self) -> u32 {
        self.extent
    }

    pub fn get_features(&self) -> &[Feature] {
        &self.features
    }

    /// `getProperties()`: every column, in wire order (C++: hash order).
    pub fn get_properties(&self) -> &[(String, PresentProperties)] {
        &self.properties
    }

    /// `getProperties().find(key)`.
    pub fn get_property_column(&self, key: &str) -> Option<&PresentProperties> {
        self.property_index.get(key).map(|&i| &self.properties[i].1)
    }

    /// The buffer that the features' triangle ranges index.
    pub fn triangle_buffer(&self) -> Option<&Arc<[u32]>> {
        self.triangles.as_ref()
    }

    /// `feature.getGeometry().getTriangles()`.
    pub fn get_triangles(&self, feature: &Feature) -> &[u32] {
        match (&self.triangles, &feature.geometry.triangles) {
            (Some(buffer), Some(range)) => &buffer[range.clone()],
            _ => &[],
        }
    }
}

/// `mlt::MapLibreTile`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapLibreTile {
    layers: Vec<Layer>,
}

impl MapLibreTile {
    /// `mlt::Decoder(supportFastPFOR).decode(DataView)`.
    pub fn decode(data: &[u8], support_fast_pfor: bool) -> Result<MapLibreTile, Error> {
        let mut parser = mlt_core::Parser::default();
        let layers = parser.parse_layers(data)?;
        let mut decoder = mlt_core::Decoder::default();
        let mut result = Vec::new();
        for layer in layers {
            let mlt_core::Layer::Tag01(lazy) = layer else {
                // Skipping unknown layer
                continue;
            };
            if !support_fast_pfor && uses_fast_pfor(&lazy) {
                // IntegerDecoder::decodeFastPfor
                return Err(Error::new("FastPFOR decoding is not enabled"));
            }
            let mut optional = Vec::new();
            lazy.for_each_column_decl(&mut |decl| {
                if let ColumnDecl::Value { optional: o, .. } = decl {
                    optional.push(o);
                }
            });
            let parsed = lazy.decode_all(&mut decoder)?;
            result.push(Layer::from_parsed(&parsed, &optional)?);
        }
        Ok(MapLibreTile { layers: result })
    }

    pub fn get_layers(&self) -> &[Layer] {
        &self.layers
    }

    /// `getLayer(name)`: the first layer with the name.
    pub fn get_layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.name == name)
    }
}

/// Whether the C++ `IntegerDecoder` would decode a FastPFOR stream of the
/// layer: integer and vertex streams go through it; present, boolean and
/// float streams are read raw whatever their physical technique.
fn uses_fast_pfor(layer: &mlt_core::Layer01<'_>) -> bool {
    let mut found = false;
    layer.for_each_stream(&mut |meta| {
        if matches!(meta.encoding.physical, PhysicalEncoding::FastPFor(_))
            && matches!(meta.encoding.logical, LogicalEncoding::Int(_) | LogicalEncoding::Vertex(_))
        {
            found = true;
        }
    });
    found
}

impl Layer {
    /// `Decoder::Impl::parseBasicMVTEquivalent` after the streams are
    /// decoded: the features (`makeFeatures` on the geometry vector's
    /// geometries) and the property columns. `optional` is the present flag
    /// of each property column (shared dictionary children one by one).
    fn from_parsed(parsed: &ParsedLayer01<'_>, optional: &[bool]) -> Result<Layer, Error> {
        let (geometries, triangles) = get_geometries(parsed.geometry_values())?;

        let ids: Vec<Option<u64>> = parsed.id().map(|id| id.iter_optional().collect()).unwrap_or_default();
        let feature_count = geometries.len();
        if !ids.is_empty() && ids.len() != feature_count {
            return Err(Error::new(format!(
                "ID count ({}) does not match geometry count ({feature_count})",
                ids.len()
            )));
        }
        let features = geometries
            .into_iter()
            .enumerate()
            .map(|(i, geometry)| Feature { id: if ids.is_empty() { None } else { ids[i] }, index: i as u32, geometry })
            .collect();

        let mut names = parsed.iterate_prop_names().map(|name| name.to_string());
        let mut optional = optional.iter().copied();
        let mut layer = Layer {
            name: parsed.name().to_string(),
            extent: parsed.extent().get(),
            features,
            properties: Vec::new(),
            property_index: HashMap::new(),
            triangles,
        };
        let mut next = || -> Result<(String, bool), Error> {
            match (names.next(), optional.next()) {
                (Some(name), Some(optional)) => Ok((name, optional)),
                _ => Err(Error::new("property column count mismatch")),
            }
        };
        for column in parsed.properties() {
            let (type_, values) = match column {
                ParsedProperty::Bool(s) => (ScalarType::Boolean, PropertyVec::Bool(s.iter_optional().collect())),
                ParsedProperty::I8(_) | ParsedProperty::U8(_) => {
                    return Err(Error::new("8-bit integer type not implemented"));
                }
                ParsedProperty::I32(s) => (ScalarType::Int32, PropertyVec::I32(s.iter_optional().collect())),
                ParsedProperty::U32(s) => (ScalarType::UInt32, PropertyVec::U32(s.iter_optional().collect())),
                ParsedProperty::I64(s) => (ScalarType::Int64, PropertyVec::I64(s.iter_optional().collect())),
                ParsedProperty::U64(s) => (ScalarType::UInt64, PropertyVec::U64(s.iter_optional().collect())),
                ParsedProperty::F32(s) => (ScalarType::Float, PropertyVec::F32(s.iter_optional().collect())),
                ParsedProperty::F64(s) => (ScalarType::Double, PropertyVec::F64(s.iter_optional().collect())),
                ParsedProperty::Str(s) => {
                    (ScalarType::String, PropertyVec::strings(s.feature_count(), |i| s.get(i as u32), true))
                }
                ParsedProperty::SharedDict(dict) => {
                    // StringDecoder::decodeSharedDictionary: one column per
                    // child, named prefix + suffix.
                    for item in dict.items() {
                        let (name, optional) = next()?;
                        let values = PropertyVec::strings(item.feature_count(), |i| item.get(dict, i), false);
                        layer.insert_column(
                            name,
                            PresentProperties {
                                type_: ScalarType::String,
                                optional,
                                values,
                                string_codes: StringCodes::default(),
                            },
                        );
                    }
                    continue;
                }
                _ => return Err(Error::new("unsupported property column")),
            };
            let (name, optional) = next()?;
            layer.insert_column(
                name,
                PresentProperties { type_, optional, values, string_codes: StringCodes::default() },
            );
        }
        Ok(layer)
    }

    /// `PropertyVecMap::insert`: an existing name keeps its column.
    fn insert_column(&mut self, name: String, column: PresentProperties) {
        if !self.property_index.contains_key(&name) {
            self.property_index.insert(name.clone(), self.properties.len());
            self.properties.push((name, column));
        }
    }
}

/// `GeometryVector::getGeometries`, on mlt-core's decoded topology: one
/// geometry per feature, except multi-points without points, plus the
/// per-feature triangle indices.
fn get_geometries(values: &GeometryValues) -> Result<(Vec<Geometry>, Option<Arc<[u32]>>), Error> {
    let vertices = values.vertices().unwrap_or(&[]);
    let geoms = values.geometry_offsets();
    let parts = values.part_offsets();
    let rings = values.ring_offsets();
    let types = values.vector_types();

    let vertex = |i: usize| -> Result<Coordinate, Error> {
        match vertices.get(2 * i..2 * i + 2) {
            Some(v) => Ok(Coordinate::from_vertex(v[0], v[1])),
            None => Err(Error::new("vertexBuffer underflow")),
        }
    };
    let offset = |level: &[u32], i: usize, name: &str| -> Result<usize, Error> {
        match level.get(i) {
            Some(&v) => Ok(v as usize),
            None => Err(Error::new(format!("{name} underflow"))),
        }
    };
    let span = |level: &[u32], i: usize, name: &str| -> Result<Range<usize>, Error> {
        Ok(offset(level, i, name)?..offset(level, i + 1, name)?)
    };
    fn require<'v>(level: Option<&'v [u32]>, what: &str) -> Result<&'v [u32], Error> {
        level.ok_or_else(|| Error::new(format!("{what} geometry without topology vector")))
    }
    // getLineStringCoords: the C++ bounds check `startIndex + numVertices - 1`
    // wraps for an empty line at the start of the buffer.
    let line = |range: Range<usize>, close: bool| -> Result<CoordVec, Error> {
        if range.is_empty() && range.start == 0 {
            return Err(Error::new("vertexBuffer underflow"));
        }
        let mut coords = Vec::with_capacity(range.len() + usize::from(close));
        for i in range {
            coords.push(vertex(i)?);
        }
        if close
            && let (Some(&front), Some(&back)) = (coords.first(), coords.last())
            && front != back
        {
            coords.push(front);
        }
        Ok(coords)
    };

    let mut geometries = Vec::with_capacity(types.len());
    // The geometry of each feature (`None`: dropped).
    let mut geometry_of = Vec::with_capacity(types.len());

    if parts.is_none() && values.index_buffer().is_some() {
        // A FlatGpuVector without topology: points read the vertex buffer in
        // order, every other type throws.
        let mut next_vertex = 0;
        for &t in types {
            match t {
                WireGeometryType::Point => {
                    geometries.push(Geometry { shape: Shape::Point(vertex(next_vertex)?), triangles: None });
                    next_vertex += 1;
                }
                other => return Err(Error::new(format!("{} geometry without topology vector", cpp_name(other)))),
            }
        }
        return Ok((geometries, None));
    }

    for (index, &t) in types.iter().enumerate() {
        let shape = match t {
            WireGeometryType::Point => {
                let mut i = match geoms {
                    Some(g) => offset(g, index, "geometryOffsets")?,
                    None => index,
                };
                if let Some(p) = parts {
                    i = offset(p, i, "partOffsets")?;
                }
                if let Some(r) = rings {
                    i = offset(r, i, "ringOffsets")?;
                }
                Shape::Point(vertex(i)?)
            }
            WireGeometryType::LineString => {
                let p = require(parts, cpp_name(t))?;
                let part = match geoms {
                    Some(g) => offset(g, index, "geometryOffsets")?,
                    None => index,
                };
                let range = match rings {
                    Some(r) => span(r, offset(p, part, "partOffsets")?, "ringOffsets")?,
                    None => span(p, part, "partOffsets")?,
                };
                Shape::LineString(line(range, false)?)
            }
            WireGeometryType::Polygon => {
                let p = require(parts, cpp_name(t))?;
                let r = require(rings, cpp_name(t))?;
                let polygon = match geoms {
                    Some(g) => offset(g, index, "geometryOffsets")?,
                    None => index,
                };
                let mut result = Vec::new();
                for ring in span(p, polygon, "partOffsets")? {
                    result.push(line(span(r, ring, "ringOffsets")?, true)?);
                }
                Shape::Polygon(result)
            }
            WireGeometryType::MultiPoint => {
                let g = require(geoms, cpp_name(t))?;
                let mut points = Vec::new();
                for i in span(g, index, "geometryOffsets")? {
                    let i = match (parts, rings) {
                        (Some(p), Some(r)) => offset(r, offset(p, i, "partOffsets")?, "ringOffsets")?,
                        (Some(p), None) => offset(p, i, "partOffsets")?,
                        (None, _) => i,
                    };
                    points.push(vertex(i)?);
                }
                if points.is_empty() {
                    // `if (numPoints)`: no geometry for this feature.
                    geometry_of.push(None);
                    continue;
                }
                Shape::MultiPoint(points)
            }
            WireGeometryType::MultiLineString => {
                let g = require(geoms, cpp_name(t))?;
                let lines = span(g, index, "geometryOffsets")?;
                let mut result = Vec::with_capacity(lines.len());
                if !lines.is_empty() {
                    let p = require(parts, cpp_name(t))?;
                    for i in lines {
                        let range = match rings {
                            Some(r) => span(r, offset(p, i, "partOffsets")?, "ringOffsets")?,
                            None => span(p, i, "partOffsets")?,
                        };
                        result.push(line(range, false)?);
                    }
                }
                Shape::MultiLineString(result)
            }
            WireGeometryType::MultiPolygon => {
                let g = require(geoms, cpp_name(t))?;
                let polygons = span(g, index, "geometryOffsets")?;
                let mut result = Vec::with_capacity(polygons.len());
                if !polygons.is_empty() {
                    let p = require(parts, cpp_name(t))?;
                    let r = require(rings, cpp_name(t))?;
                    for polygon in polygons {
                        let mut rings_out = Vec::new();
                        for ring in span(p, polygon, "partOffsets")? {
                            rings_out.push(line(span(r, ring, "ringOffsets")?, true)?);
                        }
                        result.push(rings_out);
                    }
                }
                Shape::MultiPolygon(result)
            }
        };
        geometry_of.push(Some(geometries.len()));
        geometries.push(Geometry { shape, triangles: None });
    }

    // GeometryVector::applyTriangles: each (multi-)polygon takes the next
    // count of triangles from the index buffer. mlt-core counts the indices
    // from the layer's first vertex; the tile (and the C++) from the feature's.
    let mut buffer = None;
    if let (Some(indices), Some(counts)) = (values.index_buffer(), values.triangle_offsets()) {
        let mut rebased = Vec::with_capacity(indices.len());
        let mut runs = counts.windows(2);
        for (index, &t) in types.iter().enumerate() {
            if !matches!(t, WireGeometryType::Polygon | WireGeometryType::MultiPolygon) {
                continue;
            }
            let run = runs.next().ok_or_else(|| Error::new("triangleCounts underflow"))?;
            let run = run[0] as usize * 3..run[1] as usize * 3;
            let corners = indices.get(run).ok_or_else(|| Error::new("indexBuffer underflow"))?;
            if corners.is_empty() {
                continue;
            }
            let start = values.vertex_range(index)?.start as u32;
            let first = rebased.len();
            rebased.extend(corners.iter().map(|&i| i.wrapping_sub(start)));
            // Polygons are never dropped.
            if let Some(g) = geometry_of[index] {
                geometries[g].triangles = Some(first..rebased.len());
            }
        }
        buffer = Some(Arc::from(rebased));
    }

    Ok((geometries, buffer))
}

/// The geometry names of the C++ "... geometry without topology vector" errors.
fn cpp_name(t: WireGeometryType) -> &'static str {
    match t {
        WireGeometryType::Point => "Point",
        WireGeometryType::LineString => "Linestring",
        WireGeometryType::Polygon => "Polygon",
        WireGeometryType::MultiPoint => "Multi-point",
        WireGeometryType::MultiLineString => "Multi-Linestring",
        WireGeometryType::MultiPolygon => "Multi-polygon",
    }
}
