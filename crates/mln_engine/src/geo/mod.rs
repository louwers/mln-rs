//! The map's geometry: the camera's screen coordinates, edge insets and
//! orientation; spherical Mercator ([`projection`]); the camera's frustum
//! and the bounding volumes it culls ([`bounding_volumes`]); intersection
//! tests, point math, and the Earth's size ([`constants`]).

pub mod bounding_volumes;
pub mod constants;
pub mod intersection_tests;
pub mod math;
pub mod projection;

use geometry::Point;
use mln_style::geo::LatLng;
use mln_style::size::Size;

pub type ScreenCoordinate = Point<f64>;
pub type ScreenLineString = Vec<Point<f64>>;

/// Determines the orientation of the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NorthOrientation {
    #[default]
    Upwards,
    Rightwards,
    Downwards,
    Leftwards,
}

/// The distance on each side between a rectangle and a rectangle within.
///
/// Any values are allowed; the users of insets say which they accept.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeInsets {
    /// Number of pixels inset from the top edge.
    pub top: f64,
    /// Number of pixels inset from the left edge.
    pub left: f64,
    /// Number of pixels inset from the bottom edge.
    pub bottom: f64,
    /// Number of pixels inset from the right edge.
    pub right: f64,
}

impl EdgeInsets {
    #[must_use]
    pub const fn new(top: f64, left: f64, bottom: f64, right: f64) -> Self {
        EdgeInsets { top, left, bottom, right }
    }

    #[must_use]
    pub fn top(&self) -> f64 {
        self.top
    }
    #[must_use]
    pub fn left(&self) -> f64 {
        self.left
    }
    #[must_use]
    pub fn bottom(&self) -> f64 {
        self.bottom
    }
    #[must_use]
    pub fn right(&self) -> f64 {
        self.right
    }

    /// The center of the area these insets leave in a viewport of `size`, in
    /// pixels from its top left.
    #[must_use]
    pub fn center_in(&self, size: Size) -> ScreenCoordinate {
        Point::new(
            (f64::from(size.width) - self.left() - self.right()) / 2.0 + self.left(),
            (f64::from(size.height) - self.top() - self.bottom()) / 2.0 + self.top(),
        )
    }
}

impl std::ops::AddAssign for EdgeInsets {
    fn add_assign(&mut self, o: EdgeInsets) {
        self.top += o.top;
        self.left += o.left;
        self.bottom += o.bottom;
        self.right += o.right;
    }
}

impl std::ops::Add for EdgeInsets {
    type Output = EdgeInsets;
    fn add(mut self, o: EdgeInsets) -> EdgeInsets {
        self += o;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatLngAltitude {
    pub location: LatLng,
    pub altitude: f64,
}
