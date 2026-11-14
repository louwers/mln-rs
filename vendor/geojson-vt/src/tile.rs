// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/tile.hpp.

use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};

use crate::types::{
    BoxD, VtFeatures, VtGeometry, VtLineString, VtLinearRing, VtMultiLineString, VtMultiPoint, VtMultiPolygon, VtPoint,
    VtPolygon,
};

/// A tile's features. A feature cut from a source feature shares its
/// property map.
pub type TileFeatures = Vec<Feature<i16, Arc<PropertyMap>>>;

/// `Tile`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub features: TileFeatures,
    pub(crate) num_points: u32,
    pub(crate) num_simplified: u32,
}

/// `InternalTile`
#[derive(Clone, Debug)]
pub(crate) struct InternalTile {
    pub(crate) extent: u16,
    pub(crate) z: u8,
    pub(crate) x: u32,
    pub(crate) y: u32,

    pub(crate) z2: f64,
    pub(crate) tolerance: f64,
    pub(crate) sq_tolerance: f64,
    pub(crate) line_metrics: bool,

    pub(crate) source_features: VtFeatures,
    pub(crate) bbox: BoxD,

    pub(crate) tile: Tile,
}

impl InternalTile {
    #[must_use]
    pub(crate) fn new(
        source: &VtFeatures,
        z: u8,
        x: u32,
        y: u32,
        extent: u16,
        tolerance: f64,
        line_metrics: bool,
    ) -> Self {
        let mut t = InternalTile {
            extent,
            z,
            x,
            y,
            z2: 2.0f64.powf(f64::from(z)),
            tolerance,
            sq_tolerance: tolerance * tolerance,
            line_metrics,
            source_features: VtFeatures::new(),
            bbox: BoxD::INITIAL,
            tile: Tile::default(),
        };

        t.tile.features.reserve(source.len());
        for feature in source {
            let geom = &feature.geometry;
            let props = &feature.properties;
            let id = &feature.id;

            t.tile.num_points = t.tile.num_points.wrapping_add(feature.num_points);

            t.add_feature(geom, props, id);

            t.bbox.min.x = f64::min(feature.bbox.min.x, t.bbox.min.x);
            t.bbox.min.y = f64::min(feature.bbox.min.y, t.bbox.min.y);
            t.bbox.max.x = f64::max(feature.bbox.max.x, t.bbox.max.x);
            t.bbox.max.y = f64::max(feature.bbox.max.y, t.bbox.max.y);
        }
        t
    }

    fn emplace(&mut self, geometry: Option<Geometry<i16>>, properties: Arc<PropertyMap>, id: &Identifier) {
        self.tile.features.push(Feature { geometry, properties, id: id.clone() });
    }

    /// The `addFeature` overloads.
    fn add_feature(&mut self, geom: &VtGeometry, props: &Arc<PropertyMap>, id: &Identifier) {
        match geom {
            VtGeometry::Empty => self.emplace(None, Arc::clone(props), id),
            VtGeometry::Point(point) => {
                let p = self.transform_point(point);
                self.emplace(Some(Geometry::Point(Point(p))), Arc::clone(props), id);
            }
            VtGeometry::LineString(line) => {
                let new_line = self.transform_line_string(line);
                if !new_line.0.is_empty() {
                    if self.line_metrics {
                        let mut new_props = PropertyMap::clone(props);
                        // `emplace` keeps an existing value.
                        new_props
                            .entry("mapbox_clip_start".to_owned())
                            .or_insert(Value::Double(line.seg_start / line.dist));
                        new_props
                            .entry("mapbox_clip_end".to_owned())
                            .or_insert(Value::Double(line.seg_end / line.dist));
                        self.emplace(Some(Geometry::LineString(new_line)), Arc::new(new_props), id);
                    } else {
                        self.emplace(Some(Geometry::LineString(new_line)), Arc::clone(props), id);
                    }
                }
            }
            VtGeometry::Polygon(polygon) => {
                let new_polygon = self.transform_polygon(polygon);
                if !new_polygon.is_empty() {
                    self.emplace(Some(Geometry::Polygon(to_polygon(new_polygon))), Arc::clone(props), id);
                }
            }
            VtGeometry::GeometryCollection(collection) => {
                for geom in collection {
                    self.add_feature(geom, props, id);
                }
            }
            VtGeometry::MultiPoint(points) => {
                let mut new_multi = self.transform_multi_point(points);
                match new_multi.len() {
                    0 => {}
                    1 => self.emplace(Some(Geometry::Point(new_multi.swap_remove(0))), Arc::clone(props), id),
                    _ => self.emplace(Some(Geometry::MultiPoint(MultiPoint(new_multi))), Arc::clone(props), id),
                }
            }
            VtGeometry::MultiLineString(lines) => {
                let mut new_multi = self.transform_multi_line_string(lines);
                match new_multi.len() {
                    0 => {}
                    1 => self.emplace(Some(Geometry::LineString(new_multi.swap_remove(0))), Arc::clone(props), id),
                    _ => {
                        self.emplace(
                            Some(Geometry::MultiLineString(MultiLineString(new_multi))),
                            Arc::clone(props),
                            id,
                        );
                    }
                }
            }
            VtGeometry::MultiPolygon(polygons) => {
                let mut new_multi = self.transform_multi_polygon(polygons);
                match new_multi.len() {
                    0 => {}
                    1 => self.emplace(
                        Some(Geometry::Polygon(to_polygon(new_multi.swap_remove(0)))),
                        Arc::clone(props),
                        id,
                    ),
                    _ => self.emplace(
                        Some(Geometry::MultiPolygon(MultiPolygon(new_multi.into_iter().map(to_polygon).collect()))),
                        Arc::clone(props),
                        id,
                    ),
                }
            }
        }
    }

