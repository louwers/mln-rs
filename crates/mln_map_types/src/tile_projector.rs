// Ports src/mln/map/tile_projector.hpp.

use geometry::Point;
use mln_tile_id::UnwrappedTileID;

use crate::projection::{ProjectedTilePoint, ProjectionData};
use crate::transform_state::TransformState;

/// Projects the points of one tile through the current projection, the way
/// the vertex shaders do it.
#[derive(Clone, Copy)]
pub struct TileProjector<'a> {
    state: &'a TransformState,
    tile_id: UnwrappedTileID,
    data: ProjectionData,
}

impl<'a> TileProjector<'a> {
    pub fn new(state: &'a TransformState, tile_id: &UnwrappedTileID) -> Self {
        Self::with_data(state, tile_id, state.get_projection_data(tile_id))
    }

    /// `TileProjector(const TransformState&, const UnwrappedTileID&, ProjectionData)`.
    pub fn with_data(state: &'a TransformState, tile_id: &UnwrappedTileID, data: ProjectionData) -> Self {
        TileProjector { state, tile_id: *tile_id, data }
    }

    /// GL JS's `projectTileCoordinates`, for queries and label placement.
    /// (C++ defaults `elevation` to 0.)
    #[must_use]
    pub fn project(&self, point: &Point<f64>, elevation: f64) -> ProjectedTilePoint {
        self.state.get_projection().project_tile_point(&self.data, &self.tile_id, point, elevation)
    }

    /// Where the renderer draws the point, for the rendered feature capture.
    /// (C++ defaults `elevation` to 0.)
    #[must_use]
    pub fn project_as_drawn(&self, point: &Point<f64>, elevation: f64) -> ProjectedTilePoint {
        self.state.get_projection().drawn_tile_point(&self.data, &self.tile_id, point, elevation)
    }

    #[must_use]
    pub fn circle_radius_correction(&self) -> f64 {
        self.state.get_projection().circle_radius_correction(self.state)
    }

    #[must_use]
    pub fn pitched_text_correction(&self, tile_anchor: &Point<f64>) -> f64 {
        self.state.get_projection().pitched_text_correction(self.state, tile_anchor, &self.tile_id)
    }

    #[must_use]
    pub fn get_transform_state(&self) -> &TransformState {
        self.state
    }

    #[must_use]
    pub fn get_tile_id(&self) -> &UnwrappedTileID {
        &self.tile_id
    }

    #[must_use]
    pub fn get_projection_data(&self) -> &ProjectionData {
        &self.data
    }
}
