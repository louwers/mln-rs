//! Ports `mapbox::geometry`: mapbox/geometry/{point,line_string,polygon,
//! multi_*,geometry}.hpp of geometry.hpp (the C++ core's
//! vendor/maplibre-native-base/deps/geometry.hpp), which
//! `include/mln/util/geometry.hpp` re-exports into the `mln` namespace.
//!
//! Deliberately not `geo-types`: the port needs the exact generic coordinate
//! parameter of the C++ types.

use serde::{Deserialize, Serialize};

/// `mapbox::geometry::point<T>`
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

impl<T> Point<T> {
    pub const fn new(x: T, y: T) -> Self {
        Point { x, y }
    }
}

// In C++ these are thin subclasses of std::vector; type aliases suffice here.
pub type LineString<T> = Vec<Point<T>>;
pub type LinearRing<T> = Vec<Point<T>>;
pub type MultiPoint<T> = Vec<Point<T>>;
pub type Polygon<T> = Vec<LinearRing<T>>;
pub type MultiLineString<T> = Vec<LineString<T>>;
pub type MultiPolygon<T> = Vec<Polygon<T>>;
pub type GeometryCollection<T> = Vec<Geometry<T>>;

/// `mapbox::geometry::geometry<T>` (a variant in C++; `empty` is the
/// default-constructed state).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Geometry<T> {
    #[default]
    Empty,
    Point(Point<T>),
    MultiPoint(MultiPoint<T>),
    LineString(LineString<T>),
    MultiLineString(MultiLineString<T>),
    Polygon(Polygon<T>),
    MultiPolygon(MultiPolygon<T>),
    GeometryCollection(GeometryCollection<T>),
}

// TODO(port): mapbox/geometry/point_arithmetic.hpp (operator+/-/*// on points)
// with its first user (tile geometry scaling).
