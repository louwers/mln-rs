// Ports src/mln/map/mercator_projection.hpp and
// src/mln/map/mercator_projection.cpp.

use geometry::Point;
use mln_tile_id::UnwrappedTileID;
use mln_util::error::Error;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;
use mln_util::vectors::Vec4;

use crate::projection_base::{
    ProjectedTilePoint, ProjectionBase, ProjectionData, mercator_tile_coords, mercator_tile_matrix,
};
use crate::transform_state::TransformState;

#[derive(Clone, Copy, Debug, Default)]
pub struct MercatorProjection;

impl ProjectionBase for MercatorProjection {
    fn project(&self, lat_lng: &LatLng, scale: f64) -> Point<f64> {
        Projection::project(lat_lng, scale)
    }

    fn unproject(&self, point: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        Projection::unproject(point, scale, wrap_mode)
    }

    fn tile_matrix(&self, matrix: &mut Mat4, tile_id: &UnwrappedTileID, scale: f64) {
        mercator_tile_matrix(matrix, tile_id, scale);
    }

    fn get_projection_data(
        &self,
        _state: &TransformState,
        tile_id: &UnwrappedTileID,
        mercator_matrix: &Mat4,
    ) -> ProjectionData {
        ProjectionData {
            main_matrix: *mercator_matrix,
            tile_mercator_coords: mercator_tile_coords(tile_id),
            clipping_plane: [0.0, 0.0, 0.0, 0.0],
            projection_transition: 0.0,
            fallback_matrix: *mercator_matrix,
            ..ProjectionData::default()
        }
    }

    fn project_tile_point(
        &self,
        data: &ProjectionData,
        _tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        let mut pos: Vec4 = [point.x, point.y, elevation, 1.0];
        let a = pos;
        matrix::transform_mat4(&mut pos, &a, &data.main_matrix);
        ProjectedTilePoint {
            point: Point::new(pos[0] / pos[3], pos[1] / pos[3]),
            signed_distance_from_camera: pos[3],
            occluded: false,
        }
    }
}
