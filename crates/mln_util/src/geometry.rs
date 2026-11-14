// Tile geometry's point is the `geometry` crate's, re-exported here.
// GeoJSON and feature geometry is `geo_types`'.

use geo_types::{Coord, CoordNum, Geometry};
pub use geometry::Point;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum FeatureType {
    #[default]
    Unknown = 0,
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

/// The feature type of `geometry`; `None`, a feature's null geometry, is
/// [`FeatureType::Unknown`].
#[must_use]
pub fn to_feature_type<T: CoordNum>(geometry: Option<&Geometry<T>>) -> FeatureType {
    match geometry {
        Some(Geometry::Point(_) | Geometry::MultiPoint(_)) => FeatureType::Point,
        Some(Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_)) => FeatureType::LineString,
        Some(Geometry::Polygon(_) | Geometry::MultiPolygon(_) | Geometry::Rect(_) | Geometry::Triangle(_)) => {
            FeatureType::Polygon
        }
        Some(Geometry::GeometryCollection(_)) | None => FeatureType::Unknown,
    }
}

/// Calls `f` with every coordinate of `geometry`, in order, down through its
/// rings, parts and collections. A polygon's exterior comes before its
/// interiors.
pub fn for_each_point<T: CoordNum>(geometry: &Geometry<T>, f: &mut impl FnMut(&Coord<T>)) {
    let mut each = |coords: &[Coord<T>]| coords.iter().for_each(&mut *f);
    match geometry {
        Geometry::Point(point) => each(std::slice::from_ref(&point.0)),
        Geometry::Line(line) => each(&[line.start, line.end]),
        Geometry::LineString(line) => each(&line.0),
        Geometry::MultiPoint(points) => points.iter().for_each(|p| each(std::slice::from_ref(&p.0))),
        Geometry::MultiLineString(lines) => lines.iter().for_each(|line| each(&line.0)),
        Geometry::Polygon(polygon) => {
            each(&polygon.exterior().0);
            polygon.interiors().iter().for_each(|ring| each(&ring.0));
        }
        Geometry::MultiPolygon(polygons) => polygons.iter().for_each(|polygon| {
            each(&polygon.exterior().0);
            polygon.interiors().iter().for_each(|ring| each(&ring.0));
        }),
        Geometry::GeometryCollection(geometries) => geometries.iter().for_each(|g| for_each_point(g, f)),
        Geometry::Rect(rect) => each(&rect.to_polygon().exterior().0),
        Geometry::Triangle(triangle) => each(&triangle.to_array()),
    }
}

#[cfg(test)]
mod tests {
    use geo_types::{GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon};

    use super::*;

    #[test]
    fn for_each_point_visits_every_point_in_order() {
        let p = |x: f64| Coord { x, y: -x };
        let line = |xs: &[f64]| LineString(xs.iter().map(|&x| p(x)).collect());
        let geometry = Geometry::GeometryCollection(GeometryCollection(vec![
            Geometry::GeometryCollection(GeometryCollection(vec![])),
            Geometry::Point(p(1.0).into()),
            Geometry::MultiPoint(MultiPoint(vec![p(2.0).into(), p(3.0).into()])),
            Geometry::LineString(line(&[4.0])),
            Geometry::MultiLineString(MultiLineString(vec![line(&[5.0]), line(&[6.0])])),
            // `Polygon::new` closes the exterior: 7, 8, 7.
            Geometry::Polygon(Polygon::new(line(&[7.0, 8.0]), vec![line(&[9.0])])),
            Geometry::MultiPolygon(MultiPolygon(vec![Polygon::new(line(&[10.0]), vec![])])),
            Geometry::GeometryCollection(GeometryCollection(vec![Geometry::Point(p(11.0).into())])),
        ]));
        let mut xs = Vec::new();
        for_each_point(&geometry, &mut |point| xs.push(point.x));
        assert_eq!(xs, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 7.0, 9.0, 10.0, 11.0]);
    }
}
