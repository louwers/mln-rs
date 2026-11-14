// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/convert.hpp.

use std::f64::consts::PI;

use feature::{Identifier, PropertyMap};
use geo_traits::{
    CoordTrait, GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait, LineTrait, MultiLineStringTrait,
    MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait, RectTrait, TriangleTrait,
};

use crate::simplify::simplify;
use crate::types::{VtFeature, VtFeatures, VtGeometry, VtLineString, VtLinearRing, VtPoint, VtPolygon};

/// `project` (the visitor that converts to projected `vt_geometry`).
pub(crate) struct Project {
    pub(crate) tolerance: f64,
}

impl Project {
    #[must_use]
    pub(crate) fn point(p: &impl CoordTrait<T = f64>) -> VtPoint {
        let sine = (p.y() * PI / 180.0).sin();
        let x = p.x() / 360.0 + 0.5;
        // std::max(std::min(y, 1.0), 0.0)
        let y = 0.5 - 0.25 * ((1.0 + sine) / (1.0 - sine)).ln() / PI;
        let y = if 1.0 < y { 1.0 } else { y };
        let y = if y < 0.0 { 0.0 } else { y };
        VtPoint::new(x, y, 0.0)
    }

    #[must_use]
    pub(crate) fn line_string<C: CoordTrait<T = f64>>(&self, points: impl ExactSizeIterator<Item = C>) -> VtLineString {
        let mut result = VtLineString::default();
        let len = points.len();

        if len == 0 {
            return result;
        }

        result.reserve(len);

        for p in points {
            result.push(Self::point(&p));
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
    pub(crate) fn linear_ring<C: CoordTrait<T = f64>>(&self, ring: impl ExactSizeIterator<Item = C>) -> VtLinearRing {
        let mut result = VtLinearRing::default();
        if ring.len() == 0 {
            return result;
        }

        result.reserve(ring.len() + 1);

        for p in ring {
            result.push(Self::point(&p));
        }
        // GeoJSON requires rings to be closed. One that isn't is closed
        // here, so that an outline draws its last edge.
        let (first, last) = (result[0], result[result.len() - 1]);
        if first != last {
            result.push(first);
        }
        let len = result.len();

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
    pub(crate) fn polygon(&self, polygon: &impl PolygonTrait<T = f64>) -> VtPolygon {
        polygon.exterior().into_iter().chain(polygon.interiors()).map(|r| self.linear_ring(r.coords())).collect()
    }

    /// The corners of a rectangle or triangle as a polygon's ring.
    fn corners(&self, corners: &[(f64, f64)]) -> VtGeometry {
        VtGeometry::Polygon(vec![self.linear_ring(corners.iter().copied())])
    }

    #[must_use]
    pub(crate) fn geometry(&self, geometry: &impl GeometryTrait<T = f64>) -> VtGeometry {
        match geometry.as_type() {
            GeometryType::Point(p) => p.coord().map_or(VtGeometry::Empty, |c| VtGeometry::Point(Self::point(&c))),
            GeometryType::MultiPoint(points) => {
                VtGeometry::MultiPoint(points.points().filter_map(|p| p.coord().map(|c| Self::point(&c))).collect())
            }
            GeometryType::Line(line) => VtGeometry::LineString(self.line_string(line.coords().into_iter())),
            GeometryType::LineString(points) => VtGeometry::LineString(self.line_string(points.coords())),
            GeometryType::MultiLineString(lines) => {
                VtGeometry::MultiLineString(lines.line_strings().map(|l| self.line_string(l.coords())).collect())
            }
            GeometryType::Polygon(polygon) => VtGeometry::Polygon(self.polygon(polygon)),
            GeometryType::MultiPolygon(polygons) => {
                VtGeometry::MultiPolygon(polygons.polygons().map(|p| self.polygon(&p)).collect())
            }
            GeometryType::Rect(rect) => {
                let (min, max) = (rect.min().x_y(), rect.max().x_y());
                self.corners(&[min, (max.0, min.1), max, (min.0, max.1)])
            }
            GeometryType::Triangle(triangle) => self.corners(&triangle.coords().map(|c| c.x_y())),
            GeometryType::GeometryCollection(geometries) => {
                VtGeometry::GeometryCollection(geometries.geometries().map(|g| self.geometry(&g)).collect())
            }
        }
    }
}

/// `convert(features, tolerance, generateId)`
#[must_use]
pub(crate) fn convert<'a, G: GeometryTrait<T = f64> + 'a>(
    features: impl IntoIterator<Item = (Option<&'a G>, &'a PropertyMap, &'a Identifier)>,
    tolerance: f64,
    generate_id: bool,
) -> VtFeatures {
    let features = features.into_iter();
    let mut projected = VtFeatures::with_capacity(features.size_hint().0);
    let mut gen_id: u64 = 0;
    for (geometry, properties, id) in features {
        let mut feature_id = id.clone();
        if generate_id {
            feature_id = Identifier::Uint(gen_id);
            gen_id = gen_id.wrapping_add(1);
        }
        let geometry = geometry.map_or(VtGeometry::Empty, |g| Project { tolerance }.geometry(g));
        projected.push(VtFeature::with_properties(geometry, properties, feature_id));
    }
    projected
}
