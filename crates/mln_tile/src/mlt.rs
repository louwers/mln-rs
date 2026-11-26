// MapLibre Tiles are decoded by the mlt-core crate. A tile's layers decode on
// the first read, all at once, into columns the layer owns: mlt-core's
// decoded layer borrows the tile's bytes, so it can't be kept beside them.
// The geometry columns are mlt-core's own; the property columns are copied
// out with one value or absence per feature. A feature is an index into the
// columns, and reads its geometry from them when asked.
//
// A tile mlt-core can't decode, and one that uses FastPFOR where it isn't
// enabled, is logged and reads as a tile without layers.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use feature::{Identifier, PropertyMap, Value};
use mln_style::constants::EXTENT;
use mln_style::feature::{GeometryCollection, GeometryCoordinate, GeometryCoordinates};
use mln_style::geometry::FeatureType;
use mln_style::hash::HashMap;
use mlt_core::wire::{Analyze as _, LogicalEncoding, PhysicalEncoding};
use mlt_core::{GeometryType, GeometryValues, ParsedLayer01, ParsedProperty};

use crate::tile_data::FeatureGroups;
use crate::{TileError, log_target};

struct MltData {
    bytes: Arc<Vec<u8>>,
    fast_pfor_enabled: bool,
    /// The decoded layers, in tile order.
    layers: OnceLock<Vec<Layer>>,
}

/// MapLibre Tile data. Cloning shares the bytes and the decoded layers.
#[derive(Clone)]
pub struct MltTile(Arc<MltData>);

impl MltTile {
    /// Tile data that decodes `bytes`; with `fast_pfor_enabled` false, a tile
    /// that uses FastPFOR integer encoding has no layers.
    #[must_use]
    pub fn new(bytes: Arc<Vec<u8>>, fast_pfor_enabled: bool) -> Self {
        MltTile(Arc::new(MltData { bytes, fast_pfor_enabled, layers: OnceLock::new() }))
    }

    /// The tile's bytes.
    #[must_use]
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.0.bytes
    }

    #[must_use]
    pub fn fast_pfor_enabled(&self) -> bool {
        self.0.fast_pfor_enabled
    }

    fn layers(&self) -> &[Layer] {
        self.0.layers.get_or_init(|| {
            decode(&self.0.bytes, self.0.fast_pfor_enabled).unwrap_or_else(|e| {
                log::warn!(target: log_target::PARSE_TILE, "MLT parse failed: {e}");
                Vec::new()
            })
        })
    }

    /// The first layer named `name`.
    pub(crate) fn layer(&self, name: &str) -> Option<MltLayer<'_>> {
        self.layers().iter().find(|layer| layer.name == name).map(MltLayer)
    }

    /// The names of the tile's layers, in tile order, duplicates included.
    #[cfg(test)]
    pub(crate) fn layer_names(&self) -> Vec<String> {
        self.layers().iter().map(|layer| layer.name.clone()).collect()
    }
}

/// Decodes the tile's layers; a layer of a version mlt-core reads but this
/// doesn't is skipped.
fn decode(data: &[u8], fast_pfor_enabled: bool) -> Result<Vec<Layer>, String> {
    let error = |e: mlt_core::MltError| e.to_string();
    let mut parser = mlt_core::Parser::default();
    let mut decoder = mlt_core::Decoder::default();
    let mut layers = Vec::new();
    for layer in parser.parse_layers(data).map_err(error)? {
        let mlt_core::Layer::Tag01(layer) = layer else {
            continue;
        };
        if !fast_pfor_enabled && uses_fast_pfor(&layer) {
            return Err("FastPFOR decoding is not enabled".to_owned());
        }
        layers.push(Layer::new(&layer.decode_all(&mut decoder).map_err(error)?)?);
    }
    Ok(layers)
}

/// Whether an integer or vertex stream of the layer uses FastPFOR.
fn uses_fast_pfor(layer: &mlt_core::Layer01<'_>) -> bool {
    let mut found = false;
    layer.for_each_stream(&mut |meta| {
        found |= matches!(meta.encoding.physical, PhysicalEncoding::FastPFor(_))
            && matches!(meta.encoding.logical, LogicalEncoding::Int(_) | LogicalEncoding::Vertex(_));
    });
    found
}

