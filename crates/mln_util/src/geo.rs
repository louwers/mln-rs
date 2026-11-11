// Ports include/mln/util/geo.hpp + src/mln/util/geo.cpp.
//
// The C++ tile-ID constructors (`LatLng(CanonicalTileID)`,
// `LatLng(UnwrappedTileID)`, `LatLngBounds(CanonicalTileID)`) can't name
// the tile ID types here (mln_tile_id depends on mln_util), so they take the
// IDs' fields: `LatLng::from_canonical_tile(z, x, y)` etc.

use geometry::Point;
use mln_math::{clamp_f64, rad2deg, wrap};

use crate::constants;
use crate::error::Error;

pub type ScreenCoordinate = Point<f64>;
pub type ScreenLineString = Vec<Point<f64>>;
// TODO(port): ScreenBox (mapbox::geometry::box<double>) with its first user.

/// `LatLng::WrapMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    Unwrapped,
    Wrapped,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatLng {
    lat: f64,
    lon: f64,
}

impl Default for LatLng {
    fn default() -> Self {
        LatLng { lat: 0.0, lon: 0.0 }
    }
}

fn lat_(z: u8, y: i64) -> f64 {
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y as f64 / 2.0f64.powf(z as f64);
    rad2deg((0.5 * (n.exp() - (-n).exp())).atan())
}

fn lon_(z: u8, x: i64) -> f64 {
    x as f64 / 2.0f64.powf(z as f64) * constants::DEGREES_MAX - constants::LONGITUDE_MAX
}

impl LatLng {
    /// Fails with [`ErrorKind::InvalidArgument`](crate::ErrorKind) for a NaN
    /// coordinate, a latitude outside -90 to 90, or an infinite longitude.
    pub fn new(lat: f64, lon: f64) -> Result<Self, Error> {
        Self::with_mode(lat, lon, WrapMode::Unwrapped)
    }

    pub fn with_mode(lat: f64, lon: f64, mode: WrapMode) -> Result<Self, Error> {
        if lat.is_nan() {
            return Err(Error::invalid_argument("latitude must not be NaN"));
        }
        if lon.is_nan() {
            return Err(Error::invalid_argument("longitude must not be NaN"));
        }
        if lat.abs() > 90.0 {
            return Err(Error::invalid_argument("latitude must be between -90 and 90"));
        }
        if !lon.is_finite() {
            return Err(Error::invalid_argument("longitude must not be infinite"));
        }
        let mut result = LatLng { lat, lon };
        if mode == WrapMode::Wrapped {
            result.wrap();
        }
        Ok(result)
    }

    /// For construction from values that are valid by construction (the
    /// C++ `noexcept` paths).
    pub(crate) const fn new_unchecked(lat: f64, lon: f64) -> Self {
        LatLng { lat, lon }
    }

    /// `LatLng(const CanonicalTileID&)`: the top left of the tile.
    pub fn from_canonical_tile(z: u8, x: u32, y: u32) -> Self {
        LatLng::new_unchecked(lat_(z, y as i64), lon_(z, x as i64))
    }

    /// `LatLng(const UnwrappedTileID&)`.
    pub fn from_unwrapped_tile(z: u8, x: u32, y: u32, wrap: i16) -> Self {
        LatLng::new_unchecked(lat_(z, y as i64), lon_(z, x as i64) + wrap as f64 * constants::DEGREES_MAX)
    }

    pub fn latitude(&self) -> f64 {
        self.lat
    }

    pub fn longitude(&self) -> f64 {
        self.lon
    }

    pub fn wrapped(&self) -> LatLng {
        let mut r = *self;
        r.wrap();
        r
    }

    pub fn wrap(&mut self) {
        self.lon = wrap(self.lon, -constants::LONGITUDE_MAX, constants::LONGITUDE_MAX);
    }

    /// If the distance from start to end longitudes is between half and full
    /// world, unwrap the start longitude to ensure the shortest path is taken.
    pub fn unwrap_for_shortest_path(&mut self, end: &LatLng) {
        let delta = (end.lon - self.lon).abs();
        if delta <= constants::LONGITUDE_MAX || delta >= constants::DEGREES_MAX {
            return;
        }
        if self.lon > 0.0 && end.lon < 0.0 {
            self.lon -= constants::DEGREES_MAX;
        } else if self.lon < 0.0 && end.lon > 0.0 {
            self.lon += constants::DEGREES_MAX;
        }
    }
}

