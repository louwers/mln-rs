// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/tile.hpp.

use feature::{Feature, Identifier, PropertyMap, Value};
use geojson::FeatureCollection;
use geometry::{Geometry, LineString, LinearRing, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};

use crate::types::*;

/// `Tile`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub features: FeatureCollection<i16>,
    pub num_points: u32,
    pub num_simplified: u32,
}

/// `InternalTile`
#[derive(Clone, Debug)]
pub struct InternalTile {
    pub extent: u16,
    pub z: u8,
    pub x: u32,
    pub y: u32,

    pub z2: f64,
    pub tolerance: f64,
    pub sq_tolerance: f64,
    pub line_metrics: bool,

    pub source_features: VtFeatures,
    pub bbox: BoxD,

    pub tile: Tile,
}

impl InternalTile {
    pub fn new(source: &VtFeatures, z: u8, x: u32, y: u32, extent: u16, tolerance: f64, line_metrics: bool) -> Self {
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
            let props = &*feature.properties;
            let id = &feature.id;

            t.tile.num_points = t.tile.num_points.wrapping_add(feature.num_points);

            t.add_feature(geom, props, id);

            t.bbox.min.x = std_min(feature.bbox.min.x, t.bbox.min.x);
            t.bbox.min.y = std_min(feature.bbox.min.y, t.bbox.min.y);
            t.bbox.max.x = std_max(feature.bbox.max.x, t.bbox.max.x);
            t.bbox.max.y = std_max(feature.bbox.max.y, t.bbox.max.y);
        }
        t
    }

    fn emplace(&mut self, geometry: Geometry<i16>, properties: PropertyMap, id: &Identifier) {
        self.tile.features.push(Feature { geometry, properties, id: id.clone() });
    }

    /// The `addFeature` overloads.
    fn add_feature(&mut self, geom: &VtGeometry, props: &PropertyMap, id: &Identifier) {
        match geom {
            VtGeometry::Empty => self.emplace(Geometry::Empty, props.clone(), id),
            VtGeometry::Point(point) => {
                let p = self.transform_point(point);
                self.emplace(Geometry::Point(p), props.clone(), id);
            }
            VtGeometry::LineString(line) => {
                let new_line = self.transform_line_string(line);
                if !new_line.is_empty() {
                    if self.line_metrics {
                        let mut new_props = props.clone();
                        // `emplace` keeps an existing value.
                        new_props
                            .entry("mapbox_clip_start".to_owned())
                            .or_insert(Value::Double(line.seg_start / line.dist));
                        new_props
                            .entry("mapbox_clip_end".to_owned())
                            .or_insert(Value::Double(line.seg_end / line.dist));
                        self.emplace(Geometry::LineString(new_line), new_props, id);
                    } else {
                        self.emplace(Geometry::LineString(new_line), props.clone(), id);
                    }
                }
            }
            VtGeometry::Polygon(polygon) => {
                let new_polygon = self.transform_polygon(polygon);
                if !new_polygon.is_empty() {
                    self.emplace(Geometry::Polygon(new_polygon), props.clone(), id);
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
                    1 => self.emplace(Geometry::Point(new_multi.swap_remove(0)), props.clone(), id),
                    _ => self.emplace(Geometry::MultiPoint(new_multi), props.clone(), id),
                }
            }
            VtGeometry::MultiLineString(lines) => {
                let mut new_multi = self.transform_multi_line_string(lines);
                match new_multi.len() {
                    0 => {}
                    1 => self.emplace(Geometry::LineString(new_multi.swap_remove(0)), props.clone(), id),
                    _ => self.emplace(Geometry::MultiLineString(new_multi), props.clone(), id),
                }
            }
            VtGeometry::MultiPolygon(polygons) => {
                let mut new_multi = self.transform_multi_polygon(polygons);
                match new_multi.len() {
                    0 => {}
                    1 => self.emplace(Geometry::Polygon(new_multi.swap_remove(0)), props.clone(), id),
                    _ => self.emplace(Geometry::MultiPolygon(new_multi), props.clone(), id),
                }
            }
        }
    }

    fn transform_point(&mut self, p: &VtPoint) -> Point<i16> {
        self.tile.num_simplified = self.tile.num_simplified.wrapping_add(1);
        let x = f64::from(self.x);
        let y = f64::from(self.y);
        let extent = f64::from(self.extent);
        Point::new(((p.x * self.z2 - x) * extent).round() as i16, ((p.y * self.z2 - y) * extent).round() as i16)
    }

    fn transform_multi_point(&mut self, points: &VtMultiPoint) -> MultiPoint<i16> {
        let mut result = MultiPoint::with_capacity(points.len());
        for p in points {
            result.push(self.transform_point(p));
        }
        result
    }

    fn transform_line_string(&mut self, line: &VtLineString) -> LineString<i16> {
        let mut result = LineString::new();
        if line.dist > self.tolerance {
            result.reserve(line.len());
            for p in line.iter() {
                if p.z > self.sq_tolerance {
                    result.push(self.transform_point(p));
                }
            }
        }
        result
    }

    fn transform_linear_ring(&mut self, ring: &VtLinearRing) -> LinearRing<i16> {
        let mut result = LinearRing::new();
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

    fn transform_multi_line_string(&mut self, lines: &VtMultiLineString) -> MultiLineString<i16> {
        let mut result = MultiLineString::with_capacity(lines.len());
        for line in lines {
            if line.dist > self.tolerance {
                result.push(self.transform_line_string(line));
            }
        }
        result
    }

    fn transform_polygon(&mut self, rings: &VtPolygon) -> Polygon<i16> {
        let mut result = Polygon::with_capacity(rings.len());
        for ring in rings {
            if ring.area > self.sq_tolerance {
                result.push(self.transform_linear_ring(ring));
            }
        }
        result
    }

    fn transform_multi_polygon(&mut self, polygons: &VtMultiPolygon) -> MultiPolygon<i16> {
        let mut result = MultiPolygon::with_capacity(polygons.len());
        for polygon in polygons {
            let p = self.transform_polygon(polygon);
            if !p.is_empty() {
                result.push(p);
            }
        }
        result
    }
}
