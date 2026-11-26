//! The camera the recognizer moves: `mln_runtime::Map`, and in this crate's
//! tests a bare transform.

use std::sync::Arc;

use mln_engine::camera::{CameraDelta, CameraOptions, CameraUpdate, TransformState, TransitionEnd};
use mln_style::geo::LatLng;
use mln_util::geo::ScreenCoordinate;
use parking_lot::Mutex;

/// What the recognizer needs from a camera: to read it, and
/// the map's two camera calls, which carry each change's gesture
/// phase. The calls may apply later, as a runtime's commands do: the
/// recognizer reads what the camera shows by then. It is the recognizer's
/// only tie to the map.
pub trait GestureCamera {
    /// The viewport's width and height, in logical pixels.
    fn viewport(&self) -> ScreenCoordinate;
    fn camera(&self) -> CameraOptions;
    /// The least and greatest zoom the camera may take at its center (see
    /// [`zoom_range`]).
    fn zoom_range(&self) -> (f64, f64);
    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng>;
    /// Applies a delta as [`Map::apply_camera_delta`](mln_engine::map::Map::apply_camera_delta)
    /// does. The recognizer only sends valid deltas.
    fn apply_camera_delta(&mut self, delta: &CameraDelta);
    /// Applies a gesture's last delta, as `apply_camera_delta` does, and
    /// returns how its command ends: the end of the momentum it starts,
    /// whose speed a press that stops it can carry.
    fn release(&mut self, delta: &CameraDelta) -> PendingEnd;
    fn update_camera(&mut self, update: &CameraUpdate);
    fn cancel_transitions_with_id(&mut self, transition_id: u64);
}

/// Where a camera command's end arrives: the camera keeps a clone and sets
/// it when the command ends, and the recognizer takes it. A command that
/// fails leaves it empty.
#[derive(Clone, Debug, Default)]
pub struct PendingEnd(Arc<Mutex<Option<TransitionEnd>>>);

impl PendingEnd {
    pub fn set(&self, end: TransitionEnd) {
        *self.0.lock() = Some(end);
    }

    /// The command's end, once it has come; `None` before.
    #[must_use]
    pub fn take(&self) -> Option<TransitionEnd> {
        self.0.lock().take()
    }

    /// Whether the command's end has come and not been taken.
    #[must_use]
    pub(crate) fn has_ended(&self) -> bool {
        self.0.lock().is_some()
    }
}

/// The least and greatest zoom that `state` may take at `latitude`: on the
/// globe the least falls toward the poles, and `ConstrainMode::Screen` bounds
/// raise it to the zoom they fill the screen at.
#[must_use]
pub fn zoom_range(state: &TransformState, latitude: f64) -> (f64, f64) {
    let min = state.min_zoom_at_latitude(latitude);
    let min = state.min_zoom_for_screen_bounds().map_or(min, |bounds| min.max(bounds));
    let max = state.limits().max_zoom;
    // Bounds too small to fill the screen at the greatest zoom hold the
    // camera there.
    (min.min(max), max)
}
