//! Tiles deeper than their tileset's last zoom, cut from its tiles there.
//!
//! The tiles cut from one tile share its data as an [`Ancestor`], which
//! decodes each layer they read once, and keeps where each feature lies, so
//! that a cut tile clips only the features that reach it ([`CutData`]), as in
//! GL JS.

use std::borrow::Cow;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_style::feature::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_style::geometry::FeatureType;
use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;
use mln_util::hash::HashMap;
use mln_wire::{Reader, Wire, Writer, shared};
use parking_lot::Mutex;

use crate::TileError;
use crate::tile_data::{TileData, TileLayer};

/// How far a cut tile's geometry reaches past its edges, in its units, for
/// a source layer of `extent` units across: 128 of these, and no less than 16
/// pixels of a 512-pixel tile, as in GL JS, so that the ends that clipping
/// makes of lines stay off screen.
fn buffer(extent: u32) -> i64 {
    let extent = i64::from(extent.max(1));
    let units = ((extent + 16) / 32).max(128);
    units * i64::from(EXTENT) / extent
}

/// Where a tile lies in the tile of a shallower zoom it's cut from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    /// How many zooms deeper the tile is.
    pub dz: u8,
    /// The tile's column and row among the `2^dz` by `2^dz` tiles that the
    /// source tile holds.
    pub x: u32,
    pub y: u32,
}

impl Cut {
    /// Where `tile` lies in its ancestor at zoom `z`; `None` unless `tile`
    /// is deeper.
    #[must_use]
    pub fn of(tile: &CanonicalTileID, z: u8) -> Option<Cut> {
        let dz = tile.z.checked_sub(z).filter(|&dz| dz > 0)?;
        let mask = (1u64 << dz) - 1;
        Some(Cut { dz, x: (u64::from(tile.x) & mask) as u32, y: (u64::from(tile.y) & mask) as u32 })
    }

    /// The part of the source tile, in its units and widened to whole ones,
    /// that the cut tile and `buffer` around it cover.
    fn reach(&self, buffer: i64) -> Bounds {
        let size = 1i64 << self.dz;
        let edge = |at: u32, end: i64| (i64::from(at) + end) * i64::from(EXTENT);
        let (x0, y0) = ((edge(self.x, 0) - buffer).div_euclid(size), (edge(self.y, 0) - buffer).div_euclid(size));
        let ceil = |at: i64| (at + size - 1).div_euclid(size);
        let (x1, y1) = (ceil(edge(self.x, 1) + buffer), ceil(edge(self.y, 1) + buffer));
        Bounds { min: [x0, y0], max: [x1, y1] }
    }

    /// `geometry` of the source tile, as a feature of `kind` of a layer of
    /// `extent` units across, in the cut tile's units, clipped to the cut
    /// tile and its buffer as GL JS clips it: each point of a multipoint, each piece of a line that leaves and
    /// comes back, and each ring are geometries of their own, and a ring
    /// that leaves runs along the edge it left by. Empty if nothing is
    /// left.
    #[must_use]
    pub fn geometry(&self, kind: FeatureType, geometry: &GeometryCollection, extent: u32) -> GeometryCollection {
        let scale = (1u64 << self.dz) as f64;
        let offset = [f64::from(self.x) * f64::from(EXTENT), f64::from(self.y) * f64::from(EXTENT)];
        let rings: Vec<Vec<P>> = geometry
            .iter()
            .map(|ring| {
                ring.iter().map(|p| [f64::from(p.x) * scale - offset[0], f64::from(p.y) * scale - offset[1]]).collect()
            })
            .collect();
        let buffer = buffer(extent);
        let (start, end) = (-buffer as f64, (i64::from(EXTENT) + buffer) as f64);
        let clipped = clip_on_axis(clip_on_axis(rings, kind, start, end, 0), kind, start, end, 1);
        // Every coordinate is within the buffer, so an `i16`.
        let round = |p: P| Point::new(p[0].round() as i16, p[1].round() as i16);
        GeometryCollection::from(
            clipped
                .into_iter()
                .map(|ring| ring.into_iter().map(round).collect::<GeometryCoordinates>())
                .collect::<Vec<_>>(),
        )
    }
}

/// A point, its `x` then `y`, for code that works on either axis.
type P = [f64; 2];

/// Clips `rings` to `start..=end` on `axis` (0 for `x`).
fn clip_on_axis(rings: Vec<Vec<P>>, kind: FeatureType, start: f64, end: f64, axis: usize) -> Vec<Vec<P>> {
    match kind {
        FeatureType::Point => {
            rings.into_iter().flatten().filter(|p| (start..=end).contains(&p[axis])).map(|p| vec![p]).collect()
        }
        FeatureType::LineString => rings.iter().flat_map(|line| clip_line(line, start, end, axis, false)).collect(),
        FeatureType::Polygon => rings.iter().flat_map(|ring| clip_line(ring, start, end, axis, true)).collect(),
        FeatureType::Unknown => Vec::new(),
    }
}

