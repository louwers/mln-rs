// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up in the store
// by the tile's slot and id (`FrameTile`); the store changes them only in
// the frame's update, before the layers read them.

use glam::DMat4;
use mln_map_types::{MapDebugOptions, ProjectionData};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};

use crate::buckets::debug_bucket::DebugBucket;
use crate::paint_parameters::TransformParameters;
use crate::render_source::RenderTileParameters;
use crate::tile::contents::Contents;
use crate::tile::raster::RasterContents;
use crate::tile::tile::Tile;

pub(crate) struct RenderTile {
    pub(crate) id: UnwrappedTileID,
    /// The projection contract for this tile; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: DMat4,
    /// The tile's debug overlay, when the map's debug options show one.
    pub(crate) debug_bucket: Option<Box<DebugBucket>>,

    /// `tile.id`.
    tile_id: OverscaledTileID,
    tile_hold_for_fade: bool,
    /// Where the tile is among its source's, which finds its contents
    /// first; `None` for a tile that isn't among a source's.
    slot: Option<usize>,
    needs_rendering: bool,
}

impl RenderTile {
    /// The render tile of `tile`, at `slot` of its source's tiles, drawn as
    /// `id`, before its first update.
    pub(crate) fn new(id: UnwrappedTileID, tile: &Tile, slot: usize, hold_for_fade: bool) -> Self {
        RenderTile { slot: Some(slot), ..RenderTile::of_tile_id(id, tile.id, hold_for_fade) }
    }

    /// The render tile of the tile `tile_id`, drawn as `id`, before it is
    /// updated, which isn't among a source's tiles.
    pub(crate) fn of_tile_id(id: UnwrappedTileID, tile_id: OverscaledTileID, hold_for_fade: bool) -> Self {
        RenderTile {
            id,
            projection: ProjectionData::default(),
            // `set_projection` sets it.
            matrix: DMat4::ZERO,
            tile_id,
            tile_hold_for_fade: hold_for_fade,
            slot: None,
            needs_rendering: false,
            debug_bucket: None,
        }
    }

    #[must_use]
    pub(crate) fn overscaled_tile_id(&self) -> &OverscaledTileID {
        &self.tile_id
    }

    /// Where the tile is among its source's, as of its last update; `None`
    /// for a tile that isn't among a source's.
    #[must_use]
    pub(crate) fn slot(&self) -> Option<usize> {
        self.slot
    }

    #[must_use]
    pub(crate) fn hold_for_fade(&self) -> bool {
        self.tile_hold_for_fade
    }

    #[must_use]
    pub(crate) fn needs_rendering(&self) -> bool {
        self.needs_rendering
    }

    /// Updates the render tile for the frame; `tile` is the tile it was
    /// created for.
    pub(crate) fn update(&mut self, tile: &Tile, needs_rendering: bool, parameters: &RenderTileParameters<'_>) {
        self.needs_rendering = needs_rendering;
        let (modified, expires) = (tile.load.freshness.modified, tile.load.freshness.expires);

        let needs_debug_bucket = match &self.debug_bucket {
            None => true,
            Some(debug_bucket) => {
                debug_bucket.renderable != tile.is_renderable()
                    || debug_bucket.complete != tile.is_complete()
                    || debug_bucket.modified != modified
                    || debug_bucket.expires != expires
                    || debug_bucket.debug_mode != parameters.debug_options
            }
        };
        if parameters.debug_options != MapDebugOptions::NO_DEBUG && needs_debug_bucket {
            self.debug_bucket = Some(Box::new(DebugBucket::new(
                &tile.id,
                tile.is_renderable(),
                tile.is_complete(),
                modified,
                expires,
                parameters.debug_options,
                &parameters.source_name,
            )));
        } else if parameters.debug_options == MapDebugOptions::NO_DEBUG {
            self.debug_bucket = None;
        }

        self.set_projection(parameters.transform);
    }

    /// Sets the tile's projection and its standard matrix for `transform`.
    pub(crate) fn set_projection(&mut self, transform: &TransformParameters) {
        self.projection = transform.view.projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}

/// What a frame's tile holds, which a layer's [`TileInput`](crate::layer::TileInput)
/// reads.
#[derive(Clone, Copy)]
pub(crate) enum TileContent<'a> {
    /// The tile's contents in the tile store.
    Tile(&'a Contents),
    /// The image of an image source, which each of the source's tiles
    /// shows.
    Image(&'a RasterContents),
}

/// Where a tile's content keeps what it holds for a layer. The frame's
/// found tiles keep it for layers of every type in one list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewPlace {
    /// The layer's place among the layers of the tile's layout.
    Laid(usize),
    /// The tile's own data, a raster or raster-dem tile's contents or an
    /// image source's image, which every layer of its source draws.
    Own,
}

/// A layer's tile of a frame, as the frame's update found it: its index
/// among its source's render tiles, and where what it holds for the layer
/// is in its contents.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoundTile {
    pub(crate) tile: usize,
    pub(crate) view: Option<ViewPlace>,
}
