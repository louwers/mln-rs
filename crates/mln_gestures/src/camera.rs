//! The camera the recognizer moves: a [`Map`], or a bare [`Transform`] (the
//! tests, which need no renderer).

use mln_map::transform::Transform;
use mln_map::{CameraDelta, CameraOptions, CameraUpdate, Map};
use mln_util::LatLng;
use mln_util::geo::{ScreenCoordinate, WrapMode};

/// What the recognizer needs from a camera: to read it, and
/// maplibre-native-ffi's two camera calls, which carry each change's gesture
/// phase. [`Map`] and [`Transform`] provide it.
pub trait GestureCamera {
    /// The viewport's width and height, in logical pixels.
    fn viewport(&self) -> ScreenCoordinate;
    fn camera(&self) -> CameraOptions;
    /// The least and greatest zoom the camera may take.
    fn zoom_range(&self) -> (f64, f64);
    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng>;
    /// [`Map::apply_camera_delta`]. The recognizer only sends valid deltas.
    fn apply_camera_delta(&mut self, delta: &CameraDelta);
    fn update_camera(&mut self, update: &CameraUpdate);
    fn cancel_transitions_with_id(&mut self, transition_id: u64);
}

impl GestureCamera for Map {
    fn viewport(&self) -> ScreenCoordinate {
        let size = self.get_map_options().size();
        ScreenCoordinate::new(f64::from(size.width), f64::from(size.height))
    }

    fn camera(&self) -> CameraOptions {
        self.get_camera_options(None)
    }

    fn zoom_range(&self) -> (f64, f64) {
        let bounds = self.get_bounds();
        (bounds.min_zoom.unwrap_or(f64::NEG_INFINITY), bounds.max_zoom.unwrap_or(f64::INFINITY))
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        Map::lat_lng_for_pixel(self, &pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        let applied = Map::apply_camera_delta(self, delta);
        debug_assert!(applied.is_ok(), "{applied:?}");
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        Map::update_camera(self, update);
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        Map::cancel_transitions_with_id(self, transition_id);
    }
}

impl GestureCamera for Transform {
    fn viewport(&self) -> ScreenCoordinate {
        let size = self.get_state().get_size();
        ScreenCoordinate::new(f64::from(size.width), f64::from(size.height))
    }

    fn camera(&self) -> CameraOptions {
        self.get_camera_options(None)
    }

    fn zoom_range(&self) -> (f64, f64) {
        (self.get_state().get_min_zoom(), self.get_state().get_max_zoom())
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        self.screen_coordinate_to_lat_lng(&pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        let applied = Transform::apply_camera_delta(self, delta);
        debug_assert!(applied.is_ok(), "{applied:?}");
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        Transform::update_camera(self, update);
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        Transform::cancel_transitions_with_id(self, transition_id);
    }
}
