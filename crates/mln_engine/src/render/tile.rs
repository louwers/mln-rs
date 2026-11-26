// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up in the store
// by the tile's slot and id (`FrameTile`); the store changes them only in
// the frame's update, before the layers read them.

use glam::{DMat4, Vec2};
use mln_style::types::TranslateAnchorType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::camera::{ProjectionData, View};
use crate::layer::layer_type::LayerType;
use crate::render::paint_parameters::{PaintParameters, TransformParameters};
use crate::render::pass::RenderPass;
use crate::tile_store::contents::Contents;
use crate::tile_store::dem::DemContents;
use crate::tile_store::raster::RasterContents;
use crate::tile_store::tile::Tile;

pub(crate) struct RenderTile {
    pub(crate) id: UnwrappedTileID,
    /// The projection contract for this tile; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: DMat4,

    /// `tile.id`.
    tile_id: OverscaledTileID,
    tile_hold_for_fade: bool,
    /// Where the tile is among its source's, which finds its contents
    /// first; `None` for a tile that isn't among a source's.
    slot: Option<usize>,
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
        }
    }

    /// The layer's translation in tile units, rotated for its anchor.
    pub(crate) fn tile_unit_translation(
        id: &UnwrappedTileID,
        translation: [f32; 2],
        anchor: TranslateAnchorType,
        state: &View,
    ) -> [f32; 2] {
        if translation[0] == 0.0 && translation[1] == 0.0 {
            return [0.0, 0.0];
        }
        let angle = if anchor == TranslateAnchorType::Viewport { (-state.angle()) as f32 } else { 0.0f32 };
        let translate = Vec2::from_angle(angle).rotate(Vec2::new(translation[0], translation[1]));
        let zoom = state.zoom() as f32;
        [id.pixels_to_tile_units(translate.x, zoom), id.pixels_to_tile_units(translate.y, zoom)]
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

    /// Sets the tile's projection and its standard matrix for `transform`.
    pub(crate) fn set_projection(&mut self, transform: &TransformParameters) {
        self.projection = transform.view.projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}

/// Where a tile's contents keep what they show a layer. The frame's found
/// tiles keep it for layers of every type in one list, so a place of one
/// type's views can reach another's [`TileView::at`], which asserts that it
/// doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewPlace {
    /// The layer's place among the layers of the tile's layout.
    Laid(usize),
    /// The tile's own contents, a raster or raster-dem tile's, which every
    /// layer of its source draws.
    Own,
}

/// What a frame's tile shows a layer of some type: its bucket in the tile's
/// layout, or a raster or raster-dem tile's own contents. A frame finds it by
/// the layer's id once ([`find`](Self::find)), in its update, and its encode
/// takes it again from where it was found ([`at`](Self::at)).
pub(crate) trait TileView<'a>: Copy {
    /// What `contents` show the layer `layer_id`, if anything, and where it
    /// is in them.
    fn find(contents: &'a Contents, layer_id: &str) -> Option<(Self, ViewPlace)>;

    /// What `contents` show at `place`, where [`find`](Self::find) found
    /// what they show a layer in the same contents.
    fn at(contents: &'a Contents, place: ViewPlace) -> Option<Self>;

    /// Whether this is what a tile shows the layer `layer_id`, rather than
    /// another layer.
    fn is_for(self, _layer_id: &str) -> bool {
        true
    }

    /// The passes the bucket was laid out to draw in, which its layer draws
    /// in too; none for a bucket that isn't laid out.
    fn passes(self) -> RenderPass {
        RenderPass::None
    }
}

impl<'a> TileView<'a> for &'a RasterContents {
    fn find(contents: &'a Contents, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        Some((Self::at(contents, ViewPlace::Own)?, ViewPlace::Own))
    }

    fn at(contents: &'a Contents, place: ViewPlace) -> Option<Self> {
        debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own contents");
        match contents {
            Contents::Raster(raster) => Some(raster),
            _ => None,
        }
    }
}

impl<'a> TileView<'a> for &'a DemContents {
    fn find(contents: &'a Contents, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        Some((Self::at(contents, ViewPlace::Own)?, ViewPlace::Own))
    }

    fn at(contents: &'a Contents, place: ViewPlace) -> Option<Self> {
        debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own contents");
        match contents {
            Contents::Dem(dem) => Some(dem),
            _ => None,
        }
    }
}

/// What a tile shows a layer of a type without a source: nothing.
#[derive(Clone, Copy)]
pub(crate) enum NoTileBucket {}

impl TileView<'_> for NoTileBucket {
    fn find(_contents: &Contents, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        None
    }

    fn at(_contents: &Contents, _place: ViewPlace) -> Option<Self> {
        None
    }
}

/// What a layer draws of a frame's tile: the tile, the bucket or contents
/// it shows the layer, and the generations of their mask or mesh and of
/// their DEM's border. It changes whenever the layer's draws of the tile
/// would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileStamp {
    pub(crate) tile: OverscaledTileID,
    pub(crate) bucket: Option<SimpleIdentity>,
    pub(crate) generations: [u64; 2],
}

/// A layer's tile of a frame, as the frame's update found it: its index
/// among its source's render tiles, and where what it shows the layer is in
/// its contents ([`TileView::find`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoundTile {
    pub(crate) tile: usize,
    pub(crate) view: Option<ViewPlace>,
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
    pub(crate) fn find(
        tile: &'a RenderTile,
        contents: Option<&'a Contents>,
        layer_id: &str,
    ) -> (Self, Option<ViewPlace>) {
        let found = contents.and_then(|contents| T::TileBucket::find(contents, layer_id));
        (FrameTile { tile, bucket: found.map(|(bucket, _)| bucket) }, found.map(|(_, place)| place))
    }

    /// The tile `tile`, which shows `contents`, as the layer `layer_id`
    /// sees it, with what it shows the layer at `view` in `contents`, where
    /// [`find`](Self::find) found it.
    #[must_use]
    pub(crate) fn at(
        tile: &'a RenderTile,
        contents: Option<&'a Contents>,
        layer_id: &str,
        view: Option<ViewPlace>,
    ) -> Self {
        let bucket = contents.zip(view).and_then(|(contents, place)| T::TileBucket::at(contents, place));
        debug_assert_eq!(bucket.is_some(), view.is_some(), "a tile's contents changed since the frame's update");
        debug_assert!(bucket.is_none_or(|bucket| bucket.is_for(layer_id)), "a layer found at another's place");
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

    /// The id that a layer that drapes draws the tile under with
    /// `parameters`: its own, but in a drape in the drape's world copy.
    #[must_use]
    pub(crate) fn draw_id(self, parameters: &PaintParameters<'_>) -> OverscaledTileID {
        let id = self.id();
        match parameters.drape_wrap() {
            Some(wrap) => OverscaledTileID { wrap, ..id },
            None => id,
        }
    }
}