/// A property column: each feature's value, `None` for an absent one.
enum Values {
    Bool(Vec<Option<bool>>),
    Int(Vec<Option<i64>>),
    Uint(Vec<Option<u64>>),
    Double(Vec<Option<f64>>),
    /// The strings back to back, and each feature's byte range in them.
    Str {
        data: String,
        spans: Vec<Option<(u32, u32)>>,
    },
}

impl Values {
    fn strings<'a>(len: usize, get: impl Fn(usize) -> Option<&'a str>) -> Values {
        let mut data = String::new();
        let spans = (0..len)
            .map(|i| {
                get(i).map(|s| {
                    let start = data.len() as u32;
                    data.push_str(s);
                    (start, data.len() as u32)
                })
            })
            .collect();
        Values::Str { data, spans }
    }

    /// The value of feature `i`; `None` for an absent value, also past the
    /// end of the column.
    fn get(&self, i: usize) -> Option<Value> {
        match self {
            Values::Bool(v) => v.get(i).copied().flatten().map(Value::Bool),
            Values::Int(v) => v.get(i).copied().flatten().map(Value::Int),
            Values::Uint(v) => v.get(i).copied().flatten().map(Value::Uint),
            Values::Double(v) => v.get(i).copied().flatten().map(Value::Double),
            Values::Str { data, spans } => spans
                .get(i)
                .copied()
                .flatten()
                .map(|(start, end)| Value::String(data[start as usize..end as usize].to_owned())),
        }
    }
}

struct Column {
    values: Values,
    /// Each string's code, numbered by first use, made by the first
    /// [`Column::codes`]; `None` if the strings are too varied to be worth
    /// coding.
    string_codes: OnceLock<Option<Box<[u32]>>>,
}

/// Equal codes for equal values ([`Column::codes`]).
struct ValueCodes<'a> {
    values: &'a Values,
    strings: &'a [u32],
}

impl ValueCodes<'_> {
    /// The code of feature `i`'s value, `None` for an absent value.
    fn get(&self, i: usize) -> Option<u64> {
        match self.values {
            Values::Bool(v) => v.get(i).copied().flatten().map(u64::from),
            Values::Int(v) => v.get(i).copied().flatten().map(|x| x as u64),
            Values::Uint(v) => v.get(i).copied().flatten(),
            Values::Double(v) => v.get(i).copied().flatten().map(f64::to_bits),
            Values::Str { .. } => self.strings.get(i).copied().filter(|&code| code != u32::MAX).map(u64::from),
        }
    }
}

impl Column {
    fn new(values: Values) -> Self {
        Column { values, string_codes: OnceLock::new() }
    }

    /// A code for each value, equal codes for equal values (equal values may
    /// differ in code). Numbers and booleans are their bits; strings are
    /// numbered, on the first call. `None` for a string column with more
    /// distinct strings than about a quarter of its values.
    fn codes(&self) -> Option<ValueCodes<'_>> {
        let strings = match &self.values {
            Values::Str { data, spans } => self
                .string_codes
                .get_or_init(|| {
                    // Past about a quarter as many strings as values, the codes
                    // would group little and cost about as much as reading
                    // every value.
                    let limit = spans.len() / 4 + 8;
                    let mut index: HashMap<&str, u32> = HashMap::default();
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
                            None => u32::MAX,
                        });
                    }
                    Some(codes.into_boxed_slice())
                })
                .as_deref()?,
            _ => &[],
        };
        Some(ValueCodes { values: &self.values, strings })
    }
}

/// The triangles of a layer's tessellated polygons.
struct Triangles {
    /// Three indices per triangle, each into its feature's decoded vertices
    /// (see [`GeometryCollection::get_triangles`]).
    indices: Arc<[u32]>,
    /// Each feature's range of `indices`; `None` for a feature without
    /// triangles.
    of_feature: Vec<Option<Range<usize>>>,
}

