//! Writes features in a tile's coordinates.

use std::sync::Arc;

use geo_types::{Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use mln_style::feature::{Feature as TileFeature, PropertyMap, Value};

use super::TileId;
use super::geometry::{Feature, Geometry as VtGeometry, Line, Parts, Ring, Vertex};
use crate::TileFeatures;

/// A tile's features.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Tile {
    pub(super) features: TileFeatures,
    /// The points of the features it was written from.
    pub(super) num_points: u32,
}

impl Tile {
    /// Writes `features` in the coordinates of tile `id`, `extent` units
    /// wide, leaving out what simplifying with `tolerance` (in the unit
    /// square) drops.
    pub(super) fn new(features: &[Feature], id: TileId, extent: u16, tolerance: f64, line_metrics: bool) -> Self {
        let writer = Writer {
            z2: 2f64.powi(i32::from(id.z)),
            x: f64::from(id.x),
            y: f64::from(id.y),
            extent: f64::from(extent),
            tolerance,
            sq_tolerance: tolerance * tolerance,
            line_metrics,
        };
        let mut tile = Tile { features: Vec::with_capacity(features.len()), num_points: 0 };
        for feature in features {
            tile.num_points += feature.num_points;
            writer.write(feature, &mut tile.features);
        }
        tile
    }
}

struct Writer {
    z2: f64,
    x: f64,
    y: f64,
    extent: f64,
    tolerance: f64,
    sq_tolerance: f64,
    line_metrics: bool,
}

impl Writer {
    fn write(&self, feature: &Feature, out: &mut TileFeatures) {
        let mut properties = Arc::clone(&feature.properties);
        let geometry = match &feature.geometry {
            VtGeometry::Points(points) => match points[..] {
                [] => return,
                [p] => Geometry::Point(Point(self.transform(p))),
                _ => Geometry::MultiPoint(MultiPoint(points.iter().map(|&p| Point(self.transform(p))).collect())),
            },
            VtGeometry::Lines(lines) => {
                let mut kept: Parts<(&Line, LineString<i16>)> = lines
                    .iter()
                    .filter(|line| line.length > self.tolerance)
                    .map(|line| (line, self.line(line)))
                    .collect();
                match kept.len() {
                    0 => return,
                    1 => {
                        let (line, coords) = kept.swap_remove(0);
                        if self.line_metrics {
                            properties = Arc::new(with_clip_range(&feature.properties, line));
                        }
                        Geometry::LineString(coords)
                    }
                    _ => Geometry::MultiLineString(MultiLineString(kept.into_iter().map(|(_, l)| l).collect())),
                }
            }
            VtGeometry::Polygons(polygons) => {
                let mut polygons: Parts<Polygon<i16>> =
                    polygons.iter().filter_map(|rings| self.polygon(rings)).collect();
                match polygons.len() {
                    0 => return,
                    1 => Geometry::Polygon(polygons.swap_remove(0)),
                    _ => Geometry::MultiPolygon(MultiPolygon(polygons.into_vec())),
                }
            }
        };
        out.push(TileFeature { geometry: Some(geometry), properties, id: feature.id.clone() });
    }

    fn transform(&self, p: Vertex) -> Coord<i16> {
        Coord {
            x: ((p.x * self.z2 - self.x) * self.extent).round() as i16,
            y: ((p.y * self.z2 - self.y) * self.extent).round() as i16,
        }
    }

    /// The points of `points` that matter more than the tolerance.
    fn kept(&self, points: &[Vertex]) -> Vec<Coord<i16>> {
        points.iter().filter(|p| p.importance > self.sq_tolerance).map(|&p| self.transform(p)).collect()
    }

    fn line(&self, line: &Line) -> LineString<i16> {
        LineString(self.kept(&line.points))
    }

    /// The polygon of the rings larger than the tolerance, if any is.
    fn polygon(&self, rings: &[Ring]) -> Option<Polygon<i16>> {
        let mut kept = rings.iter().enumerate().filter(|(_, ring)| ring.area > self.sq_tolerance).map(|(i, ring)| {
            let mut coords = self.kept(&ring.points);
            rewind(&mut coords, i == 0);
            LineString(coords)
        });
        let exterior = kept.next()?;
        Some(Polygon::new(exterior, kept.collect()))
    }
}

/// `properties` with where along its whole line `line` starts and ends, as a
/// fraction of the line's length, in `mapbox_clip_start` and
/// `mapbox_clip_end`, unless they're there already. A line gradient reads
/// them.
fn with_clip_range(properties: &PropertyMap, line: &Line) -> PropertyMap {
    let mut properties = properties.clone();
    properties.entry("mapbox_clip_start".to_owned()).or_insert(Value::Double(line.start / line.length));
    properties.entry("mapbox_clip_end".to_owned()).or_insert(Value::Double(line.end / line.length));
    properties
}

/// Winds an outer ring clockwise and a hole counterclockwise, with y down,
/// as the vector tile spec wants, so that a fill layer can tell a polygon's
/// rings apart by their winding. This works on the rounded coordinates, so
/// that the winding holds for the coordinates the tile carries.
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
