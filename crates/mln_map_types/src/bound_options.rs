// Ports include/mln/map/bound_options.hpp

use mln_util::geo::LatLngBounds;

/// Holds options to limit what parts of a map are visible. All fields are
/// optional.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundOptions {
    /// Sets the latitude and longitude bounds to which the camera center are
    /// constrained.
    pub bounds: Option<LatLngBounds>,
    /// Sets the maximum zoom level.
    pub max_zoom: Option<f64>,
    /// Sets the minimum zoom level.
    pub min_zoom: Option<f64>,
    /// Sets the maximum pitch.
    pub max_pitch: Option<f64>,
    /// Sets the minimum pitch.
    pub min_pitch: Option<f64>,
}

impl BoundOptions {
    #[must_use]
    pub fn with_lat_lng_bounds(mut self, b: LatLngBounds) -> Self {
        self.bounds = Some(b);
        self
    }
    #[must_use]
    pub fn with_min_zoom(mut self, z: f64) -> Self {
        self.min_zoom = Some(z);
        self
    }
    #[must_use]
    pub fn with_max_zoom(mut self, z: f64) -> Self {
        self.max_zoom = Some(z);
        self
    }
    #[must_use]
    pub fn with_min_pitch(mut self, p: f64) -> Self {
        self.min_pitch = Some(p);
        self
    }
    #[must_use]
    pub fn with_max_pitch(mut self, p: f64) -> Self {
        self.max_pitch = Some(p);
        self
    }
}
