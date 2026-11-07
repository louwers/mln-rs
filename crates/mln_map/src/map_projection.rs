// Ports include/mln/map/map_projection.hpp and src/mln/map/map_projection.cpp.
//
// gtests-core: added for test/map/map.test.cpp (LatLngForPixelUnwrapped,
// PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian).

use mln_map_types::{CameraOptions, TransformState};
use mln_util::geo::{EdgeInsets, LatLng, ScreenCoordinate, WrapMode};

use crate::map::{Map, camera_for_lat_lngs};
use crate::transform::Transform;

pub struct MapProjection {
    transform: Transform,
}

impl MapProjection {
    /// `MapProjection(const Map&)`.
    pub fn new(map: &Map) -> Self {
        Self::from_state(map.get_transform_state())
    }

    /// `MapProjection(const TransformState&)`.
    pub fn from_state(state: TransformState) -> Self {
        MapProjection { transform: Transform::from_state(state) }
    }

    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        // The implementation is just a copy from map.cpp
        self.transform.lat_lng_to_screen_coordinate(&self.transform.get_state().lat_lng_near_center(lat_lng))
    }

    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> LatLng {
        // The implementation is just a copy from map.cpp
        self.transform.screen_coordinate_to_lat_lng(pixel, wrap_mode)
    }

    pub fn set_camera(&mut self, camera: &CameraOptions) {
        self.transform.jump_to(camera);
    }

    pub fn get_camera(&self) -> CameraOptions {
        self.transform.get_camera_options(None)
    }

    /// Set the underneath camera so the requested coordinates are visible
    /// with the inset.
    pub fn set_visible_coordinates(&mut self, lat_lngs: &[LatLng], padding: &EdgeInsets) {
        let camera = camera_for_lat_lngs(lat_lngs, &self.transform, padding)
            .with_bearing(Some(mln_math::rad2deg(-self.transform.get_bearing())))
            .with_pitch(Some(mln_math::rad2deg(self.transform.get_pitch())));
        self.transform.jump_to(&camera);
    }
}
