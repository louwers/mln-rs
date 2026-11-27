// A render tile is a tile as a frame draws it: where it draws, its
// matrices, and what it copies of the tile and its render entry (its id,
// hold-for-fade state and whether it draws a visible layer) when the
// frame's update makes it. It lives in its render source, apart from the
// tile store. The frame's phases look the tile's contents up through its
// source, in the store by the tile's slot and id (`TileContent`, `FrameTile`);
// the store changes them only in the frame's update, before the layers read
// them.

use glam::{DMat4, Vec2};
use mln_style::constants;
use mln_style::types::TranslateAnchorType;

use crate::camera::{ProjectionData, View};
use crate::draw::identity::SimpleIdentity;
use crate::layer::layer_drawing::MadeFrom;
use crate::layer::render_layer::Layer;
use crate::layer::tile_input::TileData;
use crate::layer::tile_input::input::Find;
use crate::render::paint_parameters::TransformParameters;
use crate::render::pass::RenderPass;
use crate::source::render_image_source::ImageTile;
use crate::tile::{OverscaledTileID, UnwrappedTileID};
use crate::tile_store::contents::Contents;
use crate::tile_store::tile::Tile;

/// A tile of a source that the frame draws: its id, its projection, and
/// what of its contents the layers draw.
pub struct RenderTile {
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
    #[must_use]
    pub fn tile_unit_translation(
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

    /// The id the tile draws as: its world copy of the tile.
    #[must_use]
    pub fn id(&self) -> UnwrappedTileID {
        self.id
    }

    /// The tile's id.
    #[must_use]
    pub fn overscaled_tile_id(&self) -> &OverscaledTileID {
        &self.tile_id
    }

    /// Where the tile is among its source's, as of its last update; `None`
    /// for a tile that isn't among a source's.
    #[must_use]
    pub fn slot(&self) -> Option<usize> {
        self.slot
    }

    /// Whether the tile stays only while a placement fades its marks out.
    #[must_use]
    pub fn hold_for_fade(&self) -> bool {
        self.tile_hold_for_fade
    }

    /// The tile's projection in the frame.
    #[must_use]
    pub fn projection(&self) -> &ProjectionData {
        &self.projection
    }

    /// The tile's main matrix in the frame: its projection's.
    #[must_use]
    pub fn matrix(&self) -> &DMat4 {
        &self.matrix
    }

    /// Sets the tile's projection and its standard matrix for `transform`.
    pub(crate) fn set_projection(&mut self, transform: &TransformParameters) {
        self.projection = transform.view.projection_data_with(&self.id, &transform.proj_matrix);
        self.matrix = self.projection.main_matrix;
    }
}

/// What a frame's tile shows the layers of its source: its contents in the
/// tile store, or an image source's image, which each of the source's
/// tiles draws.
#[derive(Clone, Copy)]
pub enum TileContent<'a> {
    Tile(&'a Contents),
    Image(&'a ImageTile),
}

impl TileContent<'_> {
    /// The bucket or the raster, raster-dem or image contents that the
    /// content shows at `place`, with the generations of their mask or mesh
    /// and of their DEM's border.
    pub(crate) fn stamp(self, place: ViewPlace) -> Option<(SimpleIdentity, [u64; 2])> {
        match (self, place) {
            (TileContent::Tile(contents), place) => contents.stamp(place),
            (TileContent::Image(image), ViewPlace::Own) => Some(image.stamp()),
            (TileContent::Image(_), ViewPlace::Laid(_)) => None,
        }
    }

    /// The box that the content draws over, in its tile's units: min x,
    /// min y, max x, max y. A tile's contents draw over the tile; an image
    /// source's image over the box of its quad's corners, which may reach
    /// past the tile.
    pub(crate) fn bounds(self) -> [i32; 4] {
        match self {
            TileContent::Tile(_) => TILE_BOUNDS,
            TileContent::Image(image) => image.bounds,
        }
    }
}

/// The box of a whole tile, in its units.
pub(crate) const TILE_BOUNDS: [i32; 4] = [0, 0, constants::EXTENT, constants::EXTENT];

/// Whether `bounds`, a box in the units of the tile `tile`, covers part of
/// the tile `other`. A box that only touches its edge doesn't.
pub(crate) fn overlaps(tile: &UnwrappedTileID, bounds: [i32; 4], other: &UnwrappedTileID) -> bool {
    // Both boxes in the tile units of the finer of the two zooms, from the
    // west edge of world copy 0.
    let zoom = tile.canonical.z.max(other.canonical.z);
    let place = |id: &UnwrappedTileID| {
        let scale = 1i64 << (zoom - id.canonical.z);
        let extent = i64::from(constants::EXTENT) * scale;
        let x = ((i64::from(id.wrap) << id.canonical.z) + i64::from(id.canonical.x)) * extent;
        (x, i64::from(id.canonical.y) * extent, scale)
    };
    let (left, top, scale) = place(tile);
    let [min_x, min_y, max_x, max_y] = bounds.map(|bound| i64::from(bound) * scale);
    let (other_left, other_top, other_scale) = place(other);
    let other_extent = i64::from(constants::EXTENT) * other_scale;
    left + min_x < other_left + other_extent
        && other_left < left + max_x
        && top + min_y < other_top + other_extent
        && other_top < top + max_y
}

/// Where a tile's contents keep what they show a layer. The frame's found
/// tiles keep it for layers of every type in one list, so a place of one
/// type's inputs can reach another's
/// [`Find::at`](crate::layer::tile_input::input::Find::at), which asserts
/// that it doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewPlace {
    /// The layer's place among the layers of the tile's layout.
    Laid(usize),
    /// The tile's own contents, a raster or raster-dem tile's or an image
    /// source's image, which every layer of its source draws.
    Own,
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
/// its contents ([`Find::find`](crate::layer::tile_input::input::Find::find)).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoundTile {
    pub(crate) tile: usize,
    pub(crate) view: Option<ViewPlace>,
}