/// The lines or rings of a feature, as the vertices they take.
enum Parts {
    /// A point, or the points of a multi-point, as one line.
    Points(Vec<usize>),
    Lines(Vec<Range<usize>>),
    /// Polygon rings, which decoding closes.
    Rings(Vec<Range<usize>>),
}

struct Layer {
    name: String,
    extent: u32,
    geometry: GeometryValues,
    ids: Option<Vec<Option<u64>>>,
    /// The property columns in wire order; of two with the same name, the
    /// first.
    columns: Vec<(String, Column)>,
    column_index: HashMap<String, usize>,
    triangles: Option<Triangles>,
}

impl Layer {
    fn new(parsed: &ParsedLayer01<'_>) -> Result<Layer, String> {
        let mut layer = Layer {
            name: parsed.name().to_owned(),
            extent: parsed.extent().get(),
            geometry: parsed.geometry_values().clone(),
            ids: parsed.id().map(|id| id.iter_optional().collect()),
            columns: Vec::new(),
            column_index: HashMap::default(),
            triangles: None,
        };
        let mut names = parsed.iterate_prop_names().map(|name| name.to_string());
        let mut next_name = || names.next().ok_or_else(|| "property column count mismatch".to_owned());
        for column in parsed.properties() {
            let values = match column {
                ParsedProperty::Bool(s) => Values::Bool(s.iter_optional().collect()),
                ParsedProperty::I8(s) => Values::Int(s.iter_optional().map(|v| v.map(i64::from)).collect()),
                ParsedProperty::U8(s) => Values::Uint(s.iter_optional().map(|v| v.map(u64::from)).collect()),
                ParsedProperty::I32(s) => Values::Int(s.iter_optional().map(|v| v.map(i64::from)).collect()),
                ParsedProperty::U32(s) => Values::Uint(s.iter_optional().map(|v| v.map(u64::from)).collect()),
                ParsedProperty::I64(s) => Values::Int(s.iter_optional().collect()),
                ParsedProperty::U64(s) => Values::Uint(s.iter_optional().collect()),
                ParsedProperty::F32(s) => Values::Double(s.iter_optional().map(|v| v.map(f64::from)).collect()),
                ParsedProperty::F64(s) => Values::Double(s.iter_optional().collect()),
                ParsedProperty::Str(s) => Values::strings(s.feature_count(), |i| s.get(i as u32)),
                ParsedProperty::SharedDict(dict) => {
                    // One column per child, named prefix + suffix.
                    for item in dict.items() {
                        let values = Values::strings(item.feature_count(), |i| item.get(dict, i));
                        layer.insert_column(next_name()?, values);
                    }
                    continue;
                }
                _ => return Err(format!("unsupported property column type {:?}", column.kind())),
            };
            layer.insert_column(next_name()?, values);
        }
        layer.triangles = layer.triangles();
        Ok(layer)
    }

    fn insert_column(&mut self, name: String, values: Values) {
        if !self.column_index.contains_key(&name) {
            self.column_index.insert(name.clone(), self.columns.len());
            self.columns.push((name, Column::new(values)));
        }
    }

    fn column(&self, key: &str) -> Option<&Column> {
        self.column_index.get(key).map(|&i| &self.columns[i].1)
    }

    fn len(&self) -> usize {
        self.geometry.feature_count()
    }

    fn feature_type(&self, index: usize) -> FeatureType {
        match self.geometry.vector_types()[index] {
            GeometryType::Point | GeometryType::MultiPoint => FeatureType::Point,
            GeometryType::LineString | GeometryType::MultiLineString => FeatureType::LineString,
            GeometryType::Polygon | GeometryType::MultiPolygon => FeatureType::Polygon,
        }
    }

