// Ports include/mln/util/projection.hpp

use std::f64::consts::PI;

use geometry::Point;
use mln_math::{clamp_f64, deg2rad, rad2deg};

use crate::constants::{DEGREES_MAX, LATITUDE_MAX, LONGITUDE_MAX, MAX_ZOOM, MIN_ZOOM};
use crate::error::Error;
use crate::geo::{LatLng, WrapMode};

// TODO(port): use mln_util::constants::TILE_SIZE_D once it exists (constants.rs
// currently calls tileSize_D `TILE_SIZE`).
const TILE_SIZE_D: f64 = 512.0;
// TODO(port): use mln_util::constants::M2PI once it exists.
const M2PI: f64 = PI * 2.0;
// TODO(port): use mln_util::constants::EARTH_RADIUS_M once it exists.
const EARTH_RADIUS_M: f64 = 6378137.0;

/// `mln::ProjectedMeters`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectedMeters {
    /// Distance measured northwards.
    northing: f64,
    /// Distance measured eastwards.
    easting: f64,
}

// The derived PartialEq is the C++ operator==:
// `a._northing == b._northing && a._easting == b._easting`.

impl ProjectedMeters {
    /// Fails with [`ErrorKind::InvalidArgument`](crate::ErrorKind) for a NaN
    /// northing or easting.
    pub fn new(n: f64, e: f64) -> Result<Self, Error> {
        if n.is_nan() {
            return Err(Error::invalid_argument("northing must not be NaN"));
        }
        if e.is_nan() {
            return Err(Error::invalid_argument("easting must not be NaN"));
        }
        Ok(ProjectedMeters { northing: n, easting: e })
    }

    pub fn northing(&self) -> f64 {
        self.northing
    }

    pub fn easting(&self) -> f64 {
        self.easting
    }
}

/// Spherical Mercator projection
/// <https://github.com/openlayers/docs/blob/c7ab69156e17bedc1059d64d7a864b7868e354c1/library/spherical_mercator.rst>
pub struct Projection;

impl Projection {
    /// Map pixel width at given scale.
    pub fn world_size(scale: f64) -> f64 {
        scale * TILE_SIZE_D
    }

    pub fn get_meters_per_pixel_at_latitude(lat: f64, zoom: f64) -> f64 {
        let constrained_zoom = clamp_f64(zoom, MIN_ZOOM, MAX_ZOOM);
        let constrained_scale = 2.0f64.powf(constrained_zoom);
        let constrained_latitude = clamp_f64(lat, -LATITUDE_MAX, LATITUDE_MAX);
        deg2rad(constrained_latitude).cos() * M2PI * EARTH_RADIUS_M / Self::world_size(constrained_scale)
    }

    pub fn projected_meters_for_lat_lng(lat_lng: &LatLng) -> ProjectedMeters {
        let constrained_latitude = clamp_f64(lat_lng.latitude(), -LATITUDE_MAX, LATITUDE_MAX);
        let constrained_longitude = clamp_f64(lat_lng.longitude(), -LONGITUDE_MAX, LONGITUDE_MAX);

        let m = 1.0 - 1e-15;
        let f = clamp_f64(deg2rad(constrained_latitude).sin(), -m, m);

        let easting = deg2rad(EARTH_RADIUS_M * constrained_longitude);
        let northing = 0.5 * EARTH_RADIUS_M * ((1.0 + f) / (1.0 - f)).ln();

        // C++ is noexcept here: a LatLng is never NaN, so neither value is.
        ProjectedMeters::new(northing, easting).expect("projected meters from a valid LatLng")
    }

    /// Not noexcept in C++, but the clamps keep both values in range (a NaN
    /// clamps to the upper bound), so the LatLng constructor cannot throw.
    pub fn lat_lng_for_projected_meters(projected_meters: &ProjectedMeters) -> LatLng {
        let mut latitude = rad2deg(2.0 * (projected_meters.northing() / EARTH_RADIUS_M).exp().atan() - (PI / 2.0));
        let mut longitude = rad2deg(projected_meters.easting()) / EARTH_RADIUS_M;

        latitude = clamp_f64(latitude, -LATITUDE_MAX, LATITUDE_MAX);
        longitude = clamp_f64(longitude, -LONGITUDE_MAX, LONGITUDE_MAX);

        LatLng::new(latitude, longitude).expect("clamped latitude and longitude")
    }

    pub fn project(lat_lng: &LatLng, scale: f64) -> Point<f64> {
        Self::project_(lat_lng, Self::world_size(scale))
    }

    /// Returns point on tile. The C++ `project(const LatLng&, int32_t zoom)`
    /// overload: `worldSize = 1 << zoom` (int shift; zoom must be 0..=30).
    pub fn project_zoom(lat_lng: &LatLng, zoom: i32) -> Point<f64> {
        Self::project_(lat_lng, (1i32 << zoom) as f64)
    }

    /// Fails with the [`LatLng::new`] error for a NaN or infinite result.
    pub fn unproject(p: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        // `p * util::DEGREES_MAX / worldSize(scale)` (mapbox point operators)
        let world_size = Self::world_size(scale);
        let p2 = Point::new(p.x * DEGREES_MAX / world_size, p.y * DEGREES_MAX / world_size);
        LatLng::with_mode(
            deg2rad(LONGITUDE_MAX - p2.y).exp().atan() * DEGREES_MAX / PI - 90.0,
            p2.x - LONGITUDE_MAX,
            wrap_mode,
        )
    }

    fn project_(lat_lng: &LatLng, world_size: f64) -> Point<f64> {
        let latitude = clamp_f64(lat_lng.latitude(), -LATITUDE_MAX, LATITUDE_MAX);
        // `Point{...} * (worldSize / util::DEGREES_MAX)`
        let k = world_size / DEGREES_MAX;
        Point::new(
            (LONGITUDE_MAX + lat_lng.longitude()) * k,
            (LONGITUDE_MAX - rad2deg((PI / 4.0 + latitude * PI / DEGREES_MAX).tan().ln())) * k,
        )
    }
}

#[cfg(test)]
#[path = "projection_test.rs"]
mod projection_test;
