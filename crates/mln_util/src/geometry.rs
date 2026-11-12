// Ports include/mln/util/geometry.hpp: `FeatureType`, `ToFeatureType` and
// `forEachPoint`. The geometry type aliases are the
// `geometry` crate's types (`Point`, `LineString`, ...), re-exported
// here under their mln names.

pub use geometry::{
    Geometry, GeometryCollection, LineString, LinearRing, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};

/// `mln::FeatureType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum FeatureType {
    #[default]
    Unknown = 0,
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

/// `mln::ToFeatureType` applied to a geometry.
pub fn to_feature_type<T>(geometry: &Geometry<T>) -> FeatureType {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => FeatureType::Point,
        Geometry::LineString(_) | Geometry::MultiLineString(_) => FeatureType::LineString,
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => FeatureType::Polygon,
        Geometry::Empty | Geometry::GeometryCollection(_) => FeatureType::Unknown,
    }
}

/// `mln::forEachPoint`: calls `f` with every point of `geometry`, in order,
/// down through its rings, parts and collections.
pub fn for_each_point<T>(geometry: &Geometry<T>, f: &mut impl FnMut(&Point<T>)) {
    let mut each = |points: &[Point<T>]| points.iter().for_each(&mut *f);
    match geometry {
        Geometry::Empty => {}
        Geometry::Point(point) => each(std::slice::from_ref(point)),
        Geometry::MultiPoint(points) | Geometry::LineString(points) => each(points),
        Geometry::MultiLineString(lines) | Geometry::Polygon(lines) => lines.iter().for_each(|line| each(line)),
        Geometry::MultiPolygon(polygons) => polygons.iter().flatten().for_each(|ring| each(ring)),
        Geometry::GeometryCollection(geometries) => geometries.iter().for_each(|g| for_each_point(g, f)),
    }
}

// TODO(port): convertPoint, with its first user.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_each_point_visits_every_point_in_order() {
        let p = |x: f64| Point::new(x, -x);
        let geometry = Geometry::GeometryCollection(vec![
            Geometry::Empty,
            Geometry::Point(p(1.0)),
            Geometry::MultiPoint(vec![p(2.0), p(3.0)]),
            Geometry::LineString(vec![p(4.0)]),
            Geometry::MultiLineString(vec![vec![p(5.0)], vec![p(6.0)]]),
            Geometry::Polygon(vec![vec![p(7.0)], vec![p(8.0)]]),
            Geometry::MultiPolygon(vec![vec![vec![p(9.0)]], vec![vec![p(10.0)], vec![p(11.0)]]]),
            Geometry::GeometryCollection(vec![Geometry::Point(p(12.0))]),
        ]);
        let mut xs = Vec::new();
        for_each_point(&geometry, &mut |point| xs.push(point.x));
        assert_eq!(xs, (1..=12).map(f64::from).collect::<Vec<_>>());
    }
}