/// The parts of `line` within `start..=end` on `axis`; for a `polygon`'s
/// ring, the one ring that runs along the bounds where it left them.
fn clip_line(line: &[P], start: f64, end: f64, axis: usize, polygon: bool) -> Vec<Vec<P>> {
    let Some(&last) = line.last() else { return Vec::new() };
    let mut slice: Vec<P> = Vec::new();
    let mut parts = Vec::new();
    for pair in line.windows(2) {
        let (p1, p2) = (pair[0], pair[1]);
        let (a, b) = (p1[axis], p2[axis]);
        let mut exited = false;
        if a < start {
            if b >= start {
                slice.push(intersection(p1, p2, start, axis));
            }
        } else if a > end {
            if b <= end {
                slice.push(intersection(p1, p2, end, axis));
            }
        } else {
            slice.push(p1);
        }
        if b < start && a >= start {
            slice.push(intersection(p1, p2, start, axis));
            exited = true;
        }
        if b > end && a <= end {
            slice.push(intersection(p1, p2, end, axis));
            exited = true;
        }
        if !polygon && exited {
            parts.push(std::mem::take(&mut slice));
        }
    }
    if (start..=end).contains(&last[axis]) {
        slice.push(last);
    }
    if polygon
        && let (Some(&first), Some(&closing)) = (slice.first(), slice.last())
        && first != closing
    {
        slice.push(first);
    }
    if !slice.is_empty() {
        parts.push(slice);
    }
    parts
}

/// The point of `p1`–`p2` whose `axis` coordinate is `at`.
fn intersection(p1: P, p2: P, at: f64, axis: usize) -> P {
    let other = 1 - axis;
    let t = (at - p1[axis]) / (p2[axis] - p1[axis]);
    let mut p = [0.0; 2];
    p[axis] = at;
    p[other] = p1[other] + (p2[other] - p1[other]) * t;
    p
}

/// A box in a tile's units, its edges included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    min: [i64; 2],
    max: [i64; 2],
}

impl Bounds {
    /// The bounds of `geometry`'s points; `None` if it has none.
    fn of(geometry: &GeometryCollection) -> Option<Bounds> {
        let mut points = geometry.iter().flatten();
        let first = points.next()?;
        let mut bounds = Bounds { min: [first.x.into(), first.y.into()], max: [first.x.into(), first.y.into()] };
        for p in points {
            for (axis, at) in [i64::from(p.x), i64::from(p.y)].into_iter().enumerate() {
                bounds.min[axis] = bounds.min[axis].min(at);
                bounds.max[axis] = bounds.max[axis].max(at);
            }
        }
        Some(bounds)
    }

    fn overlaps(&self, other: &Bounds) -> bool {
        (0..2).all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }
}

/// A tile's data at its tileset's last zoom, which the deeper tiles cut from
/// it share, and each of its layers that they have read, decoded once for
/// all of them and kept while any of them is.
pub struct Ancestor {
    data: TileData,
    /// By layer name; `None` for a layer the data lacks.
    layers: Mutex<HashMap<String, Option<Arc<AncestorLayer>>>>,
}

/// An ancestor's layer, decoded once for all the tiles cut from it.
struct AncestorLayer {
    name: String,
    /// The units across the tile of the layer as encoded.
    extent: u32,
    features: Vec<AncestorFeature>,
}

/// A feature of an [`AncestorLayer`], and where it lies.
struct AncestorFeature {
    kind: FeatureType,
    id: Identifier,
    properties: Arc<PropertyMap>,
    geometry: GeometryCollection,
    /// `None` for a feature without points.
    bounds: Option<Bounds>,
}

impl Ancestor {
    #[must_use]
    pub fn new(data: TileData) -> Self {
        Ancestor { data, layers: Mutex::default() }
    }

    #[must_use]
    pub fn data(&self) -> &TileData {
        &self.data
    }

    /// Layer `name`, decoded; `None` if the data has none.
    fn layer(&self, name: &str) -> Result<Option<Arc<AncestorLayer>>, TileError> {
        if let Some(layer) = self.layers.lock().get(name) {
            return Ok(layer.clone());
        }
        // Decoded unlocked: a cut tile reading another layer needn't wait.
        let layer = match self.data.layer(name)? {
            Some(layer) => Some(Arc::new(AncestorLayer::decode(&layer)?)),
            None => None,
        };
        Ok(self.layers.lock().entry(name.to_owned()).or_insert(layer).clone())
    }
}

