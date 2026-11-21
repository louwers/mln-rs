//! A map as the camera that `mln_gestures`' recognizer moves: it reads the
//! map's published snapshot and sends camera commands, without waiting for
//! them.

use mln_gestures::{GestureCamera, PendingEnd};
use mln_map_types::{CameraDelta, CameraOptions, CameraUpdate};
use mln_util::LatLng;
use mln_util::geo::{LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::logging::{Event, Log};

use crate::map::{MapProjection, MapSnapshot};
use crate::map_handle::Map;

impl Map {
    /// The map's snapshot for the recognizer, which reads nothing of a map
    /// that is gone.
    fn gesture_snapshot(&self) -> Option<MapSnapshot> {
        self.snapshot().ok()
    }
}

/// Logs a command that a released map refused, which the recognizer can't
/// act on.
fn sent<T>(result: Result<T, mln_util::Error>) -> Option<T> {
    result.map_err(|error| Log::warning(Event::General, format!("gesture camera command: {error}"))).ok()
}

impl GestureCamera for Map {
    fn viewport(&self) -> ScreenCoordinate {
        let size = self.gesture_snapshot().map(|snapshot| snapshot.view.extent.size).unwrap_or_default();
        ScreenCoordinate::new(f64::from(size.width), f64::from(size.height))
    }

    fn camera(&self) -> CameraOptions {
        self.gesture_snapshot().map(|snapshot| CameraOptions::from(snapshot.view.camera)).unwrap_or_default()
    }

    fn zoom_range(&self) -> (f64, f64) {
        let Some(snapshot) = self.gesture_snapshot() else { return (0.0, 0.0) };
        let Ok(mut state) = snapshot.view.to_state() else {
            return (snapshot.bounds.min_zoom, snapshot.bounds.max_zoom);
        };
        let bounds = snapshot.bounds;
        let limits = mln_map_types::CameraLimits {
            bounds: bounds.bounds.unwrap_or_else(LatLngBounds::infinite),
            min_zoom: bounds.min_zoom,
            max_zoom: bounds.max_zoom,
            ..*state.limits()
        };
        if state.set_limits(limits).is_err() {
            return (bounds.min_zoom, bounds.max_zoom);
        }
        mln_gestures::zoom_range(&state, snapshot.view.camera.center.latitude())
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        let projection = MapProjection::from_view(&self.gesture_snapshot()?.view).ok()?;
        projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        sent(Map::apply_camera_delta(self, delta.clone()));
    }

    fn release(&mut self, delta: &CameraDelta) -> PendingEnd {
        let end = PendingEnd::default();
        let ended = end.clone();
        sent(self.apply_camera_delta_then(delta.clone(), move |result| {
            if let Ok(result) = result {
                ended.set(result);
            }
        }));
        end
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        sent(Map::update_camera(self, update.clone()));
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        sent(self.cancel_camera_transitions_with_id(transition_id));
    }
}