/// `std::min(a, b)` on doubles: `(b < a) ? b : a`.
fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max(a, b)` on doubles: `(a < b) ? b : a`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

#[derive(Clone, Copy, Debug)]
pub struct LatLngBounds {
    sw: LatLng,
    ne: LatLng,
    bounded: bool,
}

impl LatLngBounds {
    /// `LatLngBounds(LatLng sw, LatLng ne)` (private in C++).
    const fn from_corners(sw: LatLng, ne: LatLng) -> Self {
        LatLngBounds { sw, ne, bounded: true }
    }

    /// Return a bounds covering the entire (unwrapped) world.
    pub fn world() -> Self {
        LatLngBounds::from_corners(LatLng::new_unchecked(-90.0, -180.0), LatLng::new_unchecked(90.0, 180.0))
    }

    /// Return the bounds consisting of the single point.
    pub fn singleton(a: LatLng) -> Self {
        LatLngBounds::from_corners(a, a)
    }

    /// Return the convex hull of two points; the smallest bounds that
    /// contains both.
    pub fn hull(a: LatLng, b: LatLng) -> Self {
        let mut bounds = LatLngBounds::from_corners(a, a);
        bounds.extend(&b);
        bounds
    }

    /// Return a bounds that may serve as the identity element for the
    /// extend operation.
    pub fn empty() -> Self {
        let mut bounds = Self::world();
        std::mem::swap(&mut bounds.sw, &mut bounds.ne);
        bounds
    }

    /// Construct an infinite bound, a bound for which the constrain method
    /// returns its input unmodified (the C++ default constructor).
    ///
    /// Note: this is different than LatLngBounds::world() since arbitrary
    /// unwrapped coordinates are also inside the bounds.
    pub fn infinite() -> Self {
        LatLngBounds {
            sw: LatLng::new_unchecked(-90.0, -180.0),
            ne: LatLng::new_unchecked(90.0, 180.0),
            bounded: false,
        }
    }

    /// `LatLngBounds(const CanonicalTileID&)`: the tile's exact boundaries.
    pub fn from_canonical_tile(z: u8, x: u32, y: u32) -> Self {
        LatLngBounds::from_corners(
            // `id.y + 1` is uint32_t arithmetic.
            LatLng::new_unchecked(lat_(z, y.wrapping_add(1) as i64), lon_(z, x as i64)),
            LatLng::new_unchecked(lat_(z, y as i64), lon_(z, x.wrapping_add(1) as i64)),
        )
    }

    pub fn valid(&self) -> bool {
        (self.sw.latitude() <= self.ne.latitude()) && (self.sw.longitude() <= self.ne.longitude())
    }

    pub fn south(&self) -> f64 {
        self.sw.latitude()
    }
    pub fn west(&self) -> f64 {
        self.sw.longitude()
    }
    pub fn north(&self) -> f64 {
        self.ne.latitude()
    }
    pub fn east(&self) -> f64 {
        self.ne.longitude()
    }

    pub fn southwest(&self) -> LatLng {
        self.sw
    }
    pub fn northeast(&self) -> LatLng {
        self.ne
    }
    pub fn southeast(&self) -> LatLng {
        LatLng::new_unchecked(self.south(), self.east())
    }
    pub fn northwest(&self) -> LatLng {
        LatLng::new_unchecked(self.north(), self.west())
    }

    /// C++ constructs the LatLng in a `noexcept` function, so a longitude sum
    /// that overflows to infinity terminates; this panics.
    pub fn center(&self) -> LatLng {
        LatLng::new((self.sw.latitude() + self.ne.latitude()) / 2.0, (self.sw.longitude() + self.ne.longitude()) / 2.0)
            .expect("LatLngBounds::center")
    }

    pub fn constrain(&self, p: &LatLng) -> LatLng {
        if !self.bounded {
            return *p;
        }

        let mut lat = p.latitude();
        let mut lng = p.longitude();

        if !self.contains_latitude(lat) {
            lat = clamp_f64(lat, self.south(), self.north());
        }

        if !self.contains_longitude(lng, WrapMode::Unwrapped) {
            lng = clamp_f64(lng, self.west(), self.east());
        }

        LatLng::new_unchecked(lat, lng)
    }

    pub fn extend(&mut self, point: &LatLng) {
        self.sw = LatLng::new_unchecked(
            std_min(point.latitude(), self.sw.latitude()),
            std_min(point.longitude(), self.sw.longitude()),
        );
        self.ne = LatLng::new_unchecked(
            std_max(point.latitude(), self.ne.latitude()),
            std_max(point.longitude(), self.ne.longitude()),
        );
    }

    pub fn extend_bounds(&mut self, bounds: &LatLngBounds) {
        self.extend(&bounds.sw);
        self.extend(&bounds.ne);
    }

    pub fn is_empty(&self) -> bool {
        self.sw.latitude() > self.ne.latitude() || self.sw.longitude() > self.ne.longitude()
    }

    pub fn crosses_antimeridian(&self) -> bool {
        self.sw.wrapped().longitude() > self.ne.wrapped().longitude()
    }

    // TODO(port): contains(const CanonicalTileID&) (util::TileRange).

    pub fn contains(&self, point: &LatLng) -> bool {
        self.contains_with_mode(point, WrapMode::Unwrapped)
    }

    pub fn contains_with_mode(&self, point: &LatLng, wrap: WrapMode) -> bool {
        self.contains_latitude(point.latitude()) && self.contains_longitude(point.longitude(), wrap)
    }

    pub fn contains_bounds(&self, area: &LatLngBounds, wrap: WrapMode) -> bool {
        let contains_area_latitude = area.north() <= self.north() && area.south() >= self.south();
        if !contains_area_latitude {
            return false;
        }

        let contains_unwrapped = area.east() <= self.east() && area.west() >= self.west();
        if contains_unwrapped {
            return true;
        } else if wrap == WrapMode::Wrapped {
            let wrapped = LatLngBounds::from_corners(self.sw.wrapped(), self.ne.wrapped());
            let other = LatLngBounds::from_corners(area.sw.wrapped(), area.ne.wrapped());
            if self.crosses_antimeridian() && !area.crosses_antimeridian() {
                return (other.east() <= constants::LONGITUDE_MAX && other.west() >= wrapped.west())
                    || (other.east() <= wrapped.east() && other.west() >= -constants::LONGITUDE_MAX);
            } else {
                return other.east() <= wrapped.east() && other.west() >= wrapped.west();
            }
        }
        false
    }

    pub fn intersects(&self, area: LatLngBounds, wrap: WrapMode) -> bool {
        let latitude_intersects = area.north() > self.south() && area.south() < self.north();
        if !latitude_intersects {
            return false;
        }

        let longitude_intersects = area.east() > self.west() && area.west() < self.east();
        if longitude_intersects {
            return true;
        } else if wrap == WrapMode::Wrapped {
            let wrapped = LatLngBounds::from_corners(self.sw.wrapped(), self.ne.wrapped());
            let other = LatLngBounds::from_corners(area.sw.wrapped(), area.ne.wrapped());
            if self.crosses_antimeridian() {
                return area.crosses_antimeridian() || other.east() > wrapped.west() || other.west() < wrapped.east();
            } else if other.crosses_antimeridian() {
                return other.east() > wrapped.west() || other.west() < wrapped.east();
            } else {
                return other.east() > wrapped.west() && other.west() < wrapped.east();
            }
        }
        false
    }

    fn contains_latitude(&self, latitude: f64) -> bool {
        latitude >= self.south() && latitude <= self.north()
    }

    fn contains_longitude(&self, longitude: f64, wrap: WrapMode) -> bool {
        if longitude >= self.west() && longitude <= self.east() {
            return true;
        }

        if wrap == WrapMode::Wrapped {
            let wrapped = LatLngBounds::from_corners(self.sw.wrapped(), self.ne.wrapped());
            // LatLng(0.0, longitude).wrapped(): longitude is finite here
            // (it compared above), except for NaN/inf inputs, which the C++
            // constructor rejects by throwing in a noexcept function.
            let longitude =
                LatLng::new(0.0, longitude).expect("LatLngBounds::contains_longitude").wrapped().longitude();
            if self.crosses_antimeridian() {
                return (longitude >= -constants::LONGITUDE_MAX && longitude <= wrapped.east())
                    || (longitude >= wrapped.west() && longitude <= constants::LONGITUDE_MAX);
            }

            return longitude >= wrapped.west() && longitude <= wrapped.east();
        }

        false
    }
}

impl Default for LatLngBounds {
    fn default() -> Self {
        Self::infinite()
    }
}

impl PartialEq for LatLngBounds {
    fn eq(&self, b: &LatLngBounds) -> bool {
        (!self.bounded && !b.bounded) || (self.bounded && b.bounded && self.sw == b.sw && self.ne == b.ne)
    }
}

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
/// The fields are public (seed API); `new` validates like the C++
/// constructor.
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
    /// Fails with [`ErrorKind::InvalidArgument`](crate::ErrorKind) for a NaN
    /// inset.
    pub fn new(top: f64, left: f64, bottom: f64, right: f64) -> Result<Self, Error> {
        if top.is_nan() {
            return Err(Error::invalid_argument("top must not be NaN"));
        }
        if left.is_nan() {
            return Err(Error::invalid_argument("left must not be NaN"));
        }
        if bottom.is_nan() {
            return Err(Error::invalid_argument("bottom must not be NaN"));
        }
        if right.is_nan() {
            return Err(Error::invalid_argument("right must not be NaN"));
        }
        Ok(EdgeInsets { top, left, bottom, right })
    }

    pub fn top(&self) -> f64 {
        self.top
    }
    pub fn left(&self) -> f64 {
        self.left
    }
    pub fn bottom(&self) -> f64 {
        self.bottom
    }
    pub fn right(&self) -> f64 {
        self.right
    }

    pub fn is_flush(&self) -> bool {
        self.top == 0.0 && self.left == 0.0 && self.bottom == 0.0 && self.right == 0.0
    }

    pub fn get_center(&self, width: u16, height: u16) -> ScreenCoordinate {
        Point::new(
            (width as f64 - self.left() - self.right()) / 2.0 + self.left(),
            (height as f64 - self.top() - self.bottom()) / 2.0 + self.top(),
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
    // C++ goes through the throwing constructor (NaN from inf + -inf).
    fn add(self, o: EdgeInsets) -> EdgeInsets {
        EdgeInsets::new(self.top + o.top, self.left + o.left, self.bottom + o.bottom, self.right + o.right)
            .expect("EdgeInsets::operator+")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatLngAltitude {
    pub location: LatLng,
    pub altitude: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latlng_validation() {
        assert!(LatLng::new(91.0, 0.0).is_err());
        assert!(LatLng::new(f64::NAN, 0.0).is_err());
        assert!(LatLng::new(0.0, f64::INFINITY).is_err());
        assert!(LatLng::new(45.0, 190.0).is_ok());
    }

    #[test]
    fn latlng_wrapping() {
        let ll = LatLng::with_mode(0.0, 190.0, WrapMode::Wrapped).unwrap();
        assert_eq!(ll.longitude(), -170.0);
    }

    #[test]
    fn bounds_extend() {
        let mut b = LatLngBounds::empty();
        assert!(!b.valid());
        b.extend(&LatLng::new(10.0, 20.0).unwrap());
        b.extend(&LatLng::new(-10.0, -20.0).unwrap());
        assert!(b.valid());
        assert!(b.contains(&LatLng::new(0.0, 0.0).unwrap()));
        assert!(!b.contains(&LatLng::new(11.0, 0.0).unwrap()));
    }

    #[test]
    fn infinite_bounds_contains_like_world() {
        // C++ contains() does not look at `bounded`.
        let b = LatLngBounds::infinite();
        assert!(!b.contains(&LatLng::new(0.0, 200.0).unwrap()));
        assert_eq!(LatLngBounds::infinite(), LatLngBounds::default());
        assert_ne!(LatLngBounds::infinite(), LatLngBounds::world());
    }
}
