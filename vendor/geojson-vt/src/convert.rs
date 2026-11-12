// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/convert.hpp.

use std::f64::consts::PI;

use feature::Identifier;
use geojson::FeatureCollection;
use geometry::{Geometry, LineString, LinearRing, Point};

use crate::simplify::simplify;
use crate::types::{VtFeature, VtFeatures, VtGeometry, VtLineString, VtLinearRing, VtPoint, VtPolygon};

/// `project` (the visitor that converts to projected `vt_geometry`).
pub struct Project {
    pub tolerance: f64,
}

impl Project {
    #[must_use]
    pub fn point(&self, p: &Point<f64>) -> VtPoint {
        let sine = (p.y * PI / 180.0).sin();
        let x = p.x / 360.0 + 0.5;
        // std::max(std::min(y, 1.0), 0.0)
        let y = 0.5 - 0.25 * ((1.0 + sine) / (1.0 - sine)).ln() / PI;
        let y = if 1.0 < y { 1.0 } else { y };
        let y = if y < 0.0 { 0.0 } else { y };
        VtPoint::new(x, y, 0.0)
    }

    #[must_use]
    pub fn line_string(&self, points: &LineString<f64>) -> VtLineString {
        let mut result = VtLineString::default();
        let len = points.len();

        if len == 0 {
            return result;
        }

        result.reserve(len);

        for p in points {
            result.push(self.point(p));
        }

        for i in 0..len - 1 {
            let a = result[i];
            let b = result[i + 1];
            result.dist += (b.x - a.x).hypot(b.y - a.y);
        }

        simplify(&mut result.points, self.tolerance);

        result.seg_start = 0.0;
        result.seg_end = result.dist;

        result
    }

    #[must_use]
    pub fn linear_ring(&self, ring: &LinearRing<f64>) -> VtLinearRing {
        let mut result = VtLinearRing::default();
        let len = ring.len();

        if len == 0 {
            return result;
        }

        result.reserve(len);

        for p in ring {
            result.push(self.point(p));
        }

        let mut area = 0.0;

        for i in 0..len - 1 {
            let a = result[i];
            let b = result[i + 1];
            area += a.x * b.y - b.x * a.y;
        }
        result.area = (area / 2.0).abs();

        simplify(&mut result.points, self.tolerance);

        result
    }

    #[must_use]
    pub fn polygon(&self, rings: &[LinearRing<f64>]) -> VtPolygon {
        rings.iter().map(|r| self.linear_ring(r)).collect()
    }

    #[must_use]
    pub fn geometry(&self, geometry: &Geometry<f64>) -> VtGeometry {
        match geometry {
            Geometry::Empty => VtGeometry::Empty,
            Geometry::Point(p) => VtGeometry::Point(self.point(p)),
            Geometry::MultiPoint(points) => VtGeometry::MultiPoint(points.iter().map(|p| self.point(p)).collect()),
            Geometry::LineString(points) => VtGeometry::LineString(self.line_string(points)),
            Geometry::MultiLineString(lines) => {
                VtGeometry::MultiLineString(lines.iter().map(|l| self.line_string(l)).collect())
            }
            Geometry::Polygon(rings) => VtGeometry::Polygon(self.polygon(rings)),
            Geometry::MultiPolygon(polygons) => {
                VtGeometry::MultiPolygon(polygons.iter().map(|p| self.polygon(p)).collect())
            }
            Geometry::GeometryCollection(geometries) => {
                VtGeometry::GeometryCollection(geometries.iter().map(|g| self.geometry(g)).collect())
            }
        }
    }
}

/// `convert(features, tolerance, generateId)`
#[must_use]
pub fn convert(features: &FeatureCollection<f64>, tolerance: f64, generate_id: bool) -> VtFeatures {
    let mut projected = VtFeatures::with_capacity(features.len());
    let mut gen_id: u64 = 0;
    for feature in features {
        let mut feature_id = feature.id.clone();
        if generate_id {
            feature_id = Identifier::Uint(gen_id);
            gen_id = gen_id.wrapping_add(1);
        }
        projected.push(VtFeature::with_properties(
            Project { tolerance }.geometry(&feature.geometry),
            &feature.properties,
            feature_id,
        ));
    }
    projected
}
