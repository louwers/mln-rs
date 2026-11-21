// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up in the store
// by the tile's slot and id (`FrameTile`); the store changes them only in
// the frame's update, before the layers read them.

use glam::DMat4;
use mln_map_types::{MapDebugOptions, ProjectionData};
use mln_style::layer_type::SourceKind;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::buckets::debug_bucket::DebugBucket;
use crate::layer::LayerType;
use crate::paint_parameters::TransformParameters;
use crate::render_source::RenderTileParameters;
use crate::tile::contents::Contents;
use crate::tile::dem::DemContents;
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

/// What a frame's tile holds, which a layer's [`TileView`] reads.
#[derive(Clone, Copy)]
pub(crate) enum TileContent<'a> {
    /// The tile's contents in the tile store.
    Tile(&'a Contents),
    /// The image of an image source, which each of the source's tiles
    /// shows.
    Image(&'a RasterContents),
}

/// Where a tile's content keeps what it shows a layer. The frame's found
/// tiles keep it for layers of every type in one list, so a place of one
/// type's views can reach another's [`FindView::at`], which asserts that it
/// doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewPlace {
    /// The layer's place among the layers of the tile's layout.
    Laid(usize),
    /// The tile's own data, a raster or raster-dem tile's contents or an
    /// image source's image, which every layer of its source draws.
    Own,
}

/// What a frame's tile shows a layer of some type: its bucket in the tile's
/// layout ([`LayerBucket`](crate::layer::LayerBucket)), a raster tile's
/// image or an image source's image ([`RasterView`]), a raster-dem tile's
/// DEM (`&`[`DemContents`]), or nothing for a type without a source
/// ([`NoTileView`]).
/// The engine's kinds of tiles are the only ones, so these are the only
/// views: a layer type takes the one of its source's kind.
#[expect(private_bounds, reason = "the engine's views are the only ones: `FindView` seals the trait")]
pub trait TileView<'a>: Copy + FindView<'a> {}

impl<'a, V: Copy + FindView<'a>> TileView<'a> for V {}

/// How the engine finds a layer's [`TileView`] in a frame's tile. A frame
/// finds it by the layer's id once ([`find`](Self::find)), in its update,
/// and its encode takes it again from where it was found ([`at`](Self::at)).
pub(crate) trait FindView<'a>: Sized {
    /// The kind of source whose tiles show the view.
    const SOURCE: SourceKind;

    /// What `content` shows the layer `layer_id`, if anything, and where it
    /// is in it.
    fn find(content: TileContent<'a>, layer_id: &str) -> Option<(Self, ViewPlace)>;

    /// What `content` shows at `place`, where [`find`](Self::find) found
    /// what it shows a layer in the same content.
    fn at(content: TileContent<'a>, place: ViewPlace) -> Option<Self>;

    /// Whether this is what a tile shows the layer `layer_id`, rather than
    /// another layer.
    fn is_for(&self, _layer_id: &str) -> bool {
        true
    }

    /// Whether the view is a bucket laid out with values that draw, which
    /// its layer then draws too; false for a view that isn't a bucket.
    fn draws(self) -> bool {
        false
    }

    /// The index of the bucket among its layout's buckets; `None` for a
    /// view that isn't a bucket.
    fn bucket_index(self) -> Option<usize> {
        None
    }

    /// The identity of the bucket or contents the view shows the layer to
    /// draw; `None` where it shows nothing to draw: a bucket laid out with
    /// values that draw nothing, or a bucket or contents without data.
    fn data(self) -> Option<SimpleIdentity>;
}

/// What a frame's tile shows a layer of a raster source or an image source:
/// a raster tile's image, or the image source's image, which each of its
/// tiles shows.
#[derive(Clone, Copy)]
pub enum RasterView<'a> {
    /// A raster tile's image.
    Tile(&'a RasterContents),
    /// An image source's image, drawn in the tile.
    Image(&'a RasterContents),
}

impl<'a> RasterView<'a> {
    /// The image and the geometry that draws it.
    #[must_use]
    pub fn contents(self) -> &'a RasterContents {
        match self {
            RasterView::Tile(contents) | RasterView::Image(contents) => contents,
        }
    }
}

impl<'a> FindView<'a> for RasterView<'a> {
    const SOURCE: SourceKind = SourceKind::Raster;

    fn find(content: TileContent<'a>, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        Some((Self::at(content, ViewPlace::Own)?, ViewPlace::Own))
    }

    fn at(content: TileContent<'a>, place: ViewPlace) -> Option<Self> {
        debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own data");
        match content {
            TileContent::Tile(Contents::Raster(contents)) => Some(RasterView::Tile(contents)),
            TileContent::Image(contents) => Some(RasterView::Image(contents)),
            TileContent::Tile(_) => None,
        }
    }

    fn data(self) -> Option<SimpleIdentity> {
        let contents = self.contents();
        contents.has_data().then(|| contents.id())
    }
}