impl AncestorLayer {
    fn decode(layer: &TileLayer<'_>) -> Result<AncestorLayer, TileError> {
        let features = (0..layer.len())
            .map(|index| {
                let feature = layer.feature(index)?;
                let geometry = feature.geometry()?;
                Ok(AncestorFeature {
                    kind: feature.get_type(),
                    id: feature.get_id(),
                    properties: Arc::new(feature.get_properties().into_owned()),
                    bounds: Bounds::of(&geometry),
                    geometry,
                })
            })
            .collect::<Result<_, TileError>>()?;
        Ok(AncestorLayer { name: layer.name().to_owned(), extent: layer.extent(), features })
    }
}

/// The `shared::Kind` of ancestors, which cross to a worker once for all the
/// tiles cut from them.
pub struct SharedAncestor;

impl shared::Kind for SharedAncestor {
    type Value = Ancestor;

    fn write(ancestor: &Ancestor, w: &mut Writer) {
        ancestor.data.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<Ancestor>> {
        TileData::read(r).map(|data| Arc::new(Ancestor::new(data)))
    }
}

/// A tile's data cut from its ancestor's, and each layer's cut once read.
pub struct CutData {
    ancestor: Arc<Ancestor>,
    cut: Cut,
    /// By layer name; `None` for a layer the ancestor lacks.
    layers: Mutex<HashMap<String, Option<Arc<CutLayer>>>>,
}

/// The features of an ancestor's layer that reach a cut tile. They keep what
/// they are read for, so that reading them needn't decode the ancestor's
/// layer again.
pub struct CutLayer {
    name: String,
    features: Vec<CutFeature>,
}

impl CutLayer {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn features(&self) -> &[CutFeature] {
        &self.features
    }
}

/// A feature of a [`CutLayer`], with its geometry in the cut tile.
pub struct CutFeature {
    kind: FeatureType,
    id: Identifier,
    /// Shared with the ancestor's feature.
    properties: Arc<PropertyMap>,
    geometry: GeometryCollection,
}

impl CutFeature {
    pub(crate) fn geometry(&self) -> &GeometryCollection {
        &self.geometry
    }
}

impl GeometryTileFeature for CutFeature {
    fn get_type(&self) -> FeatureType {
        self.kind
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.properties.get(key).filter(|value| !matches!(value, Value::Null)).cloned()
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&*self.properties)
    }

    fn get_id(&self) -> Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_util::error::Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}

impl CutData {
    #[must_use]
    pub fn new(ancestor: Arc<Ancestor>, cut: Cut) -> Self {
        CutData { ancestor, cut, layers: Mutex::default() }
    }

    #[must_use]
    pub fn ancestor(&self) -> &Arc<Ancestor> {
        &self.ancestor
    }

    #[must_use]
    pub fn cut(&self) -> Cut {
        self.cut
    }

    pub(crate) fn layer(&self, name: &str) -> Result<Option<Arc<CutLayer>>, TileError> {
        if let Some(layer) = self.layers.lock().get(name) {
            return Ok(layer.clone());
        }
        let layer = self.ancestor.layer(name)?.map(|layer| Arc::new(self.cut_layer(&layer)));
        Ok(self.layers.lock().entry(name.to_owned()).or_insert(layer).clone())
    }

    /// The features of the ancestor's `layer` that reach the cut tile.
    fn cut_layer(&self, layer: &AncestorLayer) -> CutLayer {
        let reach = self.cut.reach(buffer(layer.extent));
        let features = layer
            .features
            .iter()
            .filter(|feature| feature.bounds.is_some_and(|bounds| bounds.overlaps(&reach)))
            .filter_map(|feature| {
                let geometry = self.cut.geometry(feature.kind, &feature.geometry, layer.extent);
                (!geometry.is_empty()).then(|| CutFeature {
                    kind: feature.kind,
                    id: feature.id.clone(),
                    properties: Arc::clone(&feature.properties),
                    geometry,
                })
            })
            .collect();
        CutLayer { name: layer.name.clone(), features }
    }

