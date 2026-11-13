// gtests-core: added for test/map/map.test.cpp (LatLngForPixelUnwrapped,
// PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian).

use mln_map_types::{Camera, CameraOptions, MapView, TransformState};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use crate::map::Map;
use crate::transform::Transform;

/// Converts coordinates for one camera and size, without a map. It holds only
/// a transform state, so it can move to another thread.
pub struct MapProjection {
    state: TransformState,
}

const _: () = {
    const fn sendable<T: Send>() {}
    sendable::<MapProjection>();
};

impl MapProjection {
    pub fn new(map: &Map) -> Self {
        Self::from_state(map.get_transform_state())
    }

    pub(crate) fn from_state(state: TransformState) -> Self {
        MapProjection { state }
    }

    /// A projection for one view of a map, from a [`MapSnapshot`] or a
    /// rendered frame. `Err`: as [`MapView::to_state`].
    ///
    /// [`MapSnapshot`]: crate::MapSnapshot
    pub fn from_view(view: &MapView) -> Result<Self, Error> {
        Ok(Self::from_state(view.to_state()?))
    }

    /// The screen point of `lat_lng`, in the copy of the world nearest the
    /// camera's center.
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let mut point = self.state.lat_lng_to_screen_coordinate(&self.state.lat_lng_near_center(lat_lng));
        point.y = f64::from(self.state.get_size().height) - point.y;
        point
    }

    /// Whether the globe hides `lat_lng` from the camera; a Mercator view
    /// hides nothing.
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.is_location_occluded(lat_lng)
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or infinite
    /// pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        let mut flipped = *pixel;
        flipped.y = f64::from(self.state.get_size().height) - flipped.y;
        self.state.screen_coordinate_to_lat_lng(&flipped, wrap_mode)
    }

    pub fn set_camera(&mut self, camera: &CameraOptions) {
        let mut transform = self.transform();
        transform.jump_to(camera);
        self.state = transform.get_state().clone();
    }

    /// The camera, with its center in the copy of the world that it is in.
    pub fn camera(&self) -> Camera {
        self.state.camera()
    }

    /// A transform over a copy of the state, to move its camera as a map
    /// would.
    fn transform(&self) -> Transform {
        Transform::from_state(self.state.clone())
    }
}
