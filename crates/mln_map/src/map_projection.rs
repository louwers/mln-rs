// gtests-core: added for test/map/map.test.cpp (LatLngForPixelUnwrapped,
// PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian).

use std::sync::Arc;

use mln_map_types::{Camera, CameraOptions, Elevation, MapView, TransformState, View};
use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};
use mln_util::geo::ScreenCoordinate;

use crate::map::Map;
use crate::transform::{TerrainGround, Transform, flip_y, lowest_ground};

/// Converts coordinates for one camera and size, without a map: on the
/// ground a renderer drew of a terrain ([`with_ground`](Self::with_ground)),
/// or else on the flat map. It holds only a transform state and the ground,
/// so it can move to another thread.
pub struct MapProjection {
    state: TransformState,
    ground: Option<Arc<dyn Elevation>>,
}

const _: () = {
    const fn sendable<T: Send>() {}
    sendable::<MapProjection>();
};

impl MapProjection {
    /// A projection for the map's camera, on the ground it draws
    /// ([`Map::ground`]), as the map converts.
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

    /// The projection on `ground`, the ground a renderer drew of a terrain
    /// ([`Map::ground`]), or on the flat map for `None`. The view's near and
    /// far planes fit that ground as the map's do, which the line of sight
    /// to the ground lies between.
    #[must_use]
    pub fn with_ground(mut self, ground: Option<Arc<dyn Elevation>>) -> Self {
        self.ground = ground;
        self.fit_planes();
        self
    }

    /// Fits the view's near and far planes to the ground under the center,
    /// or to the flat map without ground.
    fn fit_planes(&mut self) {
        let lowest = self.ground.as_deref().map(|ground| lowest_ground(self.state.view(), ground));
        if lowest != self.state.view().terrain_min_elevation() {
            self.state.set_terrain_min_elevation(lowest);
        }
    }

    /// The screen point of `lat_lng`, in the copy of the world nearest the
    /// camera's center: standing on the projection's ground, or at sea
    /// level on the flat map ([`Map::pixel_for_lat_lng`]).
    #[must_use]
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        pixel_for_lat_lng(self.state.view(), self.ground.as_deref(), lat_lng)
    }

    /// The height the projection's ground was drawn at, at `lat_lng`, in
    /// meters with the terrain's exaggeration
    /// ([`View::drawn_height_at`](mln_map_types::View::drawn_height_at)).
    /// `None` on the flat map, and where no ground was drawn.
    #[must_use]
    pub fn elevation(&self, lat_lng: &LatLng) -> Option<f64> {
        self.state.view().drawn_height_at(lat_lng, &**self.ground.as_ref()?)
    }

    /// Whether the globe hides `lat_lng` from the camera; a Mercator view
    /// hides nothing.
    #[must_use]
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.view().is_location_occluded(lat_lng)
    }

    /// The location under `pixel`: on the projection's ground, or on the
    /// flat map ([`Map::lat_lng_for_pixel`]). Fails with
    /// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a NaN or
    /// infinite pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        lat_lng_for_pixel(self.state.view(), self.ground.as_deref(), pixel, wrap_mode)
    }

    /// Moves the camera as a map's [`jump_to`](Map::jump_to) would, without
    /// its constraints. On the projection's ground, as on the map's, a move
    /// around an anchor holds the ground under it, and the eye stays above
    /// the ground; the center keeps the altitude it is given. The ground
    /// stays: where the camera now sees ground it doesn't hold, it converts
    /// as where the line of sight meets none.
    pub fn set_camera(&mut self, camera: &CameraOptions) {
        let mut transform = self.transform();
        let terrain = self.ground.clone().map(|ground| TerrainGround { elevation: Some(ground), clamped: false });
        transform.set_terrain(terrain);
        transform.jump_to(camera);
        self.state = transform.state().clone();
        self.fit_planes();
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

/// The screen point (y down) of `lat_lng` in `view`, in the copy of the world
/// nearest its center: standing on `ground`, the ground a renderer drew of a
/// terrain ([`View::lat_lng_to_screen_coordinate_on`]), or at sea level
/// without one.
pub(crate) fn pixel_for_lat_lng(view: &View, ground: Option<&dyn Elevation>, lat_lng: &LatLng) -> ScreenCoordinate {
    let lat_lng = view.lat_lng_near_center(lat_lng);
    let point = match ground {
        Some(ground) => view.lat_lng_to_screen_coordinate_on(&lat_lng, ground),
        None => view.lat_lng_to_screen_coordinate(&lat_lng),
    };
    flip_y(view, &point)
}

/// The location under `pixel` (y down) in `view`: on `ground`
/// ([`View::screen_coordinate_to_lat_lng_on`]), or at sea level without one.
/// Fails as [`View::screen_coordinate_to_lat_lng`].
pub(crate) fn lat_lng_for_pixel(
    view: &View,
    ground: Option<&dyn Elevation>,
    pixel: &ScreenCoordinate,
    wrap_mode: WrapMode,
) -> Result<LatLng, Error> {
    let point = flip_y(view, pixel);
    match ground {
        Some(ground) => view.screen_coordinate_to_lat_lng_on(&point, ground, wrap_mode),
        None => view.screen_coordinate_to_lat_lng(&point, wrap_mode),
    }
}
