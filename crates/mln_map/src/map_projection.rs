// gtests-core: added for test/map/map.test.cpp (LatLngForPixelUnwrapped,
// PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian).

use std::sync::Arc;

use mln_map_types::{Camera, CameraOptions, Elevation, MapView, TransformState};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use crate::map::Map;
use crate::transform::{Transform, flip_y, project_on_ground, unproject_on_ground};

/// Converts coordinates for one camera and size, without a map, on the flat
/// map or on the terrain's ground ([`with_ground`](Self::with_ground)). It
/// holds only a transform state and the ground, so it can move to another
/// thread.
pub struct MapProjection {
    state: TransformState,
    ground: Option<Arc<dyn Elevation>>,
}

const _: () = {
    const fn sendable<T: Send>() {}
    sendable::<MapProjection>();
};

impl MapProjection {
    /// A projection for the map's camera, on the ground its camera stands
    /// on ([`Map::ground`]).
    #[must_use]
    pub fn new(map: &Map) -> Self {
        Self::from_state(map.transform_state().clone()).with_ground(map.ground())
    }

    pub(crate) fn from_state(state: TransformState) -> Self {
        MapProjection { state, ground: None }
    }

    /// A projection for one view of a map, from a [`MapSnapshot`] or a
    /// rendered frame, on the flat map. `Err`: as [`MapView::to_state`].
    ///
    /// [`MapSnapshot`]: crate::MapSnapshot
    pub fn from_view(view: &MapView) -> Result<Self, Error> {
        Ok(Self::from_state(view.to_state()?))
    }

    /// The projection on `ground`, the terrain a map's camera stands on
    /// ([`Map::ground`]), as the map converts on it; on the flat map for
    /// `None`.
    #[must_use]
    pub fn with_ground(self, ground: Option<Arc<dyn Elevation>>) -> Self {
        MapProjection { ground, ..self }
    }

    /// The screen point of `lat_lng`, in the copy of the world nearest the
    /// camera's center: standing on the ground where there is one and it
    /// was drawn, else at sea level.
    #[must_use]
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let view = self.state.view();
        let lat_lng = view.lat_lng_near_center(lat_lng);
        let point = match &self.ground {
            Some(ground) => project_on_ground(view, Some(ground.as_ref()), &lat_lng),
            None => view.lat_lng_to_screen_coordinate(&lat_lng),
        };
        flip_y(view, &point)
    }

    /// Whether the globe hides `lat_lng` from the camera; a Mercator view
    /// hides nothing.
    #[must_use]
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.view().is_location_occluded(lat_lng)
    }

    /// The location under `pixel`: with a ground, where the line of sight
    /// first meets it, or the plane at the center's altitude where it meets
    /// no drawn ground; else on the flat map. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or
    /// infinite pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        let view = self.state.view();
        let point = flip_y(view, pixel);
        match &self.ground {
            Some(ground) => unproject_on_ground(view, Some(ground.as_ref()), &point, wrap_mode),
            None => view.screen_coordinate_to_lat_lng(&point, wrap_mode),
        }
    }

    pub fn set_camera(&mut self, camera: &CameraOptions) {
        let mut transform = self.transform();
        transform.jump_to(camera);
        self.state = transform.state().clone();
    }

    /// The camera, with its center in the copy of the world that it is in.
    #[must_use]
    pub fn camera(&self) -> Camera {
        self.state.view().camera()
    }

    /// A transform over a copy of the state, to move its camera as a map
    /// would.
    fn transform(&self) -> Transform {
        Transform::from_state(self.state.clone())
    }
}