impl<'a> FindView<'a> for &'a DemContents {
    const SOURCE: SourceKind = SourceKind::RasterDem;

    fn find(content: TileContent<'a>, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        Some((Self::at(content, ViewPlace::Own)?, ViewPlace::Own))
    }

    fn at(content: TileContent<'a>, place: ViewPlace) -> Option<Self> {
        debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own data");
        match content {
            TileContent::Tile(Contents::Dem(contents)) => Some(contents),
            _ => None,
        }
    }

    fn data(self) -> Option<SimpleIdentity> {
        self.has_data().then(|| self.id())
    }
}

/// What a tile shows a layer of a type without a source: nothing.
#[derive(Clone, Copy)]
pub enum NoTileView {}

impl FindView<'_> for NoTileView {
    const SOURCE: SourceKind = SourceKind::None;

    fn find(_content: TileContent<'_>, _layer_id: &str) -> Option<(Self, ViewPlace)> {
        None
    }

    fn at(_content: TileContent<'_>, _place: ViewPlace) -> Option<Self> {
        None
    }

    fn data(self) -> Option<SimpleIdentity> {
        match self {}
    }
}

/// A layer's tile of a frame, as the frame's update found it: its index
/// among its source's render tiles, and where what it shows the layer is in
/// its contents ([`FindView::find`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoundTile {
    pub(crate) tile: usize,
    pub(crate) view: Option<ViewPlace>,
}

/// A render tile of a frame, with what it shows a layer of the type `T`.
pub struct FrameTile<'a, T: LayerType> {
    pub(crate) tile: &'a RenderTile,
    view: Option<T::View<'a>>,
}

impl<T: LayerType> Clone for FrameTile<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType> Copy for FrameTile<'_, T> {}

impl<'a, T: LayerType> FrameTile<'a, T> {
    /// The tile `tile`, which holds `content`, as the layer `layer_id`
    /// sees it, and where it found what the tile shows the layer.
    #[must_use]
    pub(crate) fn find(
        tile: &'a RenderTile,
        content: Option<TileContent<'a>>,
        layer_id: &str,
    ) -> (Self, Option<ViewPlace>) {
        let found = content.and_then(|content| T::View::find(content, layer_id));
        (FrameTile { tile, view: found.map(|(view, _)| view) }, found.map(|(_, place)| place))
    }

    /// The tile `tile`, which holds `content`, as the layer `layer_id`
    /// sees it, with what it shows the layer at `place` in `content`, where
    /// [`find`](Self::find) found it.
    #[must_use]
    pub(crate) fn at(
        tile: &'a RenderTile,
        content: Option<TileContent<'a>>,
        layer_id: &str,
        place: Option<ViewPlace>,
    ) -> Self {
        let view = content.zip(place).and_then(|(content, place)| T::View::at(content, place));
        debug_assert_eq!(view.is_some(), place.is_some(), "a tile's contents changed since the frame's update");
        debug_assert!(view.is_none_or(|view| view.is_for(layer_id)), "a layer found at another's place");
        FrameTile { tile, view }
    }

    /// What the tile shows the layer, if anything.
    #[must_use]
    pub fn view(self) -> Option<T::View<'a>> {
        self.view
    }

    /// [`view`](Self::view), if it is a bucket laid out with values that
    /// draw.
    #[must_use]
    pub fn view_to_draw(self) -> Option<T::View<'a>> {
        self.view.filter(|view| view.draws())
    }

    /// The tile's id.
    #[must_use]
    pub fn id(self) -> OverscaledTileID {
        *self.tile.overscaled_tile_id()
    }

    /// The id the frame draws the tile at: its tile in its world copy.
    #[must_use]
    pub fn unwrapped_id(self) -> UnwrappedTileID {
        self.tile.id
    }

    /// How the frame projects the tile; [`matrix`](Self::matrix) is its
    /// main matrix.
    #[must_use]
    pub fn projection(self) -> ProjectionData {
        self.tile.projection
    }

    /// The tile's main matrix for the frame.
    #[must_use]
    pub fn matrix(self) -> DMat4 {
        self.tile.matrix
    }

    /// Whether the frame keeps the tile, which it no longer needs, while
    /// what placement showed of it fades out.
    #[must_use]
    pub fn held_for_fade(self) -> bool {
        self.tile.hold_for_fade()
    }
}
