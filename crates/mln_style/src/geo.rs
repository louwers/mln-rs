//! Geographic coordinates: [`LatLng`] and [`LatLngBounds`].

// The C++ tile-ID constructors (`LatLng(CanonicalTileID)`,
// `LatLng(UnwrappedTileID)`, `LatLngBounds(CanonicalTileID)`) take the IDs'
// fields, `LatLng::from_canonical_tile(z, x, y)` etc., since the unwrapped
// tile ID is mln_engine's, which depends on this crate.

use crate::constants;
use crate::error::Error;
use crate::math::{clamp_f64, rad2deg, wrap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    Unwrapped,
    Wrapped,
}

#[derive(Clone, Copy, Debug, PartialEq, mln_wire::Wire)]
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
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y as f64 / 2.0f64.powf(f64::from(z));
    rad2deg((0.5 * (n.exp() - (-n).exp())).atan())
}

fn lon_(z: u8, x: i64) -> f64 {
    x as f64 / 2.0f64.powf(f64::from(z)) * constants::DEGREES_MAX - constants::LONGITUDE_MAX
}

impl LatLng {
    /// Fails with [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) for a NaN
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

    /// The nearest valid location, as an `as` cast between number types
    /// saturates: a NaN coordinate becomes 0, a latitude past ±90 becomes ±90,
    /// and an infinite longitude becomes the largest finite one.
    #[must_use]
    pub fn saturating(lat: f64, lon: f64, mode: WrapMode) -> Self {
        let lat = if lat.is_nan() { 0.0 } else { lat.clamp(-90.0, 90.0) };
        let lon = if lon.is_nan() { 0.0 } else { lon.clamp(f64::MIN, f64::MAX) };
        let mut result = LatLng { lat, lon };
        if mode == WrapMode::Wrapped {
            result.wrap();
        }
        result
    }

    /// For construction from values that are valid by construction (the
    /// C++ `noexcept` paths).
    pub(crate) const fn new_unchecked(lat: f64, lon: f64) -> Self {
        LatLng { lat, lon }
    }

    /// The top left corner of the tile `z/x/y`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_canonical_tile(z: u8, x: u32, y: u32) -> Self {
        LatLng::new_unchecked(lat_(z, i64::from(y)), lon_(z, i64::from(x)))
    }

    #[must_use]
    pub fn from_unwrapped_tile(z: u8, x: u32, y: u32, wrap: i16) -> Self {
        LatLng::new_unchecked(lat_(z, i64::from(y)), lon_(z, i64::from(x)) + f64::from(wrap) * constants::DEGREES_MAX)
    }

    #[must_use]
    pub fn latitude(&self) -> f64 {
        self.lat
    }

    #[must_use]
    pub fn longitude(&self) -> f64 {
        self.lon
    }

    #[must_use]
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

#[derive(Clone, Copy, Debug)]
pub struct LatLngBounds {
    sw: LatLng,
    ne: LatLng,
    bounded: bool,
}

impl LatLngBounds {
    const fn from_corners(sw: LatLng, ne: LatLng) -> Self {
        LatLngBounds { sw, ne, bounded: true }
    }

    /// Return a bounds covering the entire (unwrapped) world.
    #[must_use]
    pub fn world() -> Self {
        LatLngBounds::from_corners(LatLng::new_unchecked(-90.0, -180.0), LatLng::new_unchecked(90.0, 180.0))
    }

    /// Return the bounds consisting of the single point.
    #[must_use]
    pub fn singleton(a: LatLng) -> Self {
        LatLngBounds::from_corners(a, a)
    }

    /// Return the convex hull of two points; the smallest bounds that
    /// contains both.
    #[must_use]
    pub fn hull(a: LatLng, b: LatLng) -> Self {
        let mut bounds = LatLngBounds::from_corners(a, a);
        bounds.extend(&b);
        bounds
    }

    /// Return a bounds that may serve as the identity element for the
    /// extend operation.
    #[must_use]
    pub fn empty() -> Self {
        let mut bounds = Self::world();
        std::mem::swap(&mut bounds.sw, &mut bounds.ne);
        bounds
    }

