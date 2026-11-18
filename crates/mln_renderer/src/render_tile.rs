// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up in the store
// by the tile's slot and id (`FrameTile`); the store changes them only in
// the frame's update, before the layers read them.

use glam::Vec2;
use mln_map_types::{MapDebugOptions, ProjectionData, TransformState};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::mat4::Mat4;

use crate::buckets::debug_bucket::DebugBucket;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::layer_type::LayerType;
use crate::paint_parameters::TransformParameters;
use crate::render_pass::RenderPass;
use crate::render_source::RenderTileParameters;
use crate::tile::contents::Contents;
use crate::tile::tile::Tile;

pub(crate) struct RenderTile {
    pub(crate) id: UnwrappedTileID,
    /// The projection contract for this tile; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: Mat4,
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
            matrix: [0.0; 16],
            tile_id,
            tile_hold_for_fade: hold_for_fade,
            slot: None,
            needs_rendering: false,
            debug_bucket: None,
        }
    }

    /// The layer's translation in tile units, rotated for its anchor.
    pub(crate) fn tile_unit_translation(
        id: &UnwrappedTileID,
        translation: [f32; 2],
        anchor: TranslateAnchorType,
        state: &TransformState,
    ) -> [f32; 2] {
        if translation[0] == 0.0 && translation[1] == 0.0 {
            return [0.0, 0.0];
        }
        let angle = if anchor == TranslateAnchorType::Viewport { (-state.get_bearing()) as f32 } else { 0.0f32 };
        let translate = Vec2::from_angle(angle).rotate(Vec2::new(translation[0], translation[1]));
        let zoom = state.get_zoom() as f32;
        [id.pixels_to_tile_units(translate.x, zoom), id.pixels_to_tile_units(translate.y, zoom)]
    }

    #[must_use]
    pub(crate) fn overscaled_tile_id(&self) -> &OverscaledTileID {
        &self.tile_id
    }

    /// Where the tile is among its source's, as of its last update; `None` for a
    /// tile that isn't among a source's.
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
        let (modified, expires) = tile.load.freshness();

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
        self.projection = transform.state.get_projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}

/// What a frame's tile shows a layer of some type: its bucket in the tile's
/// layout, or a raster or raster-dem tile's own bucket. A frame finds it by
/// the layer's id once ([`find`](Self::find)), in its update, and its encode
/// takes it again from where it was found ([`at`](Self::at)).
pub(crate) trait TileView<'a>: Copy {
    /// What `contents` show the layer `layer_id`, if anything, and where it
    /// is in them.
    fn find(contents: &'a Contents, layer_id: &str) -> Option<(Self, usize)>;

    /// What `contents` show the layer `layer_id` at `index`, where
    /// [`find`](Self::find) found it in the same contents.
    fn at(contents: &'a Contents, layer_id: &str, index: usize) -> Option<Self>;

    /// The passes the bucket was laid out to draw in, which its layer draws
    /// in too; none for a bucket that isn't laid out.
    fn passes(self) -> RenderPass {
        RenderPass::None
    }
}

impl<'a> TileView<'a> for &'a RasterBucket {
    fn find(contents: &'a Contents, layer_id: &str) -> Option<(Self, usize)> {
        Some((Self::at(contents, layer_id, 0)?, 0))
    }

    fn at(contents: &'a Contents, _layer_id: &str, _index: usize) -> Option<Self> {
        match contents {
            Contents::Raster(bucket) => Some(bucket),
            _ => None,
        }
    }
}

impl<'a> TileView<'a> for &'a HillshadeBucket {
    fn find(contents: &'a Contents, layer_id: &str) -> Option<(Self, usize)> {
        Some((Self::at(contents, layer_id, 0)?, 0))
    }

    fn at(contents: &'a Contents, _layer_id: &str, _index: usize) -> Option<Self> {
        match contents {
            Contents::Dem(bucket) => Some(bucket),
            _ => None,
        }
    }
}

/// What a tile shows a layer of a type without a source: nothing.
#[derive(Clone, Copy)]
pub(crate) enum NoTileBucket {}

impl TileView<'_> for NoTileBucket {
    fn find(_contents: &Contents, _layer_id: &str) -> Option<(Self, usize)> {
        None
    }

    fn at(_contents: &Contents, _layer_id: &str, _index: usize) -> Option<Self> {
        None
    }
}

/// A layer's tile of a frame, as the frame's update found it: its index
/// among its source's render tiles, and where what it shows the layer is in
/// its contents ([`TileView::find`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoundTile {
    pub(crate) tile: usize,
    pub(crate) view: Option<usize>,
}

/// A render tile of a frame, with what it shows a layer of the type `T`.
pub(crate) struct FrameTile<'a, T: LayerType> {
    pub(crate) tile: &'a RenderTile,
    bucket: Option<T::TileBucket<'a>>,
}

impl<T: LayerType> Clone for FrameTile<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType> Copy for FrameTile<'_, T> {}

impl<'a, T: LayerType> FrameTile<'a, T> {
    /// The tile `tile`, which shows `contents`, as the layer `layer_id`
    /// sees it, and where it found what the tile shows the layer.
    #[must_use]
    pub(crate) fn find(tile: &'a RenderTile, contents: Option<&'a Contents>, layer_id: &str) -> (Self, Option<usize>) {
        let found = contents.and_then(|contents| T::TileBucket::find(contents, layer_id));
        (FrameTile { tile, bucket: found.map(|(bucket, _)| bucket) }, found.map(|(_, index)| index))
    }

    /// The tile `tile`, which shows `contents`, as the layer `layer_id`
    /// sees it, with what it shows the layer at `view` in `contents`, where
    /// [`find`](Self::find) found it.
    #[must_use]
    pub(crate) fn at(
        tile: &'a RenderTile,
        contents: Option<&'a Contents>,
        layer_id: &str,
        view: Option<usize>,
    ) -> Self {
        let bucket = contents.zip(view).and_then(|(contents, index)| T::TileBucket::at(contents, layer_id, index));
        debug_assert_eq!(bucket.is_some(), view.is_some(), "a tile's contents changed since the frame's update");
        FrameTile { tile, bucket }
    }

    /// What the tile shows the layer, if anything.
    pub(crate) fn bucket(self) -> Option<T::TileBucket<'a>> {
        self.bucket
    }

    /// [`bucket`](Self::bucket), if it was laid out to draw in `pass`.
    pub(crate) fn bucket_for_pass(self, pass: RenderPass) -> Option<T::TileBucket<'a>> {
        self.bucket.filter(|bucket| bucket.passes() & pass != RenderPass::None)
    }

    /// The tile's id.
    #[must_use]
    pub(crate) fn id(self) -> OverscaledTileID {
        *self.tile.overscaled_tile_id()
    }
}