    /// The lines or rings of feature `index`.
    fn parts(&self, index: usize) -> Result<Parts, TileError> {
        let values = &self.geometry;
        let geometry_type = values.vector_types()[index];
        let geoms = values.geometry_offsets();
        let parts = values.part_offsets();
        let rings = values.ring_offsets();
        let malformed = |what: &str| TileError::Geometry(format!("MLT {what} out of range"));
        let offset =
            |level: &[u32], i: usize, what: &str| level.get(i).map(|&v| v as usize).ok_or_else(|| malformed(what));
        let span = |level: &[u32], i: usize, what: &str| {
            Ok::<_, TileError>(offset(level, i, what)?..offset(level, i + 1, what)?)
        };
        let no_topology = || TileError::Geometry(format!("MLT {geometry_type} geometry without topology"));
        // The geometry, else this feature's first part.
        let first = |i: usize| geoms.map_or(Ok(i), |g| offset(g, i, "geometry offset"));
        // A line: a range of rings, else of parts.
        let line = |part: usize, p: &[u32]| match rings {
            Some(r) => span(r, offset(p, part, "part offset")?, "ring offset"),
            None => span(p, part, "part offset"),
        };

        if parts.is_none() && values.index_buffer().is_some() {
            // Tessellated without topology: only points can be read.
            return match geometry_type {
                GeometryType::Point => Ok(Parts::Points(vec![index])),
                _ => Err(no_topology()),
            };
        }
        Ok(match geometry_type {
            GeometryType::Point => {
                let mut i = first(index)?;
                if let Some(p) = parts {
                    i = offset(p, i, "part offset")?;
                }
                if let Some(r) = rings {
                    i = offset(r, i, "ring offset")?;
                }
                Parts::Points(vec![i])
            }
            GeometryType::MultiPoint => {
                let g = geoms.ok_or_else(no_topology)?;
                let mut points = Vec::new();
                for i in span(g, index, "geometry offset")? {
                    points.push(match (parts, rings) {
                        (Some(p), Some(r)) => offset(r, offset(p, i, "part offset")?, "ring offset")?,
                        (Some(p), None) => offset(p, i, "part offset")?,
                        (None, _) => i,
                    });
                }
                Parts::Points(points)
            }
            GeometryType::LineString => Parts::Lines(vec![line(first(index)?, parts.ok_or_else(no_topology)?)?]),
            GeometryType::MultiLineString => {
                let lines = span(geoms.ok_or_else(no_topology)?, index, "geometry offset")?;
                let mut result = Vec::with_capacity(lines.len());
                for i in lines {
                    result.push(line(i, parts.ok_or_else(no_topology)?)?);
                }
                Parts::Lines(result)
            }
            GeometryType::Polygon => {
                let (p, r) = (parts.ok_or_else(no_topology)?, rings.ok_or_else(no_topology)?);
                let mut result = Vec::new();
                for i in span(p, first(index)?, "part offset")? {
                    result.push(span(r, i, "ring offset")?);
                }
                Parts::Rings(result)
            }
            GeometryType::MultiPolygon => {
                let polygons = span(geoms.ok_or_else(no_topology)?, index, "geometry offset")?;
                let mut result = Vec::new();
                for polygon in polygons {
                    let (p, r) = (parts.ok_or_else(no_topology)?, rings.ok_or_else(no_topology)?);
                    for i in span(p, polygon, "part offset")? {
                        result.push(span(r, i, "ring offset")?);
                    }
                }
                Parts::Rings(result)
            }
        })
    }

    /// The geometry of feature `index`, scaled to `util::EXTENT`. Decoding
    /// closes polygon rings, which the tile stores without their closing
    /// vertex.
    fn geometry(&self, index: usize) -> Result<GeometryCollection, TileError> {
        let vertices = self.geometry.vertices().unwrap_or(&[]);
        let scale = f64::from(EXTENT) / f64::from(self.extent);
        let vertex = |i: usize| -> Result<GeometryCoordinate, TileError> {
            let v = vertices
                .get(2 * i..2 * i + 2)
                .ok_or_else(|| TileError::Geometry("MLT vertex out of range".to_owned()))?;
            let convert = |c: i32| (f64::from(c) * scale).round() as i16;
            Ok(GeometryCoordinate::new(convert(v[0]), convert(v[1])))
        };
        let line = |range: Range<usize>, close: bool| -> Result<GeometryCoordinates, TileError> {
            let mut coords = Vec::with_capacity(range.len() + usize::from(close));
            for i in range.clone() {
                coords.push(vertex(i)?);
            }
            if close && let Some(&first) = coords.first() {
                coords.push(first);
            }
            Ok(coords)
        };
        let mut geometry = match self.parts(index)? {
            Parts::Points(points) => {
                GeometryCollection::from(vec![points.into_iter().map(vertex).collect::<Result<Vec<_>, _>>()?])
            }
            Parts::Lines(lines) => {
                GeometryCollection::from(lines.into_iter().map(|l| line(l, false)).collect::<Result<Vec<_>, _>>()?)
            }
            Parts::Rings(rings) => {
                GeometryCollection::from(rings.into_iter().map(|r| line(r, true)).collect::<Result<Vec<_>, _>>()?)
            }
        };
        if let Some(triangles) = &self.triangles
            && let Some(Some(range)) = triangles.of_feature.get(index)
        {
            geometry.set_triangles(Arc::clone(&triangles.indices), range.clone());
        }
        Ok(geometry)
    }

