use geometry::Point;
use mln_tile_id::{GeometryCoordinate, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::projection::Projection;

use crate::transform_state::TransformState;

pub type TileCoordinatePoint = Point<f64>;

/// Has floating point x/y coordinates. Used for computing the tiles that
/// need to be visible in the viewport. In mapbox-gl-js, this is named
/// `MercatorCoordinate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileCoordinate {
    pub p: TileCoordinatePoint,
    pub z: f64,
}

impl TileCoordinate {
    #[must_use]
    pub fn from_lat_lng(zoom: f64, lat_lng: &LatLng) -> TileCoordinate {
        let scale = 2.0f64.powf(zoom);
        let p = Projection::project(lat_lng, scale);
        TileCoordinate { p: Point::new(p.x / constants::TILE_SIZE, p.y / constants::TILE_SIZE), z: zoom }
    }

    pub fn from_screen_coordinate(
        state: &TransformState,
        zoom: u8,
        screen_coordinate: &ScreenCoordinate,
    ) -> TileCoordinate {
        state.screen_coordinate_to_tile_coordinate(screen_coordinate, zoom)
    }

    #[must_use]
    pub fn zoom_to(&self, zoom: f64) -> TileCoordinate {
        let scale_diff = 2.0f64.powf(zoom - self.z);
        TileCoordinate { p: Point::new(self.p.x * scale_diff, self.p.y * scale_diff), z: zoom }
    }

    #[must_use]
    pub fn to_geometry_coordinate(tile_id: &UnwrappedTileID, point: &TileCoordinatePoint) -> GeometryCoordinate {
        let scale = 2.0f64.powf(f64::from(tile_id.canonical.z));
        let zoomed = TileCoordinate { p: *point, z: 0.0 }.zoom_to(f64::from(tile_id.canonical.z));
        let clamp = |v: f64| -> i16 {
            // util::clamp<int64_t>(static_cast<int64_t>(v), INT16_MIN, INT16_MAX)
            let v = v as i64;
            v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
        };
        Point::new(
            clamp(
                (zoomed.p.x - f64::from(tile_id.canonical.x) - f64::from(tile_id.wrap) * scale)
                    * f64::from(constants::EXTENT),
            ),
            clamp((zoomed.p.y - f64::from(tile_id.canonical.y)) * f64::from(constants::EXTENT)),
        )
    }
}
