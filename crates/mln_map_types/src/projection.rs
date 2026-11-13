use geometry::Point;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants;
use mln_util::mat4::{Mat4, matrix};
use mln_util::projection::Projection as WebMercator;
use mln_util::vectors::{Vec2, Vec3, Vec4};

use crate::transform_state::TransformState;
use crate::vertical_perspective_projection::VerticalPerspectiveProjection;

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
    pub(crate) camera_position: Vec3,
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
pub(crate) fn mercator_tile_matrix(matrix: &mut Mat4, tile_id: &UnwrappedTileID, scale: f64) {
    let tile_scale: u64 = 1u64 << tile_id.canonical.z;
    let s = WebMercator::world_size(scale) / tile_scale as f64;

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
pub(crate) fn mercator_tile_coords(tile_id: &UnwrappedTileID) -> Vec4 {
    let tile_scale = (1u64 << tile_id.canonical.z) as f64;
    [
        f64::from(tile_id.canonical.x) / tile_scale,
        f64::from(tile_id.canonical.y) / tile_scale,
        1.0 / tile_scale / f64::from(constants::EXTENT),
        1.0 / tile_scale / f64::from(constants::EXTENT),
    ]
}

/// How a [`TransformState`] places tiles on the screen. Both variants
/// project locations onto the same Mercator plane, so the tile-to-world matrix
/// is `mercator_tile_matrix` on either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Projection {
    #[default]
    Mercator,
    /// The globe, also while it hands over to or from Mercator.
    VerticalPerspective,
}

impl Projection {
    /// The per-tile projection contract, given the Mercator tile-to-clip
    /// matrix the renderer already computed.
    #[must_use]
    pub fn get_projection_data(
        self,
        state: &TransformState,
        tile_id: &UnwrappedTileID,
        mercator_matrix: &Mat4,
    ) -> ProjectionData {
        match self {
            Projection::Mercator => ProjectionData {
                main_matrix: *mercator_matrix,
                tile_mercator_coords: mercator_tile_coords(tile_id),
                clipping_plane: [0.0, 0.0, 0.0, 0.0],
                projection_transition: 0.0,
                fallback_matrix: *mercator_matrix,
                ..ProjectionData::default()
            },
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::projection_data(state, tile_id, mercator_matrix)
            }
        }
    }

    /// The clip-space position of a tile point, `elevation` meters above the
    /// surface, computed the way the vertex shaders do it.
    #[must_use]
    pub(crate) fn project_tile_point(
        self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        match self {
            Projection::Mercator => {
                let mut pos: Vec4 = [point.x, point.y, elevation, 1.0];
                let a = pos;
                matrix::transform_mat4(&mut pos, &a, &data.main_matrix);
                ProjectedTilePoint {
                    point: Point::new(pos[0] / pos[3], pos[1] / pos[3]),
                    signed_distance_from_camera: pos[3],
                    occluded: false,
                }
            }
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::project_tile_point(data, tile_id, point, elevation)
            }
        }
    }

    /// Where the vertex shaders draw a tile point: `project_tile_point`,
    /// except while one projection hands over to another.
    #[must_use]
    pub(crate) fn drawn_tile_point(
        self,
        data: &ProjectionData,
        tile_id: &UnwrappedTileID,
        point: &Point<f64>,
        elevation: f64,
    ) -> ProjectedTilePoint {
        match self {
            Projection::Mercator => self.project_tile_point(data, tile_id, point, elevation),
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::drawn_tile_point(data, tile_id, point, elevation)
            }
        }
    }

    /// Scale that keeps map-aligned circles and pitched text the size they
    /// have on Mercator at the map center, blended with Mercator's 1 through
    /// the projection transition as GL JS blends it.
    #[must_use]
    pub fn circle_radius_correction(self, state: &TransformState) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => VerticalPerspectiveProjection::circle_radius_correction(state),
        }
    }

    /// Ratio of a screen pixel at the map center to one at the same zoom on
    /// Mercator, blended the same way.
    #[must_use]
    pub fn pixel_scale(self, state: &TransformState) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => VerticalPerspectiveProjection::pixel_scale(state),
        }
    }

    /// `circle_radius_correction` extended to a point away from the center,
    /// by its latitude, blended the same way.
    #[must_use]
    pub fn pitched_text_correction(self, state: &TransformState, point: &Point<f64>, tile_id: &UnwrappedTileID) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::pitched_text_correction(state, point, tile_id)
            }
        }
    }
}