    /// The triangles of the tessellated polygons: each polygon and
    /// multi-polygon takes the next count of triangles from the index
    /// buffer. A feature whose triangles name a vertex it doesn't have, or
    /// that the counts run out before, has none.
    fn triangles(&self) -> Option<Triangles> {
        let values = &self.geometry;
        let (indices, counts) = (values.index_buffer()?, values.triangle_offsets()?);
        let mut rebased = Vec::with_capacity(indices.len());
        let mut of_feature = vec![None; self.len()];
        let (mut ring_lens, mut ring_shifts) = (Vec::new(), Vec::new());
        let mut runs = counts.windows(2);
        for (index, &t) in values.vector_types().iter().enumerate() {
            if !matches!(t, GeometryType::Polygon | GeometryType::MultiPolygon) {
                continue;
            }
            let Some(run) = runs.next() else { break };
            let Some(corners) = indices.get(run[0] as usize * 3..run[1] as usize * 3) else { break };
            if corners.is_empty() {
                continue;
            }
            let (Ok(Parts::Rings(rings)), Ok(vertices)) = (self.parts(index), values.vertex_range(index)) else {
                continue;
            };
            ring_lens.clear();
            ring_lens.extend(rings.iter().map(|r| (r.len() as u32, (r.len() + usize::from(!r.is_empty())) as u32)));
            let first = rebased.len();
            if rebase_triangles(corners, vertices.start as u32, &ring_lens, &mut ring_shifts, &mut rebased) {
                of_feature[index] = Some(first..rebased.len());
            }
        }
        Some(Triangles { indices: Arc::from(rebased), of_feature })
    }

    /// Port-only: groups by geometry type and by each key's value code
    /// ([`Column::codes`]): equal codes are equal values, and equal values
    /// with distinct codes only split a group. A key without a column is
    /// absent from every feature.
    fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        let count = self.len();
        // Grouping costs about as much as evaluating each feature once.
        let limit = count / 4 + 8;
        let columns: Vec<ValueCodes<'_>> =
            keys.iter().filter_map(|key| self.column(key)).map(Column::codes).collect::<Option<_>>()?;
        let mut index: HashMap<Box<[Option<u64>]>, u32> = HashMap::default();
        let mut codes = vec![None; columns.len() + 1];
        let mut groups = FeatureGroups { of_feature: Vec::with_capacity(count), first: Vec::new() };
        for i in 0..count {
            codes[columns.len()] = Some(self.feature_type(i) as u64);
            for (code, column) in codes.iter_mut().zip(&columns) {
                *code = column.get(i);
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

/// A layer of an [`MltTile`].
#[derive(Clone, Copy)]
pub struct MltLayer<'a>(&'a Layer);

impl<'a> MltLayer<'a> {
    pub(crate) fn name(self) -> &'a str {
        &self.0.name
    }

    pub(crate) fn extent(self) -> u32 {
        self.0.extent
    }

    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    /// The feature at `index`; fails with [`TileError::OutOfRange`] past the
    /// end.
    pub(crate) fn feature(self, index: usize) -> Result<MltFeature<'a>, TileError> {
        if index < self.len() {
            Ok(MltFeature { layer: self.0, index })
        } else {
            Err(TileError::vector_range_check(index, self.len()))
        }
    }