/// A render tile of a frame, with what it holds for a layer of the type
/// `T` and the world copy the layer draws it in.
pub struct FrameTile<'a, T: Layer> {
    pub(crate) tile: &'a RenderTile,
    /// The world copy the layer draws the tile in: its own, but in a drape
    /// one by the drape's.
    wrap: i16,
    data: Option<TileData<'a, T>>,
}

impl<T: Layer> Clone for FrameTile<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Layer> Copy for FrameTile<'_, T> {}

impl<'a, T: Layer> FrameTile<'a, T> {
    /// The tile `tile`, which shows `content`, as the layer `layer_id`
    /// sees it, and where it found what the tile holds for the layer.
    #[must_use]
    pub(crate) fn find(
        tile: &'a RenderTile,
        content: Option<TileContent<'a>>,
        layer_id: &str,
    ) -> (Self, Option<ViewPlace>) {
        let found = content.and_then(|content| <T::Input as Find<T::Style>>::find(content, layer_id));
        let frame_tile = FrameTile { tile, wrap: tile.id.wrap, data: found.map(|(data, _)| data) };
        (frame_tile, found.map(|(_, place)| place))
    }

    /// The tile `tile`, which shows `content`, as the layer `layer_id`
    /// sees it, with what it holds for the layer at `view` in `content`,
    /// where [`find`](Self::find) found it.
    #[must_use]
    pub(crate) fn at(
        tile: &'a RenderTile,
        content: Option<TileContent<'a>>,
        layer_id: &str,
        view: Option<ViewPlace>,
    ) -> Self {
        let data = content.zip(view).and_then(|(content, place)| <T::Input as Find<T::Style>>::at(content, place));
        debug_assert_eq!(data.is_some(), view.is_some(), "a tile's contents changed since the frame's update");
        debug_assert!(
            data.is_none_or(|data| <T::Input as Find<T::Style>>::is_for(data, layer_id)),
            "a layer found at another's place"
        );
        FrameTile { tile, wrap: tile.id.wrap, data }
    }

    /// The tile, drawn in the world copy `wrap`.
    #[must_use]
    pub(crate) fn in_copy(self, wrap: i16) -> Self {
        FrameTile { wrap, ..self }
    }

    /// What the layer's value of the tile is made from, if the tile holds
    /// anything for the layer ([`TileContent::stamp`]).
    pub(crate) fn made_from(self) -> Option<MadeFrom> {
        self.data.map(<T::Input as Find<T::Style>>::made_from)
    }

    /// The tile as the frame draws it.
    #[must_use]
    pub fn render_tile(self) -> &'a RenderTile {
        self.tile
    }

    /// What the tile holds for the layer, if anything.
    pub fn data(self) -> Option<TileData<'a, T>> {
        self.data
    }

    /// [`data`](Self::data), if it was laid out to draw in `pass`.
    pub fn data_for_pass(self, pass: RenderPass) -> Option<TileData<'a, T>> {
        self.data.filter(|&data| <T::Input as Find<T::Style>>::passes(data) & pass != RenderPass::None)
    }

    /// The tile's id.
    #[must_use]
    pub fn id(self) -> OverscaledTileID {
        *self.tile.overscaled_tile_id()
    }

    /// The id that the layer draws the tile under: its own, but in a drape
    /// in the world copy the drape draws it in.
    #[must_use]
    pub fn draw_id(self) -> OverscaledTileID {
        OverscaledTileID { wrap: self.wrap, ..self.id() }
    }
}
