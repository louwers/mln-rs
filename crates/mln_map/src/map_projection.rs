// gtests-core: added for test/map/map.test.cpp (LatLngForPixelUnwrapped,
// PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian).

use mln_map_types::{Camera, CameraOptions, MapView, TransformState};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use crate::map::Map;
use crate::transform::{TerrainGround, Transform, flip_y, project_on_ground, unproject_on_ground};

/// Converts coordinates for one camera and size, without a map, on the flat
/// map or on the terrain's ground ([`with_terrain`](Self::with_terrain)). It
/// holds only a transform state and the terrain, so it can move to another
/// thread.
pub struct MapProjection {
    state: TransformState,
    terrain: Option<TerrainGround>,
}

const _: () = {
    const fn sendable<T: Send>() {}
    sendable::<MapProjection>();
};

impl MapProjection {
    /// A projection for the map's camera, on the terrain its camera stands
    /// on ([`Map::terrain`]).
    #[must_use]
    pub fn new(map: &Map) -> Self {
        Self::from_state(map.transform_state().clone()).with_terrain(map.terrain())
    }

    pub(crate) fn from_state(state: TransformState) -> Self {
        MapProjection { state, terrain: None }
    }

    /// A projection for one view of a map, from a [`MapSnapshot`] or a
    /// rendered frame, on the flat map. `Err`: as [`MapView::to_state`].
    ///
    /// [`MapSnapshot`]: crate::MapSnapshot
    pub fn from_view(view: &MapView) -> Result<Self, Error> {
        Ok(Self::from_state(view.to_state()?))
    }

    /// The projection on `terrain`, the terrain a map's camera stands on
    /// ([`Map::terrain`]), as the map converts on it, on any projection; on
    /// the flat map for `None`. The view's near and far planes fit the
    /// ground given, as the map's do, its lowest under the center, or the
    /// flat map's without one, whatever terrain the projection had before.
    #[must_use]
    pub fn with_terrain(mut self, terrain: Option<TerrainGround>) -> Self {
        let ground = terrain.as_ref().and_then(|terrain| terrain.elevation.as_ref());
        let center = self.state.view().center(WrapMode::Wrapped);
        let min_elevation = ground.map(|ground| ground.min_at(center).unwrap_or(0.0));
        if min_elevation != self.state.view().terrain_min_elevation() {
            self.state.set_terrain_min_elevation(min_elevation);
        }
        MapProjection { terrain, ..self }
    }

    /// The screen point of `lat_lng`, in the copy of the world nearest the
    /// camera's center: standing on the ground where there is terrain and it
    /// was drawn, else at sea level.
    #[must_use]
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let view = self.state.view();
        let lat_lng = view.lat_lng_near_center(lat_lng);
        let point = match &self.terrain {
            Some(terrain) => project_on_ground(view, terrain.elevation.as_deref(), &lat_lng),
            None => view.lat_lng_to_screen_coordinate(&lat_lng),
        };
        flip_y(view, &point)
    }

    /// The height of the ground at `lat_lng`, in meters with the terrain's
    /// exaggeration, as the frame drew it ([`Elevation::at`]). `None` without
    /// terrain, which includes the flat map of
    /// [`from_view`](Self::from_view), before any ground was drawn, off the
    /// drawn tiles, and where the ground's data still loads; 0 where no data
    /// will come.
    ///
    /// [`Elevation::at`]: mln_map_types::Elevation::at
    #[must_use]
    pub fn terrain_elevation(&self, lat_lng: &LatLng) -> Option<f64> {
        self.terrain.as_ref()?.elevation.as_ref()?.at(*lat_lng)
    }

    /// Whether the globe hides `lat_lng` from the camera; a Mercator view
    /// hides nothing.
    #[must_use]
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.state.view().is_location_occluded(lat_lng)
    }

    /// The location under `pixel`: with terrain, where the line of sight
    /// first meets its ground, or the plane at the center's altitude where
    /// it meets no drawn ground or before any was drawn; else on the flat
    /// map. Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind)
    /// for a NaN or infinite pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        let view = self.state.view();
        let point = flip_y(view, pixel);
        match &self.terrain {
            Some(terrain) => unproject_on_ground(view, terrain.elevation.as_deref(), &point, wrap_mode),
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
