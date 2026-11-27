//! Moves GeoJSON between lng/lat and a registered projection's world square,
//! where a map drawn in the projection tiles and clusters it.

use geo_types::{Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};

use crate::crs::CrsMapping;
use crate::feature::Feature;

/// Features in lng/lat, or in a world square.
type Features = Vec<Feature<f64>>;

/// The longitude step, in degrees, between the positions that an edge along
/// a limit of the latitude range gets, so that it follows the parallel in
/// the projection instead of cutting across it.
const PARALLEL_STEP: f64 = 10.0;

/// A turn around the pole, in degrees of longitude, past which an edge along
/// a parallel traces it again.
const TURN: f64 = 360.0;

/// `features` with every position moved from lng/lat to its place in the
/// world square of `crs`, which the projection converts in one batch.
///
/// The positions of lines and rings that reach past the projection's
/// latitude range move onto it first, and an edge that then runs along it
/// gets a position every 10 degrees of longitude, over a turn at most, so
/// that what lies past it, such as the far pole of a polar projection, ends
/// up along it, off the map. Points move as they are. A position that isn't
/// a lng/lat, or that the projection has no finite place for, is left out,
/// and a geometry left without positions with it.
#[must_use]
pub fn features_to_world(mut features: Features, crs: &CrsMapping) -> Features {
    let range = OntoLatitudeRange::new(crs.latitude_range());
    for feature in &mut features {
        feature.geometry = feature.geometry.take().and_then(|geometry| map_geometry(geometry, &range));
    }
    let mut finite = true;
    convert(&mut features, |lng_lats, places| {
        crs.world_from_lng_lats(lng_lats, places);
        finite = places.iter().all(|place| place[0].is_finite() && place[1].is_finite());
    });
    if !finite {
        for feature in &mut features {
            feature.geometry = feature.geometry.take().and_then(|geometry| map_geometry(geometry, &Finite));
        }
    }
    features
}

/// `features` with every position moved from its place in the world square
/// of `crs` back to lng/lat, in one batch.
#[must_use]
pub fn features_from_world(mut features: Features, crs: &CrsMapping) -> Features {
    convert(&mut features, |places, lng_lats| crs.lng_lats_from_world(places, lng_lats));
    features
}

/// Converts every position of `features` in one call of `convert`, which
/// writes what each position of its first argument becomes into its second.
fn convert(features: &mut Features, convert: impl FnOnce(&[[f64; 2]], &mut [[f64; 2]])) {
    let mut positions = Vec::new();
    for geometry in features.iter_mut().filter_map(|feature| feature.geometry.as_mut()) {
        for_each_position(geometry, &mut |position| positions.push([position.x, position.y]));
    }
    let mut converted = vec![[0.0; 2]; positions.len()];
    convert(&positions, &mut converted);
    let mut converted = converted.into_iter();
    for geometry in features.iter_mut().filter_map(|feature| feature.geometry.as_mut()) {
        for_each_position(geometry, &mut |position| {
            if let Some([x, y]) = converted.next() {
                *position = Coord { x, y };
            }
        });
    }
}

/// Calls `visit` with each position of `geometry`, in order.
fn for_each_position(geometry: &mut Geometry, visit: &mut impl FnMut(&mut Coord)) {
    match geometry {
        Geometry::Point(point) => visit(&mut point.0),
        Geometry::Line(line) => {
            visit(&mut line.start);
            visit(&mut line.end);
        }
        Geometry::LineString(line) => line.0.iter_mut().for_each(visit),
        Geometry::Polygon(polygon) => for_each_polygon_position(polygon, visit),
        Geometry::MultiPoint(points) => points.0.iter_mut().for_each(|point| visit(&mut point.0)),
        Geometry::MultiLineString(lines) => lines.0.iter_mut().flat_map(|line| &mut line.0).for_each(visit),
        Geometry::MultiPolygon(polygons) => {
            for polygon in &mut polygons.0 {
                for_each_polygon_position(polygon, visit);
            }
        }
        Geometry::GeometryCollection(geometries) => {
            for geometry in &mut geometries.0 {
                for_each_position(geometry, visit);
            }
        }
        Geometry::Rect(rect) => {
            let (mut min, mut max) = (rect.min(), rect.max());
            visit(&mut min);
            visit(&mut max);
            *rect = geo_types::Rect::new(min, max);
        }
        Geometry::Triangle(triangle) => {
            let [mut a, mut b, mut c] = triangle.to_array();
            visit(&mut a);
            visit(&mut b);
            visit(&mut c);
            *triangle = geo_types::Triangle::new(a, b, c);
        }
    }
}

fn for_each_polygon_position(polygon: &mut Polygon, visit: &mut impl FnMut(&mut Coord)) {
    polygon.exterior_mut(|ring| ring.0.iter_mut().for_each(&mut *visit));
    polygon.interiors_mut(|rings| rings.iter_mut().flat_map(|ring| &mut ring.0).for_each(&mut *visit));
}

/// How a geometry's positions map: points one by one, and lines and rings
/// as a whole, which may gain or lose positions.
trait Positions {
    /// The position `position` maps to, if any.
    fn point(&self, position: Coord) -> Option<Coord>;

