//! Ports `mapbox::geometry::point<T>` (mapbox/geometry/point.hpp of
//! geometry.hpp, the C++ core's vendor/maplibre-native-base/deps/geometry.hpp),
//! the point of tile geometry and the 2D vector of the renderer.
//!
//! GeoJSON and feature geometry is `geo_types`'; [`Point`] converts to and
//! from its [`Coord`].

use geo_types::{Coord, CoordNum};
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

impl<T: CoordNum> From<Coord<T>> for Point<T> {
    fn from(c: Coord<T>) -> Self {
        Point { x: c.x, y: c.y }
    }
}

impl<T: CoordNum> From<Point<T>> for Coord<T> {
    fn from(p: Point<T>) -> Self {
        Coord { x: p.x, y: p.y }
    }
}
