//! The camera the recognizer moves: a small command sink that reads the
//! camera and sends it maplibre-native-ffi's camera calls.
//! `mln_runtime::Map` implements it.

use mln_map::{CameraDelta, CameraOptions, CameraUpdate, TransformState, TransitionEnd};
use mln_util::LatLng;
use mln_util::geo::{ScreenCoordinate, WrapMode};

/// What the recognizer needs from a camera: to read it, and
/// maplibre-native-ffi's two camera calls, which carry each change's gesture
/// phase. The calls may apply later, as a runtime's commands do: the
/// recognizer reads what the camera shows by then.
pub trait GestureCamera {
    /// The viewport's width and height, in logical pixels.
    fn viewport(&self) -> ScreenCoordinate;
    fn camera(&self) -> CameraOptions;
    /// The least and greatest zoom the camera may take at its center (see
    /// [`zoom_range`]).
    fn zoom_range(&self) -> (f64, f64);
    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng>;
    /// Applies a delta as [`Map::apply_camera_delta`](mln_map::Map::apply_camera_delta)
    /// does. The recognizer only sends valid deltas.
    fn apply_camera_delta(&mut self, delta: &CameraDelta);
    /// Applies a gesture's last delta, as `apply_camera_delta` does, and
    /// returns how its command ends: the end of the momentum it starts,
    /// whose speed a press that stops it can carry.
    fn release(&mut self, delta: &CameraDelta) -> Box<dyn PendingEnd>;
    fn update_camera(&mut self, update: &CameraUpdate);
    fn cancel_transitions_with_id(&mut self, transition_id: u64);
}

/// How a camera command ends, once it has.
pub trait PendingEnd: std::fmt::Debug {
    /// The command's end, once it has come; `None` before.
    fn take(&mut self) -> Option<TransitionEnd>;
}

/// The least and greatest zoom that `state` may take at `latitude`: on the
/// globe the least falls toward the poles, and `ConstrainMode::Screen` bounds
/// raise it to the zoom they fill the screen at.
#[must_use]
pub fn zoom_range(state: &TransformState, latitude: f64) -> (f64, f64) {
    let min = state.get_min_zoom_at_latitude(latitude);
    let min = state.get_min_zoom_for_screen_bounds().map_or(min, |bounds| min.max(bounds));
    let max = state.get_max_zoom();
    // Bounds too small to fill the screen at the greatest zoom hold the
    // camera there.
    (min.min(max), max)
}

/// The latitude of `state`'s center, as the recognizer reads it.
#[must_use]
pub fn center_latitude(state: &TransformState) -> f64 {
    state.get_lat_lng(WrapMode::Unwrapped).latitude()
}