    /// The positions that the line or ring `positions` maps to.
    fn line(&self, positions: Vec<Coord>) -> Vec<Coord>;
}

/// `geometry` with its positions mapped by `positions`, or `None` for a
/// point that maps to none. Lines and rectangles become line strings and
/// polygons, which can gain positions.
fn map_geometry(geometry: Geometry, positions: &impl Positions) -> Option<Geometry> {
    Some(match geometry {
        Geometry::Point(point) => Geometry::Point(Point(positions.point(point.0)?)),
        Geometry::MultiPoint(points) => Geometry::MultiPoint(MultiPoint(
            points.0.into_iter().filter_map(|point| positions.point(point.0).map(Point)).collect(),
        )),
        Geometry::Line(line) => Geometry::LineString(LineString(positions.line(vec![line.start, line.end]))),
        Geometry::LineString(line) => Geometry::LineString(LineString(positions.line(line.0))),
        Geometry::MultiLineString(lines) => Geometry::MultiLineString(MultiLineString(
            lines.0.into_iter().map(|line| LineString(positions.line(line.0))).collect(),
        )),
        Geometry::Polygon(polygon) => Geometry::Polygon(map_polygon(polygon, positions)),
        Geometry::MultiPolygon(polygons) => Geometry::MultiPolygon(MultiPolygon(
            polygons.0.into_iter().map(|polygon| map_polygon(polygon, positions)).collect(),
        )),
        Geometry::Rect(rect) => Geometry::Polygon(map_polygon(rect.to_polygon(), positions)),
        Geometry::Triangle(triangle) => Geometry::Polygon(map_polygon(triangle.to_polygon(), positions)),
        Geometry::GeometryCollection(geometries) => Geometry::GeometryCollection(
            geometries.into_iter().filter_map(|geometry| map_geometry(geometry, positions)).collect(),
        ),
    })
}

fn map_polygon(polygon: Polygon, positions: &impl Positions) -> Polygon {
    let (exterior, interiors) = polygon.into_inner();
    Polygon::new(
        LineString(positions.line(exterior.0)),
        interiors.into_iter().map(|ring| LineString(positions.line(ring.0))).collect(),
    )
}

/// Whether `position` is a lng/lat: a finite longitude and a latitude within
/// 90 degrees of the equator.
fn is_lng_lat(position: Coord) -> bool {
    position.x.is_finite() && position.y.abs() <= 90.0
}

/// Lines and rings moved onto a latitude range, still in lng/lat, and
/// positions that aren't lng/lat left out.
struct OntoLatitudeRange {
    south: f64,
    north: f64,
}

impl OntoLatitudeRange {
    fn new([south, north]: [f64; 2]) -> Self {
        OntoLatitudeRange { south, north }
    }

    /// Whether `latitude` is a limit of the range short of a pole, along
    /// which an edge follows the parallel.
    fn is_limit(&self, latitude: f64) -> bool {
        (latitude == self.south && self.south > -90.0) || (latitude == self.north && self.north < 90.0)
    }
}

impl Positions for OntoLatitudeRange {
    fn point(&self, position: Coord) -> Option<Coord> {
        is_lng_lat(position).then_some(position)
    }

    fn line(&self, mut positions: Vec<Coord>) -> Vec<Coord> {
        if positions.iter().all(|position| position.y >= self.south && position.y <= self.north) {
            positions.retain(|&position| is_lng_lat(position));
            return positions;
        }
        let mut line = Vec::with_capacity(positions.len());
        let mut add = |position: Coord| {
            if is_lng_lat(position) {
                line.push(position);
            }
        };
        let mut previous_longitude = 0.0;
        let mut previous_latitude = f64::NAN;
        for position in positions {
            if !position.x.is_finite() || position.y.is_nan() {
                continue;
            }
            let latitude = position.y.max(self.south).min(self.north);
            if latitude == previous_latitude && self.is_limit(latitude) {
                let span = parallel_span(position.x - previous_longitude);
                let steps = (span.abs() / PARALLEL_STEP).ceil() as u32;
                for step in 1..steps {
                    let longitude = previous_longitude + span * f64::from(step) / f64::from(steps);
                    add(Coord { x: longitude, y: latitude });
                }
            }
            add(Coord { x: position.x, y: latitude });
            previous_longitude = position.x;
            previous_latitude = latitude;
        }
        line
    }
}

/// The longitude that an edge along a parallel, spanning `span` degrees,
/// gets positions over: `span` up to a turn, and past one what is left of
/// it after its whole turns, or a turn if nothing is, which ends where
/// `span` does and draws the same parallel.
fn parallel_span(span: f64) -> f64 {
    if span.abs() <= TURN {
        return span;
    }
    let rest = span % TURN;
    if rest == 0.0 { TURN.copysign(span) } else { rest }
}

/// Positions without a finite place left out.
struct Finite;

impl Positions for Finite {
    fn point(&self, position: Coord) -> Option<Coord> {
        (position.x.is_finite() && position.y.is_finite()).then_some(position)
    }

    fn line(&self, mut positions: Vec<Coord>) -> Vec<Coord> {
        positions.retain(|&position| self.point(position).is_some());
        positions
    }
}
