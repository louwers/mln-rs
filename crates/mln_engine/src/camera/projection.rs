use geometry::Point;
use glam::{DMat4, DVec2, DVec3, DVec4};
use mln_style::constants;
use mln_util::projection::Projection as WebMercator;

use crate::camera::vertical_perspective_projection::VerticalPerspectiveProjection;
use crate::camera::view::View;
use crate::tile::UnwrappedTileID;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectionData {
    pub main_matrix: DMat4,
    pub tile_mercator_coords: DVec4,
    pub clipping_plane: DVec4,
    pub projection_transition: f64,
    pub fallback_matrix: DMat4,
    /// Clip-space Z shift for this drawable's layer, the same one the
    /// Mercator matrix carries.
    pub depth_offset: f64,
    /// The layer's translation in tile units, added on the sphere; the
    /// fallback matrix already carries it.
    pub translate: DVec2,
    /// The camera on the unit sphere's scale, for the line of sight to points
    /// above the surface.
    pub(crate) camera_position: DVec3,
    /// On the globe, the point on the unit sphere under the middle of the
    /// tile, which the shaders place the tile's geometry relative to; zero on
    /// Mercator.
    pub sphere_anchor: DVec3,
    /// The sine and cosine of the anchor's longitude, then of its latitude,
    /// in the angles of [`VerticalPerspectiveProjection::tile_coordinates_to_sphere`].
    pub sphere_anchor_angles: DVec4,
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
pub(crate) fn mercator_tile_matrix(tile_id: &UnwrappedTileID, scale: f64) -> DMat4 {
    let tile_scale: u64 = 1u64 << tile_id.canonical.z;
    let s = WebMercator::world_size(scale) / tile_scale as f64;
    let x = (i64::from(tile_id.canonical.x) + i64::from(tile_id.wrap) * tile_scale as i64) as f64 * s;
    let y = i64::from(tile_id.canonical.y) as f64 * s;
    let unit = s / f64::from(constants::EXTENT);
    DMat4::from_translation(DVec3::new(x, y, 0.0)) * DMat4::from_scale(DVec3::new(unit, unit, 1.0))
}

/// The Mercator extent of a tile, the same on every projection.
#[must_use]
pub(crate) fn mercator_tile_coords(tile_id: &UnwrappedTileID) -> DVec4 {
    let tile_scale = (1u64 << tile_id.canonical.z) as f64;
    DVec4::new(
        f64::from(tile_id.canonical.x) / tile_scale,
        f64::from(tile_id.canonical.y) / tile_scale,
        1.0 / tile_scale / f64::from(constants::EXTENT),
        1.0 / tile_scale / f64::from(constants::EXTENT),
    )
}

/// How a [`View`] places tiles on the screen. Both variants
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
    pub fn projection_data(self, view: &View, tile_id: &UnwrappedTileID, mercator_matrix: &DMat4) -> ProjectionData {
        match self {
            Projection::Mercator => ProjectionData {
                main_matrix: *mercator_matrix,
                tile_mercator_coords: mercator_tile_coords(tile_id),
                clipping_plane: DVec4::ZERO,
                projection_transition: 0.0,
                fallback_matrix: *mercator_matrix,
                ..ProjectionData::default()
            },
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::projection_data(view, tile_id, mercator_matrix)
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
                let pos = data.main_matrix * DVec4::new(point.x, point.y, elevation, 1.0);
                ProjectedTilePoint {
                    point: Point::new(pos.x / pos.w, pos.y / pos.w),
                    signed_distance_from_camera: pos.w,
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
    pub fn circle_radius_correction(self, view: &View) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => VerticalPerspectiveProjection::circle_radius_correction(view),
        }
    }

    /// Ratio of a screen pixel at the map center to one at the same zoom on
    /// Mercator, blended the same way.
    #[must_use]
    pub fn pixel_scale(self, view: &View) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => VerticalPerspectiveProjection::pixel_scale(view),
        }
    }

    /// `circle_radius_correction` extended to a point away from the center,
    /// by its latitude, blended the same way.
    #[must_use]
    pub fn pitched_text_correction(self, view: &View, point: &Point<f64>, tile_id: &UnwrappedTileID) -> f64 {
        match self {
            Projection::Mercator => 1.0,
            Projection::VerticalPerspective => {
                VerticalPerspectiveProjection::pitched_text_correction(view, point, tile_id)
            }
        }
    }
}