    pub(crate) fn group_features(self, keys: &[String]) -> Option<FeatureGroups> {
        self.0.group_features(keys)
    }
}

/// A feature of an [`MltLayer`].
#[derive(Clone, Copy)]
pub struct MltFeature<'a> {
    layer: &'a Layer,
    index: usize,
}

impl MltFeature<'_> {
    pub(crate) fn get_type(&self) -> FeatureType {
        self.layer.feature_type(self.index)
    }

    pub(crate) fn get_id(&self) -> Identifier {
        let id = self.layer.ids.as_ref().and_then(|ids| ids.get(self.index).copied().flatten());
        id.map_or(Identifier::Null, Identifier::Uint)
    }

    pub(crate) fn get_value(&self, key: &str) -> Option<Value> {
        self.layer.column(key)?.values.get(self.index)
    }

    /// Every property; an absent value is a null.
    pub(crate) fn get_properties(&self) -> PropertyMap {
        (self.layer.columns.iter())
            .map(|(key, column)| (key.clone(), column.values.get(self.index).unwrap_or(Value::Null)))
            .collect()
    }

    pub(crate) fn geometry(&self) -> Result<GeometryCollection, TileError> {
        self.layer.geometry(self.index)
    }
}

/// Appends `corners`, the triangles of a feature as indices of the layer's
/// vertices, to `out` as indices of the feature's decoded vertices; returns
/// `false`, appending nothing, when a corner is not one of the feature's
/// vertices.
///
/// `first_vertex` is the feature's first vertex in the layer, and `rings`
/// the stored and decoded vertex counts of its rings. A ring decodes one
/// vertex longer than it is stored when decoding closes it, which moves the
/// vertices of the rings after it up by one. `ring_shifts` is scratch space.
fn rebase_triangles(
    corners: &[u32],
    first_vertex: u32,
    rings: &[(u32, u32)],
    ring_shifts: &mut Vec<(u32, u32)>,
    out: &mut Vec<u32>,
) -> bool {
    // The first stored vertex of each ring, and how far decoding moves it.
    ring_shifts.clear();
    let (mut stored, mut decoded) = (0u32, 0u32);
    for &(stored_len, decoded_len) in rings {
        ring_shifts.push((stored, decoded - stored));
        stored += stored_len;
        decoded += decoded_len;
    }
    // Only a closing vertex before the last ring moves any corner.
    let moved = ring_shifts.last().is_some_and(|&(_, shift)| shift > 0);
    let first = out.len();
    for &corner in corners {
        let i = corner.wrapping_sub(first_vertex);
        if i >= stored {
            out.truncate(first);
            return false;
        }
        let shift = if moved { ring_shifts[ring_shifts.partition_point(|&(start, _)| start <= i) - 1].1 } else { 0 };
        out.push(i + shift);
    }
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use feature::Value;
    use geometry::Point;
    use mlt_core::encoder::{EncoderConfig, Network, Profile};
    use mlt_core::geo_types::Coord;
    use mlt_core::{LayerWriter, PropKind};

    use super::*;

    /// A layer "l" with a multi-point, a polygon and a line, an 8-bit
    /// integer column, and a string column the line lacks.
    fn tile() -> Vec<u8> {
        let mut layer = LayerWriter::new("l", 2 * EXTENT as u32).unwrap();
        let small = layer.add_property("small", PropKind::I8).unwrap();
        let name = layer.add_property("name", PropKind::Str).unwrap();
        let points = [Coord { x: 2, y: 4 }, Coord { x: 6, y: 8 }];
        let ring = [Coord { x: 0, y: 0 }, Coord { x: 100, y: 0 }, Coord { x: 100, y: 100 }];
        for (i, geometry_type) in
            [GeometryType::MultiPoint, GeometryType::Polygon, GeometryType::LineString].into_iter().enumerate()
        {
            let mut feature = layer.feature(geometry_type);
            match geometry_type {
                GeometryType::MultiPoint => feature.points(points),
                GeometryType::Polygon => feature.exterior_ring(ring),
                _ => feature.line(points),
            }
            .unwrap();
            feature.id(Some(i as u64 + 10));
            feature.property(small, -(i as i8)).unwrap();
            if geometry_type != GeometryType::LineString {
                feature.property(name, "n").unwrap();
            }
            feature.finish().unwrap();
        }
        layer.encode(EncoderConfig::from_profile(Profile::Web(Network::Offline))).unwrap()
    }

    #[test]
    fn features_read_their_columns() {
        let tile = MltTile::new(Arc::new(tile()), false);
        let layer = tile.layer("l").unwrap();
        assert_eq!((layer.name(), layer.len()), ("l", 3));
        let [multi_point, polygon, line] = [0, 1, 2].map(|i| layer.feature(i).unwrap());
        assert!(layer.feature(3).is_err());

        // A multi-point is a point feature, its points one line; an extent
        // of twice `util::EXTENT` halves the coordinates.
        assert_eq!(multi_point.get_type(), FeatureType::Point);
        assert_eq!(*multi_point.geometry().unwrap(), [vec![Point::new(1, 2), Point::new(3, 4)]]);
        assert_eq!(multi_point.get_id(), Identifier::Uint(10));
        // Decoding closes a polygon's rings.
        assert_eq!(polygon.get_type(), FeatureType::Polygon);
        assert_eq!(
            *polygon.geometry().unwrap(),
            [vec![Point::new(0, 0), Point::new(50, 0), Point::new(50, 50), Point::new(0, 0)]]
        );
        assert_eq!(line.get_type(), FeatureType::LineString);

        // 8-bit integers read as integers; an absent value is none, and a
        // null among the properties.
        assert_eq!(polygon.get_value("small"), Some(Value::Int(-1)));
        assert_eq!(polygon.get_value("name"), Some(Value::String("n".to_owned())));
        assert_eq!(line.get_value("name"), None);
        assert_eq!(line.get_properties().get("name"), Some(&Value::Null));
        assert_eq!(line.get_value("nothing"), None);
    }

    /// Clones share one decoding, which threads may race to make.
    #[test]
    fn clones_share_the_decoded_layers() {
        let path = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/test/fixtures/map/issue12432/0-0-0.mlt");
        let tile = MltTile::new(Arc::new(std::fs::read(path).unwrap()), false);
        let layers: Vec<usize> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let tile = tile.clone();
                    scope.spawn(move || std::ptr::from_ref(tile.layer("admin").unwrap().0) as usize)
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect()
        });
        let here = std::ptr::from_ref(tile.layer("admin").unwrap().0) as usize;
        assert!(layers.iter().all(|&layer| layer == here));
    }

    fn rebase(corners: &[u32], first_vertex: u32, rings: &[(u32, u32)]) -> Option<Vec<u32>> {
        let mut out = vec![7];
        let ok = rebase_triangles(corners, first_vertex, rings, &mut Vec::new(), &mut out);
        assert_eq!(out[0], 7, "kept what was there");
        ok.then(|| out[1..].to_vec())
    }

    #[test]
    fn rebases_corners_past_closing_vertices() {
        // A ring of 4 stored vertices closed by decoding, an empty ring, a
        // ring stored closed, and a ring closed by decoding.
        let rings = [(4, 5), (0, 0), (3, 3), (3, 4)];
        assert_eq!(rebase(&[10, 13, 14, 16, 17, 19], 10, &rings), Some(vec![0, 3, 5, 7, 8, 10]));
        // One ring: its closing vertex moves nothing.
        assert_eq!(rebase(&[2, 3, 5], 2, &[(4, 5)]), Some(vec![0, 1, 3]));
    }

    #[test]
    fn rejects_corners_outside_the_feature() {
        let rings = [(4, 5), (3, 4)];
        // Past the feature's last vertex.
        assert_eq!(rebase(&[0, 1, 7], 0, &rings), None);
        // Before its first.
        assert_eq!(rebase(&[5, 6, 4], 5, &rings), None);
        assert_eq!(rebase(&[0, 1, 2], 0, &[]), None);
    }
}
