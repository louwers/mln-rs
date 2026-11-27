//! The camera's limits: its pitch, field of view and zoom.

pub(crate) const PITCH_MIN: f64 = 0.0;
pub(crate) const DEFAULT_PITCH_MAX: f64 = std::f64::consts::PI / 3.0;
pub(crate) const PITCH_MAX: f64 = std::f64::consts::PI;
/// This fov value is somewhat arbitrary. The altitude of the camera used
/// to be defined as 1.5 screen heights above the ground, which was an
/// arbitrary choice. This is the fov equivalent to that value calculated
/// with: `fov = 2 * arctan((height / 2) / (height * 1.5))`
pub const DEFAULT_FOV: f64 = 0.643_501_108_793_284_4;
pub const MIN_ZOOM: f64 = 0.0;
pub const MAX_ZOOM: f64 = 25.5;
