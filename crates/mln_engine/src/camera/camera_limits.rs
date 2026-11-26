//! Where a map's camera may go: its bounds and the zoom and pitch it may take.

use mln_style::constants::DEFAULT_MAX_ZOOM;
use mln_style::error::Error;
use mln_style::geo::LatLngBounds;

use crate::camera::constants::{DEFAULT_PITCH_MAX, MAX_ZOOM, MIN_ZOOM, PITCH_MAX, PITCH_MIN};

/// The narrowest field of view a camera may take, in radians.
pub const MIN_FIELD_OF_VIEW: f64 = 0.1 * std::f64::consts::PI / 180.0;
/// The widest field of view a camera may take, in radians.
pub const MAX_FIELD_OF_VIEW: f64 = 150.0 * std::f64::consts::PI / 180.0;

/// The limits a map's camera moves within.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraLimits {
    /// The area the center stays within.
    pub bounds: LatLngBounds,
    pub min_zoom: f64,
    pub max_zoom: f64,
    /// The least pitch, in radians.
    pub min_pitch: f64,
    /// The most pitch, in radians.
    pub max_pitch: f64,
}

impl Default for CameraLimits {
    fn default() -> Self {
        CameraLimits {
            bounds: LatLngBounds::infinite(),
            min_zoom: MIN_ZOOM,
            max_zoom: f64::from(DEFAULT_MAX_ZOOM),
            min_pitch: PITCH_MIN,
            max_pitch: DEFAULT_PITCH_MAX,
        }
    }
}

impl CameraLimits {
    /// The widest limits that any map allows.
    #[must_use]
    pub fn widest() -> Self {
        CameraLimits { max_zoom: MAX_ZOOM, max_pitch: PITCH_MAX, ..CameraLimits::default() }
    }

    /// These limits with each zoom and pitch clamped to the range that any
    /// map allows, or `Err` for limits that leave the camera nowhere to go:
    /// a number that isn't finite, a minimum above its maximum, or invalid
    /// bounds.
    pub fn validated(self) -> Result<Self, Error> {
        if ![self.min_zoom, self.max_zoom, self.min_pitch, self.max_pitch].iter().all(|value| value.is_finite()) {
            return Err(Error::invalid_argument("camera limits must be finite"));
        }
        if !self.bounds.valid() {
            return Err(Error::invalid_argument("camera bounds are invalid"));
        }
        if self.min_zoom > self.max_zoom {
            return Err(Error::invalid_argument("the minimum zoom must be at most the maximum zoom"));
        }
        if self.min_pitch > self.max_pitch {
            return Err(Error::invalid_argument("the minimum pitch must be at most the maximum pitch"));
        }
        Ok(CameraLimits {
            bounds: self.bounds,
            min_zoom: self.min_zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            max_zoom: self.max_zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            min_pitch: self.min_pitch.clamp(PITCH_MIN, PITCH_MAX),
            max_pitch: self.max_pitch.clamp(PITCH_MIN, PITCH_MAX),
        })
    }
}