    pub(crate) fn has_layer(&self, name: &str) -> Result<bool, TileError> {
        self.ancestor.data.has_layer(name)
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        shared::write::<SharedAncestor>(&self.ancestor, w);
        w.push(self.cut.dz);
        self.cut.x.write(w);
        self.cut.y.write(w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Self> {
        let ancestor = shared::read::<SharedAncestor>(r)?;
        let cut = Cut { dz: r.byte()?, x: u32::read(r)?, y: u32::read(r)? };
        Some(CutData::new(ancestor, cut))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(kind: FeatureType, rings: &[&[(i32, i32)]]) -> Vec<Vec<(i32, i32)>> {
        let rings =
            rings.iter().map(|ring| ring.iter().map(|&(x, y)| [f64::from(x), f64::from(y)]).collect()).collect();
        let clipped = clip_on_axis(clip_on_axis(rings, kind, 0.0, 10.0, 0), kind, 0.0, 10.0, 1);
        clipped.into_iter().map(|ring| ring.into_iter().map(|p| (p[0] as i32, p[1] as i32)).collect()).collect()
    }

    #[test]
    fn a_tile_finds_its_place_in_an_ancestor() {
        assert_eq!(Cut::of(&CanonicalTileID::new(16, 4 * 10 + 3, 4 * 20 + 1), 14), Some(Cut { dz: 2, x: 3, y: 1 }));
        assert_eq!(Cut::of(&CanonicalTileID::new(14, 10, 20), 14), None);
    }

    #[test]
    fn the_buffer_is_128_units_of_the_layer_or_a_32nd_of_the_tile() {
        let extent = i64::from(EXTENT);
        assert_eq!(buffer(4096), extent / 32);
        assert_eq!(buffer(8192), extent / 32);
        assert_eq!(buffer(1024), extent / 8);
    }

    #[test]
    fn a_cut_reaches_its_buffer_in_the_ancestor() {
        let quarter = i64::from(EXTENT) / 4;
        let reach = Cut { dz: 2, x: 1, y: 0 }.reach(256);
        assert_eq!(reach, Bounds { min: [quarter - 64, -64], max: [2 * quarter + 64, quarter + 64] });
    }

    #[test]
    fn a_line_through_the_tile_keeps_its_middle() {
        assert_eq!(clip(FeatureType::LineString, &[&[(-5, 5), (15, 5)]]), vec![vec![(0, 5), (10, 5)]]);
    }

    #[test]
    fn a_line_leaving_and_coming_back_splits() {
        let parts = clip(FeatureType::LineString, &[&[(2, 2), (2, 20), (8, 20), (8, 2)]]);
        assert_eq!(parts, vec![vec![(2, 2), (2, 10)], vec![(8, 10), (8, 2)]]);
    }

    #[test]
    fn a_ring_around_the_tile_becomes_its_bounds() {
        let ring = clip(FeatureType::Polygon, &[&[(-5, -5), (15, -5), (15, 15), (-5, 15), (-5, -5)]]);
        assert_eq!(ring, vec![vec![(10, 0), (10, 10), (0, 10), (0, 0), (10, 0)]]);
    }

    #[test]
    fn each_point_inside_is_a_geometry_of_its_own() {
        let points = clip(FeatureType::Point, &[&[(1, 1), (20, 1), (9, 9)]]);
        assert_eq!(points, vec![vec![(1, 1)], vec![(9, 9)]]);
    }

    #[test]
    fn a_cut_scales_and_moves_its_part_of_the_ancestor() {
        let half = (EXTENT / 2) as i16;
        let cut = Cut { dz: 1, x: 1, y: 1 };
        let line =
            GeometryCollection::from(vec![vec![Point::new(half + 10, half + 20), Point::new(half + 30, half + 20)]]);
        let geometry = cut.geometry(FeatureType::LineString, &line, 4096);
        assert_eq!(geometry[0], vec![Point::new(20, 40), Point::new(60, 40)]);
        let elsewhere = GeometryCollection::from(vec![vec![Point::new(10, 10), Point::new(30, 10)]]);
        assert!(cut.geometry(FeatureType::LineString, &elsewhere, 4096).is_empty());
    }

    #[test]
    fn the_tiles_cut_from_an_ancestor_share_its_decoded_layer() {
        let (half, quarter) = ((EXTENT / 2) as i16, (EXTENT / 4) as i16);
        let road = feature::Feature {
            geometry: Some(geo_types::Geometry::LineString(geo_types::LineString::from(vec![
                (half - 100, quarter),
                (half + 100, quarter),
            ]))),
            properties: Arc::new(PropertyMap::new()),
            id: Identifier::Uint(1),
        };
        let ancestor = Arc::new(Ancestor::new(TileData::Features(Arc::new(vec![road]))));
        let decoded = ancestor.layer("roads").unwrap().unwrap();
        let [west, east] = [0, 1].map(|x| CutData::new(Arc::clone(&ancestor), Cut { dz: 1, x, y: 0 }));
        let [west, east] = [west, east].map(|cut| cut.layer("roads").unwrap().unwrap());

        assert!(Arc::ptr_eq(&decoded, &ancestor.layer("roads").unwrap().unwrap()), "decoded once");
        assert!(Arc::ptr_eq(&west.features()[0].properties, &east.features()[0].properties));
        assert_eq!(
            west.features()[0].geometry()[0],
            vec![Point::new(EXTENT as i16 - 200, half), Point::new(EXTENT as i16 + 200, half)]
        );
    }
}
