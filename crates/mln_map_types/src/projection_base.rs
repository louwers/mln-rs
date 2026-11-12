// Ports src/mln/map/projection_base.hpp.
//
// C++ `TransformState` holds its projection as a
// `std::shared_ptr<const ProjectionBase>`; here it is an
// `Arc<dyn ProjectionBase>`, so a `TransformState` stays cheap to clone and
// `Send`.

use std::fmt;

use geometry::Point;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection;
use mln_util::vectors::{Vec2, Vec3, Vec4};

use crate::transform_state::TransformState;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectionData {
    pub main_matrix: Mat4,
    pub tile_mercator_coords: Vec4,
    pub clipping_plane: Vec4,
    pub projection_transition: f64,
    pub fallback_matrix: Mat4,
    /// Clip-space Z shift for this drawable's layer, the same one the
    /// Mercator matrix carries.
    pub depth_offset: f64,
    /// The layer's translation in tile units, added on the sphere; the
    /// fallback matrix already carries it.
    pub translate: Vec2,
    /// The camera on the unit sphere's scale, for the line of sight to points
    /// above the surface.
    pub camera_position: Vec3,
}

/// A tile point in clip space, as the vertex shaders would place it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectedTilePoint {
    pub point: Point<f64>,
    pub signed_distance_from_camera: f64,
    /// Behind the planet's horizon; never true on Mercator.
    pub occluded: bool,
}

/// The tile-to-world matrix of a tile, the same on every projection.
pub fn mercator_tile_matrix(matrix: &mut Mat4, tile_id: &UnwrappedTileID, scale: f64) {
    let tile_scale: u64 = 1u64 << tile_id.canonical.z;
    let s = Projection::world_size(scale) / tile_scale as f64;

    matrix::identity(matrix);
    let a = *matrix;
    matrix::translate(
        matrix,
        &a,
        (i64::from(tile_id.canonical.x) + i64::from(tile_id.wrap) * tile_scale as i64) as f64 * s,
        i64::from(tile_id.canonical.y) as f64 * s,
        0.0,
    );
    let a = *matrix;
    matrix::scale(matrix, &a, s / f64::from(constants::EXTENT), s / f64::from(constants::EXTENT), 1.0);
}

/// The Mercator extent of a tile, the same on every projection.
#[must_use]
pub fn mercator_tile_coords(tile_id: &UnwrappedTileID) -> Vec4 {
    let tile_scale = (1u64 << tile_id.canonical.z) as f64;
    [
        f64::from(tile_id.canonical.x) / tile_scale,
        f64::from(tile_id.canonical.y) / tile_scale,
        1.0 / tile_scale / f64::from(constants::EXTENT),
        1.0 / tile_scale / f64::from(constants::EXTENT),
    ]
}

pub trait ProjectionBase: fmt::Debug + Send + Sync {
    fn project(&self, lat_lng: &LatLng, scale: f64) -> Point<f64>;

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) when
    /// the point unprojects to an invalid `LatLng`.
    fn unproject(&self, point: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Result<LatLng, Error>;

    fn tile_matrix(&self, matrix: &mut Mat4, tile_id: &UnwrappedTileID, scale: f64);

    /// The per-tile projection contract, given the Mercator tile-to-clip
    /// matrix the renderer already computed.
    fn get_projection_data(
        &self,
        state: &TransformState,
        tile_id: &UnwrappedTileID,
        mercator_matrix: &Mat4,
    ) -> ProjectionData;

    /// The clip-space position of a tile point, `elevation` meters above the
    /// surface, computed the way the vertex shaders do it.
    fn project_tile_point(
        &self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint;

    /// Where the vertex shaders draw a tile point: `project_tile_point`,
    /// except while one projection hands over to another.
    fn drawn_tile_point(
        &self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        self.project_tile_point(data, tile_id, point, elevation)
    }

    /// Scale that keeps map-aligned circles and pitched text the size they
    /// have on Mercator at the map center, blended with Mercator's 1 through
    /// the projection transition as GL JS blends it.
    fn circle_radius_correction(&self, _state: &TransformState) -> f64 {
        1.0
    }

    /// Ratio of a screen pixel at the map center to one at the same zoom on
    /// Mercator, blended the same way.
    fn pixel_scale(&self, _state: &TransformState) -> f64 {
        1.0
    }

    /// `circle_radius_correction` extended to a point away from the center,
    /// by its latitude, blended the same way.
    fn pitched_text_correction(&self, _state: &TransformState, _point: &Point<f64>, _tile_id: &UnwrappedTileID) -> f64 {
        1.0
    }
}
