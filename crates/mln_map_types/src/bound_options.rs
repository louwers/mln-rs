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
