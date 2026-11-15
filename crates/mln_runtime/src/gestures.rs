//! A map as the camera that `mln_gestures`' recognizer moves: it reads the
//! map's published snapshot and sends camera commands, without waiting for
//! them.

use std::sync::Arc;

use mln_gestures::{GestureCamera, PendingEnd};
use mln_map::{CameraDelta, CameraOptions, CameraUpdate, MapProjection, MapSnapshot, TransitionEnd};
use mln_util::LatLng;
use mln_util::geo::{LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::logging::{Event, Log};
use parking_lot::Mutex;

use crate::map::Map;

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
        state.set_lat_lng_bounds(bounds.bounds.unwrap_or_else(LatLngBounds::infinite));
        state.set_min_zoom(bounds.min_zoom);
        state.set_max_zoom(bounds.max_zoom);
        mln_gestures::zoom_range(&state, snapshot.view.camera.center.latitude())
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        let projection = MapProjection::from_view(&self.gesture_snapshot()?.view).ok()?;
        projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        sent(Map::apply_camera_delta(self, delta.clone()));
    }

    fn release(&mut self, delta: &CameraDelta) -> Box<dyn PendingEnd> {
        let end = Arc::new(Mutex::new(None));
        let ended = Arc::clone(&end);
        sent(self.apply_camera_delta_then(delta.clone(), move |result| *ended.lock() = result.ok()));
        Box::new(EndSlot(end))
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        sent(Map::update_camera(self, update.clone()));
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        sent(self.cancel_camera_transitions_with_id(transition_id));
    }
}

/// A camera command's end, which the runtime's thread leaves here.
#[derive(Debug)]
struct EndSlot(Arc<Mutex<Option<TransitionEnd>>>);

impl PendingEnd for EndSlot {
    fn take(&mut self) -> Option<TransitionEnd> {
        self.0.lock().take()
    }
}
