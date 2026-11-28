//! The map's geometry: the camera's screen coordinates, edge insets and
//! orientation; spherical Mercator ([`projection`]); the world square and
//! how lng/lat maps to it, in Mercator or a registered projection
//! ([`world`]); the camera's frustum and the bounding volumes it culls
//! (`bounding_volumes`); intersection tests, point math, and the Earth's
//! size ([`constants`]).

pub(crate) mod bounding_volumes;
pub mod constants;
pub mod intersection_tests;
pub mod math;
pub mod projection;
pub mod world;

use geometry::Point;
use mln_style::geo::LatLng;
use mln_style::size::Size;

pub type ScreenCoordinate = Point<f64>;
pub(crate) type ScreenLineString = Vec<Point<f64>>;

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
    pub(crate) fn center_in(&self, size: Size) -> ScreenCoordinate {
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
pub(crate) struct LatLngAltitude {
    pub(crate) location: LatLng,
    pub(crate) altitude: f64,
}

/// The latitudes of the north and the south edges of `id`, in degrees, by
/// which the hillshade shader undoes Mercator's stretch of slopes: `[0, 0]`,
/// no correction, in a registered projection's `world`, which doesn't wrap
/// and draws in its own units.
#[must_use]
pub fn lat_range(id: &crate::tile::UnwrappedTileID, world: &world::WorldMapping) -> [f32; 2] {
    if !world.wraps() {
        return [0.0, 0.0];
    }
    let latlng0 = LatLng::from_unwrapped_tile(id.canonical.z, id.canonical.x, id.canonical.y, id.wrap);
    let below =
        crate::tile::UnwrappedTileID::new(id.canonical.z, i64::from(id.canonical.x), i64::from(id.canonical.y) + 1);
    let latlng1 = LatLng::from_unwrapped_tile(below.canonical.z, below.canonical.x, below.canonical.y, below.wrap);
    [latlng0.latitude() as f32, latlng1.latitude() as f32]
}
