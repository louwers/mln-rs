//! Puts GeoJSON features in the unit square, and ranks their lines' points
//! for simplification.

use std::sync::Arc;

use geo_types::{Coord, Geometry as GeoGeometry};
use mln_style::feature::{Identifier, PropertyMap};
use smallvec::smallvec;

use super::geometry::{Feature, Geometry, Line, Ring, Vertex};
use super::simplify::simplify;
use crate::sources::geojson::Coordinates;

/// The features of `features`, whose positions are in `coordinates`, put in
/// the unit square and simplified with `tolerance` (in the unit square). A
/// feature without geometry or points has none. A geometry collection
/// becomes a feature for each of its geometries, and, with `line_metrics`,
/// a multi line string one for each line, so that each line has its own
/// metrics.
pub(super) fn convert(
    features: &[mln_style::feature::Feature<f64>],
    tolerance: f64,
    line_metrics: bool,
    coordinates: Coordinates,
) -> Vec<Feature> {
    let mut converted = Vec::with_capacity(features.len());
    for f in features {
        if let Some(geometry) = &f.geometry {
            let properties = Arc::new(f.properties.clone());
            Converter {
                tolerance,
                line_metrics,
                coordinates,
                properties: &properties,
                id: &f.id,
                features: &mut converted,
            }
            .geometry(geometry);
        }
    }
    converted
}

struct Converter<'a> {
    tolerance: f64,
    line_metrics: bool,
    coordinates: Coordinates,
    properties: &'a Arc<PropertyMap>,
    id: &'a Identifier,
    features: &'a mut Vec<Feature>,
}

impl Converter<'_> {
    fn geometry(&mut self, geometry: &GeoGeometry) {
        match geometry {
            GeoGeometry::Point(p) => self.push(Geometry::Points(smallvec![self.vertex(p.0)])),
            GeoGeometry::MultiPoint(points) => {
                self.push(Geometry::Points(points.iter().map(|p| self.vertex(p.0)).collect()));
            }
            GeoGeometry::Line(line) => self.push(Geometry::Lines(smallvec![self.line(&[line.start, line.end])])),
            GeoGeometry::LineString(line) => self.push(Geometry::Lines(smallvec![self.line(&line.0)])),
            GeoGeometry::MultiLineString(lines) if self.line_metrics => {
                for line in lines {
                    self.push(Geometry::Lines(smallvec![self.line(&line.0)]));
                }
            }
            GeoGeometry::MultiLineString(lines) => {
                self.push(Geometry::Lines(lines.iter().map(|line| self.line(&line.0)).collect()));
            }
            GeoGeometry::Polygon(polygon) => self.push(Geometry::Polygons(smallvec![self.polygon(polygon)])),
            GeoGeometry::MultiPolygon(polygons) => {
                self.push(Geometry::Polygons(polygons.iter().map(|p| self.polygon(p)).collect()));
            }
            GeoGeometry::Rect(rect) => self.push(Geometry::Polygons(smallvec![self.polygon(&rect.to_polygon())])),
            GeoGeometry::Triangle(triangle) => {
                self.push(Geometry::Polygons(smallvec![self.polygon(&triangle.to_polygon())]));
            }
            GeoGeometry::GeometryCollection(geometries) => {
                for geometry in geometries {
                    self.geometry(geometry);
                }
            }
        }
    }

    fn push(&mut self, geometry: Geometry) {
        if let Some(feature) = Feature::new(geometry, Arc::clone(self.properties), self.id.clone()) {
            self.features.push(feature);
        }
    }

    fn line(&self, coords: &[Coord]) -> Line {
        let mut points: Vec<Vertex> = coords.iter().map(|&c| self.vertex(c)).collect();
        let length = points.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
        simplify(&mut points, self.tolerance);
        Line { points, length, start: 0.0, end: length }
    }

    /// A ring of `coords`, closed if it isn't.
    fn ring(&self, coords: &[Coord]) -> Ring {
        let mut points: Vec<Vertex> = coords.iter().map(|&c| self.vertex(c)).collect();
        if let (Some(&first), Some(&last)) = (points.first(), points.last())
            && !first.same_position(last)
        {
            points.push(first);
        }
        let area = points.windows(2).map(|w| w[0].x * w[1].y - w[1].x * w[0].y).sum::<f64>().abs() / 2.0;
        simplify(&mut points, self.tolerance);
        Ring { points, area }
    }

    fn polygon(&self, polygon: &geo_types::Polygon) -> Vec<Ring> {
        std::iter::once(polygon.exterior()).chain(polygon.interiors()).map(|ring| self.ring(&ring.0)).collect()
    }

    fn vertex(&self, c: Coord) -> Vertex {
        let p = self.coordinates.project(c);
        Vertex::new(p.x, p.y, 0.0)
    }
}