    /// Construct an infinite bound, a bound for which the constrain method
    /// returns its input unmodified (the C++ default constructor).
    ///
    /// Note: this is different than `LatLngBounds::world()` since arbitrary
    /// unwrapped coordinates are also inside the bounds.
    #[must_use]
    pub fn infinite() -> Self {
        LatLngBounds {
            sw: LatLng::new_unchecked(-90.0, -180.0),
            ne: LatLng::new_unchecked(90.0, 180.0),
            bounded: false,
        }
    }

    /// The exact boundaries of the tile `z/x/y`.
    #[must_use]
    pub fn from_canonical_tile(z: u8, x: u32, y: u32) -> Self {
        LatLngBounds::from_corners(
            // `id.y + 1` is uint32_t arithmetic.
            LatLng::new_unchecked(lat_(z, i64::from(y.wrapping_add(1))), lon_(z, i64::from(x))),
            LatLng::new_unchecked(lat_(z, i64::from(y)), lon_(z, i64::from(x.wrapping_add(1)))),
        )
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        (self.sw.latitude() <= self.ne.latitude()) && (self.sw.longitude() <= self.ne.longitude())
    }

    #[must_use]
    pub fn south(&self) -> f64 {
        self.sw.latitude()
    }
    #[must_use]
    pub fn west(&self) -> f64 {
        self.sw.longitude()
    }
    #[must_use]
    pub fn north(&self) -> f64 {
        self.ne.latitude()
    }
    #[must_use]
    pub fn east(&self) -> f64 {
        self.ne.longitude()
    }

    #[must_use]
    pub fn southwest(&self) -> LatLng {
        self.sw
    }
    #[must_use]
    pub fn northeast(&self) -> LatLng {
        self.ne
    }
    #[must_use]
    pub fn southeast(&self) -> LatLng {
        LatLng::new_unchecked(self.south(), self.east())
    }
    #[must_use]
    pub fn northwest(&self) -> LatLng {
        LatLng::new_unchecked(self.north(), self.west())
    }

    #[must_use]
    pub fn center(&self) -> LatLng {
        // The midpoint of two valid coordinates is valid: `midpoint` can't
        // overflow.
        LatLng::new_unchecked(
            f64::midpoint(self.sw.latitude(), self.ne.latitude()),
            f64::midpoint(self.sw.longitude(), self.ne.longitude()),
        )
    }

    #[must_use]
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
            f64::min(point.latitude(), self.sw.latitude()),
            f64::min(point.longitude(), self.sw.longitude()),
        );
        self.ne = LatLng::new_unchecked(
            f64::max(point.latitude(), self.ne.latitude()),
            f64::max(point.longitude(), self.ne.longitude()),
        );
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sw.latitude() > self.ne.latitude() || self.sw.longitude() > self.ne.longitude()
    }

    #[must_use]
    pub fn crosses_antimeridian(&self) -> bool {
        self.sw.wrapped().longitude() > self.ne.wrapped().longitude()
    }

    #[must_use]
    pub fn contains(&self, point: &LatLng) -> bool {
        self.contains_with_mode(point, WrapMode::Unwrapped)
    }

    #[must_use]
    pub fn contains_with_mode(&self, point: &LatLng, wrap: WrapMode) -> bool {
        self.contains_latitude(point.latitude()) && self.contains_longitude(point.longitude(), wrap)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn contains_bounds(&self, area: &LatLngBounds, wrap: WrapMode) -> bool {
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
            }
            return other.east() <= wrapped.east() && other.west() >= wrapped.west();
        }
        false
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn intersects(&self, area: LatLngBounds, wrap: WrapMode) -> bool {
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
            }
            return other.east() > wrapped.west() && other.west() < wrapped.east();
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
            let longitude = crate::math::wrap(longitude, -constants::LONGITUDE_MAX, constants::LONGITUDE_MAX);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_lat_lng() {
        let lat_lng = LatLng::saturating(f64::NAN, f64::INFINITY, WrapMode::Unwrapped);
        assert_eq!((0.0, f64::MAX), (lat_lng.latitude(), lat_lng.longitude()));
        let lat_lng = LatLng::saturating(-100.0, f64::NAN, WrapMode::Unwrapped);
        assert_eq!((-90.0, 0.0), (lat_lng.latitude(), lat_lng.longitude()));
        assert_eq!(190.0 - 360.0, LatLng::saturating(0.0, 190.0, WrapMode::Wrapped).longitude());
    }

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
