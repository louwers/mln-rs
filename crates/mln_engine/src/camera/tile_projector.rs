use geometry::Point;

use crate::camera::projection::{ProjectedTilePoint, ProjectionData};
use crate::camera::view::View;
use crate::tile::UnwrappedTileID;

/// Projects the points of one tile through the current projection, the way
/// the vertex shaders do it.
#[derive(Clone, Copy)]
pub struct TileProjector<'a> {
    view: &'a View,
    tile_id: UnwrappedTileID,
    data: ProjectionData,
}

impl<'a> TileProjector<'a> {
    #[must_use]
    pub fn new(view: &'a View, tile_id: &UnwrappedTileID) -> Self {
        Self::with_data(view, tile_id, view.projection_data(tile_id))
    }

    #[must_use]
    pub fn with_data(view: &'a View, tile_id: &UnwrappedTileID, data: ProjectionData) -> Self {
        TileProjector { view, tile_id: *tile_id, data }
    }

    /// GL JS's `projectTileCoordinates`, for queries and label placement.
    #[must_use]
    pub fn project(&self, point: &Point<f64>, elevation: f64) -> ProjectedTilePoint {
        self.view.projection().project_tile_point(&self.data, &self.tile_id, point, elevation)
    }

    /// Where the renderer draws the point, for the rendered feature capture.
    #[must_use]
    pub fn project_as_drawn(&self, point: &Point<f64>, elevation: f64) -> ProjectedTilePoint {
        self.view.projection().drawn_tile_point(&self.data, &self.tile_id, point, elevation)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn circle_radius_correction(&self) -> f64 {
        self.view.projection().circle_radius_correction(self.view)
    }

    #[must_use]
    pub fn pitched_text_correction(&self, tile_anchor: &Point<f64>) -> f64 {
        self.view.projection().pitched_text_correction(self.view, tile_anchor, &self.tile_id)
    }

    #[must_use]
    pub fn view(&self) -> &View {
        self.view
    }

    #[must_use]
    pub fn tile_id(&self) -> &UnwrappedTileID {
        &self.tile_id
    }
}