    fn transform_point(&mut self, p: &VtPoint) -> Coord<i16> {
        self.tile.num_simplified = self.tile.num_simplified.wrapping_add(1);
        let x = f64::from(self.x);
        let y = f64::from(self.y);
        let extent = f64::from(self.extent);
        Coord { x: ((p.x * self.z2 - x) * extent).round() as i16, y: ((p.y * self.z2 - y) * extent).round() as i16 }
    }

    fn transform_multi_point(&mut self, points: &VtMultiPoint) -> Vec<Point<i16>> {
        let mut result = Vec::with_capacity(points.len());
        for p in points {
            result.push(Point(self.transform_point(p)));
        }
        result
    }

    fn transform_line_string(&mut self, line: &VtLineString) -> LineString<i16> {
        let mut result = Vec::new();
        if line.dist > self.tolerance {
            result.reserve(line.len());
            for p in line.iter() {
                if p.z > self.sq_tolerance {
                    result.push(self.transform_point(p));
                }
            }
        }
        LineString(result)
    }

    fn transform_linear_ring(&mut self, ring: &VtLinearRing) -> Vec<Coord<i16>> {
        let mut result = Vec::new();
        if ring.area > self.sq_tolerance {
            result.reserve(ring.len());
            for p in ring.iter() {
                if p.z > self.sq_tolerance {
                    result.push(self.transform_point(p));
                }
            }
        }
        result
    }

    fn transform_multi_line_string(&mut self, lines: &VtMultiLineString) -> Vec<LineString<i16>> {
        let mut result = Vec::with_capacity(lines.len());
        for line in lines {
            if line.dist > self.tolerance {
                result.push(self.transform_line_string(line));
            }
        }
        result
    }

    /// The polygon's rings that are larger than the tolerance.
    fn transform_polygon(&mut self, rings: &VtPolygon) -> Vec<LineString<i16>> {
        let mut result = Vec::with_capacity(rings.len());
        for (i, ring) in rings.iter().enumerate() {
            if ring.area > self.sq_tolerance {
                let mut ring = self.transform_linear_ring(ring);
                rewind(&mut ring, i == 0);
                result.push(LineString(ring));
            }
        }
        result
    }

    fn transform_multi_polygon(&mut self, polygons: &VtMultiPolygon) -> Vec<Vec<LineString<i16>>> {
        let mut result = Vec::with_capacity(polygons.len());
        for polygon in polygons {
            let p = self.transform_polygon(polygon);
            if !p.is_empty() {
                result.push(p);
            }
        }
        result
    }
}

/// The polygon of a non-empty list of rings: the first is its exterior.
fn to_polygon(rings: Vec<LineString<i16>>) -> Polygon<i16> {
    let mut rings = rings.into_iter();
    let exterior = rings.next().unwrap_or_else(|| LineString(Vec::new()));
    Polygon::new(exterior, rings.collect())
}

/// Winds an outer ring clockwise and a hole counterclockwise, with y down,
/// as the vector tile spec wants, so that a fill layer can tell a polygon's
/// rings apart by their winding. geojson-vt JS's `rewind` does the same
/// before rounding; this works on the rounded coordinates, so that the
/// winding holds for the coordinates the tile carries.
fn rewind(ring: &mut [Coord<i16>], outer: bool) {
    let mut area = 0i64;
    let mut j = ring.len().wrapping_sub(1);
    for (i, p) in ring.iter().enumerate() {
        let q = ring[j];
        area += (i64::from(p.x) - i64::from(q.x)) * (i64::from(p.y) + i64::from(q.y));
        j = i;
    }
    if (area > 0) == outer {
        ring.reverse();
    }
}
