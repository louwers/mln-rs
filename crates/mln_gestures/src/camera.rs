//! The camera the recognizer moves: a [`Map`], or a bare [`Transform`] (the
//! tests, which need no renderer).

use mln_map::transform::Transform;
use mln_map::{AnimationOptions, CameraMomentum, CameraOptions, Map};
use mln_util::LatLng;
use mln_util::geo::{ScreenCoordinate, WrapMode};

/// The camera moves the recognizer makes, which [`Map`] and [`Transform`]
/// provide.
pub trait GestureCamera {
    /// The viewport's width and height, in logical pixels.
    fn viewport(&self) -> ScreenCoordinate;
    fn camera(&self) -> CameraOptions;
    /// The least and greatest zoom the camera may take.
    fn zoom_range(&self) -> (f64, f64);
    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng>;
    fn ease_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions);
    fn move_by(&mut self, offset: ScreenCoordinate, animation: &AnimationOptions);
    fn start_momentum(&mut self, momentum: &CameraMomentum, animation: &AnimationOptions);
    fn cancel_transitions(&mut self);
    fn cancel_transitions_with_id(&mut self, transition_id: u64);
    fn set_gesture_in_progress(&mut self, in_progress: bool);
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

    fn ease_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions) {
        Map::ease_to(self, camera, animation);
    }

    fn move_by(&mut self, offset: ScreenCoordinate, animation: &AnimationOptions) {
        // Only a NaN or infinite offset fails, which no gesture produces.
        let _ = Map::move_by(self, &offset, animation);
    }

    fn start_momentum(&mut self, momentum: &CameraMomentum, animation: &AnimationOptions) {
        Map::start_momentum(self, momentum, animation);
    }

    fn cancel_transitions(&mut self) {
        Map::cancel_transitions(self);
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        Map::cancel_transitions_with_id(self, transition_id);
    }

    fn set_gesture_in_progress(&mut self, in_progress: bool) {
        Map::set_gesture_in_progress(self, in_progress);
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

    fn ease_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions) {
        Transform::ease_to(self, camera, animation);
    }

    fn move_by(&mut self, offset: ScreenCoordinate, animation: &AnimationOptions) {
        let _ = Transform::move_by(self, &offset, animation);
    }

    fn start_momentum(&mut self, momentum: &CameraMomentum, animation: &AnimationOptions) {
        Transform::start_momentum(self, momentum, animation);
    }

    fn cancel_transitions(&mut self) {
        Transform::cancel_transitions(self);
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        Transform::cancel_transitions_with_id(self, transition_id);
    }

    fn set_gesture_in_progress(&mut self, in_progress: bool) {
        Transform::set_gesture_in_progress(self, in_progress);
    }
}
