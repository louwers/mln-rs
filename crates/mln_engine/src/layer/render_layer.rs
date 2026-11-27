// A render layer is the renderer's side of a style layer: its type's layer
// (`TypedLayer`), whose hooks see the frame's tiles and what they hold for
// the type (`TileInput`), and what the engine keeps of it around them
// (`Hosted`): its base, its values of its type, its value of each tile it
// draws and its draws. `RenderLayer` is what the engine does with a render
// layer of any type. The orchestrator owns the render layers, in the
// style's order.

use std::ops::Range;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_style::color::Color;
use mln_style::expression::Dependency;
use mln_style::layer::{ErasedLayerStyle, LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style::types::{LayerBlendType, VisibilityType};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use smallvec::SmallVec;

use crate::camera::View;
use crate::draw::context::Context;
use crate::draw::draws::{DrawOrder, Draws, Group, LayerFrame, LendAt, Lent};
use crate::draw::resources::Resources;
use crate::layer::bucket::ErasedBucket;
use crate::layer::layer_drawing::{ChangedBuckets, TileMap};
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::layer::tile_input::TileData;
use crate::layer::tile_input::input::Find;
use crate::layers::custom::EndedHosts;
use crate::query::feature_index::QueryRegion;
use crate::render::paint_parameters::PaintParameters;
use crate::render::pass::RenderPass;
use crate::render::terrain::SurfaceLight;
use crate::render::tile::{FoundTile, FrameTile, RenderTile, TILE_BOUNDS, TileContent, TileStamp, overlaps};
use crate::render::update_parameters::UpdateParameters;
use crate::source::render_source::{RenderSource, TileOrder};
use crate::tile::{OverscaledTileID, UnwrappedTileID};

/// A layer's bucket in a layout, which the layers of its layout group
/// share, with the layer's values it was laid out with, whichever the
/// layer's type.
#[derive(Clone, Copy)]
pub(crate) struct ErasedLayerBucket<'a> {
    pub(crate) bucket: &'a dyn ErasedBucket,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a dyn ErasedEvaluatedLayer,
}

/// A source's render tiles in a frame, with what each of them shows, and
/// the tiles that the frame's updates of the layers found
/// ([`LayerUpdateParameters::found`]), as the engine sees them.
#[derive(Clone, Copy)]
pub(crate) struct FrameSourceTiles<'a> {
    tiles: &'a [RenderTile],
    /// What each of `tiles` shows, at its index.
    contents: &'a [Option<TileContent<'a>>],
    found: &'a [FoundTile],
}

/// The tiles of a layer of the type `T` in a frame, each with what it shows
/// the layer, which keeps up to 64 of them on the stack.
pub(crate) type FrameTiles<'a, T> = SmallVec<[FrameTile<'a, T>; 64]>;

impl<'a> FrameSourceTiles<'a> {
    /// The render tiles `tiles`, which show `contents`, at their indexes,
    /// and the tiles of them that the frame's layers found.
    #[must_use]
    pub(crate) fn new(
        tiles: &'a [RenderTile],
        contents: &'a [Option<TileContent<'a>>],
        found: &'a [FoundTile],
    ) -> Self {
        FrameSourceTiles { tiles, contents, found }
    }

    /// What the tile at `index` shows.
    fn content(self, index: usize) -> Option<TileContent<'a>> {
        self.contents.get(index).copied().flatten()
    }

    /// The tiles at the indexes of `order`, in that order, as the layer
    /// `layer_id` of the type `T` sees them, each with what it shows the
    /// layer, found by the layer's id. `found` keeps where.
    fn find<T: LayerType>(self, order: &[usize], layer_id: &str, found: &mut Vec<FoundTile>) -> FrameTiles<'a, T> {
        let mut tiles = FrameTiles::new();
        for &index in order {
            let Some(tile) = self.tiles.get(index) else { continue };
            let (frame_tile, view) = FrameTile::find(tile, self.content(index), layer_id);
            tiles.push(frame_tile);
            found.push(FoundTile { tile: index, view });
        }
        tiles
    }

    /// The found tiles at `found`, a range of them.
    pub(crate) fn found(self, found: Range<usize>) -> &'a [FoundTile] {
        self.found.get(found).unwrap_or_default()
    }

    /// The tiles of `found`, as [`find`](Self::find) found them for the
    /// layer `layer_id` of the type `T` in the same frame.
    pub(crate) fn at<'f, T: LayerType>(
        self,
        found: impl IntoIterator<Item = &'f FoundTile>,
        layer_id: &str,
    ) -> FrameTiles<'a, T> {
        found
            .into_iter()
            .filter_map(|&FoundTile { tile, view }| {
                let render_tile = self.tiles.get(tile);
                debug_assert!(render_tile.is_some(), "a tile found past the frame's render tiles");
                Some(FrameTile::at(render_tile?, self.content(tile), layer_id, view))
            })
            .collect()
    }

    /// The tiles of the found tiles at `found`, as [`at`](Self::at) gives
    /// them, that overlap `tile` (what they show covers part of it), each in
    /// the world copy that does: on Mercator their own, so those of `tile`'s
    /// copy; on the `globe`, which has no copies, whatever their copy, one by
    /// `tile`'s. A drape of `tile` draws each in that copy
    /// ([`FrameTile::draw_id`]).
    pub(crate) fn overlapping<T: LayerType>(
        self,
        found: Range<usize>,
        layer_id: &str,
        tile: UnwrappedTileID,
        globe: bool,
    ) -> FrameTiles<'a, T> {
        let found = self.found(found);
        self.overlaps(found, tile, globe)
            .map(|(&FoundTile { tile: index, view }, render_tile, wrap)| {
                FrameTile::at(render_tile, self.content(index), layer_id, view).in_copy(wrap)
            })
            .collect()
    }

    /// What a layer draws of the tiles that
    /// [`overlapping`](Self::overlapping) gives, in its order, under the ids
    /// it draws them in.
    pub(crate) fn stamps(
        self,
        found: Range<usize>,
        tile: UnwrappedTileID,
        globe: bool,
    ) -> impl Iterator<Item = TileStamp> {
        let found = self.found(found);
        self.overlaps(found, tile, globe).map(move |(&FoundTile { tile: index, view }, render_tile, wrap)| {
            let stamp = self.content(index).zip(view).and_then(|(content, place)| content.stamp(place));
            TileStamp {
                tile: OverscaledTileID { wrap, ..*render_tile.overscaled_tile_id() },
                bucket: stamp.map(|(bucket, _)| bucket),
                generations: stamp.map_or([0, 0], |(_, generations)| generations),
            }
        })
    }

    /// The tiles of `found` that overlap `tile`, with their render tiles and
    /// the world copies they overlap it in: on Mercator their own; on the
    /// `globe`, any of `tile`'s and the copies either side, which an image
    /// source's image that crosses the antimeridian reaches into.
    fn overlaps<'f>(
        self,
        found: &'f [FoundTile],
        tile: UnwrappedTileID,
        globe: bool,
    ) -> impl Iterator<Item = (&'f FoundTile, &'a RenderTile, i16)> {
        let found = found.iter().filter_map(move |found| Some((found, self.tiles.get(found.tile)?)));
        found.flat_map(move |(found, render_tile)| {
            let bounds = self.content(found.tile).map_or(TILE_BOUNDS, TileContent::bounds);
            let canonical = render_tile.id.canonical;
            let wraps = if globe { tile.wrap - 1..=tile.wrap + 1 } else { render_tile.id.wrap..=render_tile.id.wrap };
            wraps
                .filter(move |&wrap| overlaps(&UnwrappedTileID::with_wrap(wrap, canonical), bounds, &tile))
                .map(move |wrap| (found, render_tile, wrap))
        })
    }
}

/// What a layer reads in the frame's update, besides its tiles.
pub(crate) struct LayerUpdateParameters<'a> {
    /// The frame's camera.
    pub(crate) view: &'a View,
    /// The update the frame draws.
    pub(crate) update_parameters: &'a UpdateParameters,
    /// The layer's source, `None` for a layer without one or a source type
    /// the renderer has no render source for.
    pub(crate) source: Option<&'a dyn RenderSource>,
    /// What each of the source's render tiles shows, at its index.
    pub(crate) contents: &'a [Option<TileContent<'a>>],
    /// The images of the style.
    pub(crate) images: &'a crate::image::image_store::ImageStore,
    /// The tiles of the frame's layers, as their updates find them, which
    /// the update adds the layer's to: those the encode draws.
    pub(crate) found: &'a mut Vec<FoundTile>,
}

impl<'a> LayerUpdateParameters<'a> {
    /// The source's tiles, and their indexes in `order`; `None` for a layer
    /// without a source or a source without tiles.
    #[must_use]
    fn tiles(&self, order: TileOrder) -> Option<(FrameSourceTiles<'a>, &'a [usize])> {
        let source = self.source?;
        Some((FrameSourceTiles::new(source.render_tiles(), self.contents, &[]), source.tile_order(order)?))
    }
}

/// What every render layer has, whatever its type.
pub(crate) struct RenderLayerBase {
    pub(crate) evaluated_properties: Arc<dyn ErasedEvaluatedLayer>,
    pub(crate) style: Arc<dyn ErasedLayerStyle>,

    pub(crate) passes: RenderPass,
    /// What the layer's last evaluation said of how it draws.
    pub(crate) evaluation: Evaluation,
    pub(crate) style_dependencies: Dependency,
    /// Whether the frame draws the layer.
    pub(crate) is_renderable: bool,

    /// The projection variant this layer's draws are built with.
    pub(crate) projection_variant: ProjectionVariant,

    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdateParameters::found`]).
    found: Range<usize>,
}

impl RenderLayerBase {
    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdateParameters::found`]).
    pub(crate) fn found(&self) -> Range<usize> {
        self.found.clone()
    }

    pub(crate) fn new(properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        let style = properties.shared_style();
        RenderLayerBase {
            evaluated_properties: properties,
            style,
            passes: RenderPass::None,
            evaluation: Evaluation::NONE,
            style_dependencies: Dependency::NONE,
            is_renderable: false,
            projection_variant: ProjectionVariant::Mercator,
            found: 0..0,
        }
    }
}

/// A layer's values of its type `T`: its style, its paint values in
/// transition, and their evaluation for the last frame.
pub(crate) struct LayerValues<T: LayerType> {
    pub(crate) style: Arc<LayerStyle<T>>,
    pub(crate) unevaluated: <T::PaintProperties as PaintSet>::Unevaluated,
    pub(crate) evaluated: Arc<EvaluatedLayer<T>>,
}

impl<T: LayerType> LayerValues<T> {
    /// The values of the layer `style`, before any transition, and the base
    /// of a render layer that holds them.
    pub(crate) fn new(style: Arc<LayerStyle<T>>) -> (RenderLayerBase, Self) {
        let unevaluated = style.paint.untransitioned();
        let evaluated = Arc::new(EvaluatedLayer::new(Arc::clone(&style)));
        let mut base = RenderLayerBase::new(Arc::<EvaluatedLayer<T>>::clone(&evaluated));
        base.style_dependencies = T::PaintProperties::dependencies(&unevaluated);
        (base, LayerValues { style, unevaluated, evaluated })
    }

    /// The paint values evaluated for a frame, copying from the last
    /// evaluation each value that can't have changed, with the frame's
    /// cross-fade for a type that has one, and no render passes yet.
    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) -> EvaluatedLayer<T> {
        let paint =
            T::PaintProperties::evaluate_with_previous(&mut self.unevaluated, parameters, &self.evaluated.paint);
        let mut evaluated = EvaluatedLayer::with_paint(Arc::clone(&self.style), paint);
        if T::CROSSFADES {
            evaluated.crossfade = parameters.get_crossfade_parameters();
        }
        evaluated
    }

    /// Makes `evaluated` the layer's values, drawing in `base`'s passes.
    pub(crate) fn set_evaluated(&mut self, base: &mut RenderLayerBase, mut evaluated: EvaluatedLayer<T>) {
        evaluated.render_passes = base.passes.underlying_type();
        self.evaluated = Arc::new(evaluated);
        base.evaluated_properties = Arc::<EvaluatedLayer<T>>::clone(&self.evaluated);
    }
}

/// What a layer's evaluation says of how it draws with its values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Evaluation {
    /// Whether the layer draws anything.
    pub(crate) draws: bool,
    /// Whether the layer may draw in the opaque pass, which the frame draws
    /// front to back before it blends the rest.
    pub(crate) opaque: bool,
    /// The one solid color the layer is, which the frame clears to instead
    /// of drawing the layer when it is the style's bottom layer.
    pub(crate) solid: Option<Color>,
    /// Whether the layer draws a drape by the map's bearing: something of
    /// it is anchored to the viewport.
    pub(crate) drapes_by_bearing: bool,
}

impl Evaluation {
    /// The layer draws nothing.
    pub(crate) const NONE: Evaluation =
        Evaluation { draws: false, opaque: false, solid: None, drapes_by_bearing: false };
    /// The layer draws, translucent.
    pub(crate) const TRANSLUCENT: Evaluation = Evaluation { draws: true, ..Evaluation::NONE };
    /// The layer draws, and may draw in the opaque pass.
    pub(crate) const OPAQUE: Evaluation = Evaluation { opaque: true, ..Evaluation::TRANSLUCENT };

    /// The passes a layer of this evaluation draws in.
    fn passes(self) -> RenderPass {
        match self {
            Evaluation { draws: false, .. } => RenderPass::None,
            Evaluation { opaque: true, .. } => RenderPass::Opaque | RenderPass::Translucent,
            Evaluation { opaque: false, .. } => RenderPass::Translucent,
        }
    }
}

/// What a layer's [`TypedLayer::evaluate`] reads and completes.
pub(crate) struct EvaluateCx<'a, L: TypedLayer> {
    /// The layer's values before the evaluation: its style, its paint values
    /// in transition and their last evaluation.
    pub(crate) values: &'a LayerValues<L::Type>,
    /// What the values were evaluated for.
    pub(crate) parameters: &'a PropertyEvaluationParameters,
    /// The new evaluation of the values, which becomes the layer's once the
    /// hook returns.
    pub(crate) evaluated: &'a mut EvaluatedLayer<L::Type>,
}

/// How a layer type meets terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnTerrain {
    /// The layer draws flat, at sea level.
    Flat,
    /// The layer drapes over terrain: it draws flat into the terrain tiles'
    /// textures ([`RenderLayer::update_drape_draws`]) rather than onto the
    /// frame.
    Drapes,
    /// The layer stands on terrain: over terrain, its draws are of the
    /// terrain variant ([`ProjectionVariant::stands`]), and their points
    /// stand on the frame's ground ([`LayerFrame::ground`]). One that
    /// `hides_behind_ground` hides its points behind the ground in front of
    /// them, which reads the ground's depth ([`FrameGround::depth`]).
    ///
    /// [`FrameGround::depth`]: crate::render::terrain::FrameGround::depth
    Stands { hides_behind_ground: bool },
}

impl OnTerrain {
    /// Whether the layer drapes over terrain.
    pub(crate) fn drapes(self) -> bool {
        self == OnTerrain::Drapes
    }

    /// Whether the layer stands on terrain.
    pub(crate) fn stands(self) -> bool {
        matches!(self, OnTerrain::Stands { .. })
    }

    /// Whether the layer hides behind the ground in front of it.
    pub(crate) fn hides_behind_ground(self) -> bool {
        self == OnTerrain::Stands { hides_behind_ground: true }
    }
}

/// What a layer's [`TypedLayer::prepare`] reads and changes: its base and
/// values, its values of the tiles it draws ([`TileMap`]), which the frame
/// and its drapes share, by the tiles' own ids ([`FrameTile::id`]), and the
/// frame it makes its draws for, the map's or a drape's.
pub(crate) struct PrepareCx<'a, 'f, L: TypedLayer> {
    pub(crate) base: &'a RenderLayerBase,
    pub(crate) values: &'a LayerValues<L::Type>,
    pub(crate) tiles: &'a mut TileMap<L::Tile>,
    /// For a layer that picks its tiles ([`TypedLayer::PICKS_TILES`]),
    /// those the frame draws, in id order: those it picked, or a drape's
    /// tile. Empty for another layer.
    pub(crate) picked: &'a [OverscaledTileID],
    /// What the layer makes its meshes and targets with.
    pub(crate) res: Resources<'a>,
    pub(crate) frame: &'a LayerFrame<'f>,
}

/// What a layer's [`TypedLayer::tile`] reads to make a tile's value.
pub(crate) struct TileCx<'a, 'f, L: TypedLayer> {
    pub(crate) base: &'a RenderLayerBase,
    pub(crate) values: &'a LayerValues<L::Type>,
    /// What the layer makes its meshes and targets with.
    pub(crate) res: Resources<'a>,
    /// The frame the value is made in: the map's, or for a layer that
    /// drapes, the drape's that first draws it, whose value every drape
    /// the tile overlaps draws.
    pub(crate) frame: &'a LayerFrame<'f>,
}

/// What a layer's [`TypedLayer::update`] reads, besides its tiles.
pub(crate) struct UpdateCx<'a, 'p, L: TypedLayer> {
    pub(crate) values: &'a LayerValues<L::Type>,
    pub(crate) parameters: &'a mut LayerUpdateParameters<'p>,
    picked: &'a mut Vec<OverscaledTileID>,
}

impl<L: TypedLayer> UpdateCx<'_, '_, L> {
    /// Makes `tiles` those that a layer that picks its tiles
    /// ([`TypedLayer::PICKS_TILES`]) draws in the map's frame.
    pub(crate) fn pick_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        debug_assert!(L::PICKS_TILES, "a layer that doesn't pick its tiles picked some");
        self.picked.clear();
        self.picked.extend(tiles);
        self.picked.sort();
        self.picked.dedup();
    }
}

/// A drape a layer draws in: its terrain tile, whether the frame is on the
/// globe, and the tiles of every drape of the frame, in id order, which a
/// layer that picks its tiles keeps values of.
#[derive(Clone, Copy)]
pub(crate) struct DrapeTile<'a> {
    pub(crate) tile: OverscaledTileID,
    pub(crate) globe: bool,
    pub(crate) tiles: &'a [OverscaledTileID],
}

/// The draws a layer makes in a frame: the map's, or a drape's, into the
/// drape's draws of the layer.
enum DrawsFor<'d> {
    Map,
    Drape(&'d mut Draws, &'d OverscaledTileID),
}

/// A layer type's render half: what it makes of its values, its tiles'
/// data and the frame, whose hooks the engine calls on a render layer of the
/// type ([`Hosted`]). The engine keeps the layer's base, its values of the
/// layer type `Type`, its value of each tile it draws, a `Tile`, which the
/// map's frame and every drape the tile overlaps draw, and its draws
/// ([`Draws`]) in the frame and in each drape it draws in.
pub(crate) trait TypedLayer: Sized + 'static {
    type Type: LayerType;
    /// What the engine keeps of each tile the layer draws for it, from when
    /// the layer makes it ([`tile`](Self::tile)) until what the tile shows
    /// the layer changes.
    type Tile: 'static;

    /// The order the layer draws its source's tiles in.
    const TILE_ORDER: TileOrder = TileOrder::ById;

    /// How the layer meets terrain.
    const TERRAIN: OnTerrain = OnTerrain::Flat;

    /// Whether the layer draws 3D geometry, whose depth it writes in the
    /// range of [`PaintParameters::depth_mode_for_3d`]. The layers above the
    /// lowest such layer that draws draw without a depth test, and on the
    /// globe the planet's depth hides its geometry behind the horizon.
    const VOLUME: bool = false;

    /// Whether a layer of a type without a source draws tiles it picks in
    /// its update ([`UpdateCx::pick_tiles`]), and in a drape the drape's
    /// tile ([`PrepareCx::picked`]); one that picks none draws nothing. One
    /// that doesn't pick draws without tiles.
    const PICKS_TILES: bool = false;

    /// Whether the layer's tile values hold nothing of its style's values,
    /// so that they stay through a restyle.
    const RESTYLE_KEEPS_TILES: bool = false;

    /// The layer of the values `values`, before they transition.
    fn new(values: &LayerValues<Self::Type>) -> Self;

    /// Called once the layer's paint values are evaluated for a frame,
    /// before they become the layer's: the layer derives the values of
    /// `cx.evaluated` that its type derives from others, remakes what it
    /// makes of them, such as a color ramp, and says how it draws with them.
    fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    /// The light that the terrain's surface can shade the layer's drapes
    /// with in `frame`, whose draws then put in them what it shades
    /// ([`LayerFrame::drape_lit`]); `None` for a layer whose drapes hold its
    /// colors.
    fn surface_light(&mut self, _values: &LayerValues<Self::Type>, _frame: &LayerFrame<'_>) -> Option<SurfaceLight> {
        None
    }

    /// Called in the frame's update of the layer with `tiles`, the frame's
    /// tiles of its source in the order the layer draws them, each with
    /// what it holds for the layer; none for a type without a source
    /// ([`NoTiles`](crate::layer::tile_input::NoTiles)). The layer collects
    /// here what the update needs of its tiles' data; a type that places
    /// collects what it places in its participant instead
    /// ([`Placement::collect`](crate::placement::Placement::collect)). One
    /// without a source that picks its tiles picks them here.
    fn update(&mut self, _cx: UpdateCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self::Type>]) {}

    /// Whether `tile`, a tile's value the layer made, still draws the tile
    /// with the layer's `values` of the frame. The engine retires a value
    /// the layer doesn't keep before the layer makes the frame's draws.
    fn keeps(&self, _values: &LayerValues<Self::Type>, _tile: &Self::Tile) -> bool {
        true
    }

    /// The value of `tile`, which holds `data` for the layer, for a tile
    /// the engine keeps no value of: none where the tile shows nothing to
    /// draw. The engine keeps it until the tile shows the layer something
    /// else (another bucket, or contents of another generation), leaves the
    /// frame, or the layer's draws drop. The map's frame and every drape
    /// the tile overlaps, in any world copy, draw the same value, so it
    /// holds nothing of the frame or the copy it is first drawn in.
    ///
    /// `previous` is the tile's value before, if the engine retired it
    /// rather than dropped it: when the tile came to show other contents or
    /// another generation of them, the layer stopped keeping it
    /// ([`keeps`](Self::keeps)), or its draws dropped for a restyle or the
    /// frame's projection. A layer takes from it what it made of contents
    /// that are still the tile's, such as a texture of the tile's image,
    /// rather than making them again.
    fn tile(
        &mut self,
        _cx: &mut TileCx<'_, '_, Self>,
        _tile: FrameTile<'_, Self::Type>,
        _data: TileData<'_, Self::Type>,
        _previous: Option<Self::Tile>,
    ) -> Option<Self::Tile> {
        None
    }

    /// Makes the layer's draws of the frame, the map's or a drape's, in the
    /// frame's encode, of a layer that draws in a pass and has tiles to draw,
    /// once the engine made the values of the tiles that had none
    /// ([`tile`](Self::tile)): from `tiles`, the frame's tiles of the layer,
    /// in the order the update gave them, each with what it holds for the
    /// layer, or in a drape those that overlap the drape's tile, each in the
    /// world copy the drape draws it in ([`FrameTile::draw_id`]); none for a
    /// type without a source, which draws the tiles it picked
    /// ([`PrepareCx::picked`]). The layer pushes its draws, with the values
    /// of their blocks, into `out`, which holds no draws yet, and copies
    /// again into the tile values what changed of their buckets
    /// ([`TileEntry::take_changes`]), which every frame that draws them
    /// then draws. The engine then writes, uploads and draws them, and draws
    /// the masks of the layer's tiles where a draw clips.
    ///
    /// [`TileEntry::take_changes`]: crate::layer::layer_drawing::TileEntry::take_changes
    fn prepare(&mut self, _cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self::Type>], _out: &mut Draws) {}

    /// Called when the layer's style changed to another of the same id and
    /// type, once it is the layer's, of the values `values`, and before its
    /// values transition. A custom layer whose host the style replaced puts
    /// the old host that drew in `ended`.
    fn style_changed(&mut self, _values: &LayerValues<Self::Type>, _ended: &mut EndedHosts) {}

    /// Draws with the backend's commands that the frame lends the layer
    /// `at` its time in the frame, where its draws asked for them
    /// ([`Draws::lend`]): `lent`, which the layer may use only until this
    /// returns. A frame without commands to lend, such as one outside a
    /// pass at [`LendAt::Pass`], lends none.
    fn lend(&mut self, _at: LendAt, _lent: Lent<'_>) {}

    /// Lets go of a custom layer's host as the layer leaves the renderer: a
    /// host that drew goes to `ended`, to hear that the renderer stopped.
    fn leave(&mut self, _ended: &mut EndedHosts) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(&mut self) {}

    /// Whether the feature `feature` with `geometry`, with `feature_state`,
    /// intersects `region` of its tile as the layer of `values` draws it at
    /// the view's zoom; false unless a layer type overrides it.
    fn query_intersects_feature(
        &self,
        _values: &LayerValues<Self::Type>,
        _region: &QueryRegion<'_>,
        _feature: &dyn mln_style::feature::GeometryTileFeature,
        _geometry: &mln_style::feature::GeometryCollection,
        _feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        false
    }
}

/// A render layer of a [`TypedLayer`] `L`: the layer, and what the engine
/// keeps of it, its base, its values, its tile values and its draws of the
/// map's frame.
pub(crate) struct Hosted<L: TypedLayer> {
    base: RenderLayerBase,
    values: LayerValues<L::Type>,
    tiles: TileMap<L::Tile>,
    /// The tiles that a layer that picks its tiles picked in its update,
    /// in id order, which it draws in the map's frame.
    picked: Vec<OverscaledTileID>,
    draws: Draws,
    layer: L,
}

impl<L: TypedLayer> Hosted<L> {
    /// The render layer of the layer `style`, before its values transition.
    pub(crate) fn new(style: Arc<LayerStyle<L::Type>>) -> Self {
        let (base, values) = LayerValues::new(style);
        let layer = L::new(&values);
        Hosted { base, values, tiles: TileMap::default(), picked: Vec::new(), draws: Draws::default(), layer }
    }

    /// [`new`](Self::new), boxed as a [`RenderLayer`].
    pub(crate) fn boxed(style: Arc<LayerStyle<L::Type>>) -> Box<dyn RenderLayer> {
        Box::new(Self::new(style))
    }

    /// The layer and its values, which benchmarks fill as the layer's
    /// update would.
    #[cfg(feature = "bench")]
    pub(crate) fn layer_mut(&mut self) -> (&mut L, &LayerValues<L::Type>) {
        (&mut self.layer, &self.values)
    }

    /// Makes `variant` the projection the layer draws in: where its draws
    /// were of another, drops them and retires its tile values.
    fn use_projection_variant(&mut self, variant: ProjectionVariant) {
        if variant != self.base.projection_variant {
            self.base.projection_variant = variant;
            self.retire_draws();
        }
    }

    /// Drops the frame's draws of the layer and retires its tile values,
    /// which the next values of their tiles may take from: until the
    /// layer's next draws are done, the draws keep what they kept for the
    /// retired values' meshes. The tiles stay.
    fn retire_draws(&mut self) {
        self.tiles.retire();
        self.draws.clear();
    }

    /// Readies the values of `tiles`, the tiles the layer draws, before the
    /// layer makes its draws of them: retires each that was made from
    /// something else than what its tile shows the layer, or that the layer
    /// doesn't keep ([`TypedLayer::keeps`]), and makes those the layer has
    /// none of ([`TypedLayer::tile`]), from their previous values. For a
    /// layer that picks its tiles, retires those of its picked tiles that it
    /// doesn't keep.
    fn ready_tiles(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        frame: &LayerFrame<'_>,
        tiles: &[FrameTile<'_, L::Type>],
    ) {
        let Hosted { base, values, tiles: map, layer, .. } = self;
        if !has_tiles::<L>() {
            for (_, mut entry) in map.entries() {
                if entry.value.as_ref().is_some_and(|value| !layer.keeps(values, value)) {
                    entry.retire();
                }
            }
            return;
        }
        for &tile in tiles {
            let Some(mut entry) = map.entry(&tile.id()) else { continue };
            entry.show(tile.made_from());
            if entry.value.as_ref().is_some_and(|value| !layer.keeps(values, value)) {
                entry.retire();
            }
            if entry.value.is_none()
                && let Some(data) = tile.data()
            {
                // A new value copies its bucket as it is.
                entry.take_changes();
                let previous = entry.take_previous();
                let res = Resources::new(&mut *gpu, &mut *context);
                *entry.value = layer.tile(&mut TileCx { base, values, res, frame }, tile, data, previous);
            }
        }
    }

    /// Makes the layer's draws of `into`'s frame, the map's or a drape's,
    /// if it draws in a pass: starts them, readies its tile values of
    /// `tiles` ([`ready_tiles`](Self::ready_tiles)), runs its
    /// [`TypedLayer::prepare`] and writes the buffers of the draws it made,
    /// with the paint parameters of `parameters`' frame where it has its own
    /// (a drape's), and the masks of `tiles` where a draw clips. One that
    /// draws in no pass with its values of the frame draws nothing, and
    /// keeps its tile values.
    fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        tiles: &[FrameTile<'_, L::Type>],
        into: DrawsFor<'_>,
    ) {
        if self.values.evaluated.render_passes == RenderPass::None.underlying_type() {
            match into {
                DrawsFor::Map => self.draws.begin(),
                DrawsFor::Drape(draws, _) => draws.begin(),
            }
            return;
        }
        let frame = parameters.frame();
        self.ready_tiles(gpu, parameters.context, &frame, tiles);
        let Hosted { base, values, tiles: map, picked, draws, layer } = self;
        let (draws, picked) = match into {
            DrawsFor::Map => (draws, &picked[..]),
            DrawsFor::Drape(draws, tile) => (draws, std::slice::from_ref(tile)),
        };
        draws.begin();
        let res = Resources::new(&mut *gpu, &mut *parameters.context);
        layer.prepare(&mut PrepareCx { base, values, tiles: map, picked, res, frame: &frame }, tiles, draws);
        let paint_params = parameters.own_paint_params();
        let clip_tiles = tiles.iter().map(|tile| tile.draw_id().to_unwrapped());
        draws.finish(gpu, parameters.context, paint_params.as_ref(), clip_tiles);
    }
}

/// Lends `layer` the backend's commands of `gpu` `at` its time in the frame
/// of `parameters`, where the frame has them to lend: in a pass, the open
/// one. A lend that fails fails the frame.
fn lend<L: TypedLayer>(layer: &mut L, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, at: LendAt) {
    let Some(projection) = parameters.lent_projection else { return };
    let frame = parameters.frame();
    let layer_depth = match at {
        LendAt::BeforePasses => None,
        LendAt::Pass if parameters.pass_target().is_some() => Some(f64::from(parameters.layer_depth())),
        LendAt::Pass => return,
    };
    let lent = gpu.lend(&mut |raw| layer.lend(at, Lent { raw, frame: &frame, projection: &projection, layer_depth }));
    if let Err(error) = lent {
        parameters.context.fail(format!("Lending a layer the frame's commands failed: {error}"));
    }
}

/// Whether layers of `L`'s type draw from their source's tiles: false for a
/// type without a source.
fn has_tiles<L: TypedLayer>() -> bool {
    <<L::Type as LayerType>::Input as Find<L::Type>>::KIND != SourceKind::None
}

/// What the engine does with a render layer, whatever its type: its base,
/// its values, which this transitions without knowing their type, its tile
/// values and its draws, which it writes, uploads and draws. It runs
/// the layer's [`TypedLayer`] hooks with what is common to every layer type
/// around them.
pub(crate) trait RenderLayer {
    fn base(&self) -> &RenderLayerBase;

    /// The layer's id.
    fn id(&self) -> &str {
        &self.base().style.base().id
    }

    /// The order the layer draws its source's tiles in.
    fn tile_order(&self) -> TileOrder;

    /// Makes `style`, the changed style of the layer (of the same id and
    /// type), the layer's, drops the draws, which were built for the
    /// previous style's values, with the tile values unless the layer keeps
    /// them ([`TypedLayer::RESTYLE_KEEPS_TILES`]), and begins transitions to
    /// the new values. A style of another type leaves the layer's. The next
    /// encode builds the draws again from the tiles' buckets. A custom
    /// layer's host that drew and that the style replaced goes to `ended`.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>, ended: &mut EndedHosts);

    /// Begins transitions for the paint values that changed since the last
    /// frame, to those of the layer's style.
    fn transition(&mut self, parameters: &TransitionParameters);

    /// Evaluates the layer's paint values for a frame of `parameters`,
    /// which the layer completes ([`TypedLayer::evaluate`]), and sets the
    /// passes the layer draws in with them.
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters);

    /// Whether a paint value of the layer is still in transition.
    fn has_transition(&self) -> bool;

    /// Whether the layer has a pattern property and is in a cross-fade.
    fn has_crossfade(&self) -> bool;

    /// The frame's update of the layer: finds the tiles of `parameters`'
    /// source the layer draws, which it adds to `parameters.found`, adds the
    /// passes their buckets were laid out for to the layer's, runs the
    /// layer's [`TypedLayer::update`], and keeps entries for the tiles; or,
    /// for a type without a source, runs its update without tiles.
    fn update(&mut self, parameters: &mut LayerUpdateParameters<'_>);

    /// The encode's update of the layer's draws: picks the shader variant
    /// for the frame's projection, standing on the frame's ground where the
    /// layer stands and the frame has one, dropping the draws and tile values
    /// when it changed; drops them of a layer that has nothing to draw, and
    /// otherwise starts its draws of the frame; for a layer that draws in a
    /// pass, readies the tile values and runs the layer's
    /// [`TypedLayer::prepare`] with the tiles of its type that its update
    /// found in `source`, or none for a type without a source, and writes
    /// the buffers of its draws.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
    );

    /// How the layer meets terrain ([`TypedLayer::TERRAIN`]).
    fn terrain(&self) -> OnTerrain;

    /// Whether the layer draws 3D geometry ([`TypedLayer::VOLUME`]).
    fn volume(&self) -> bool;

    /// Whether the layer blends with what is under it otherwise than by
    /// covering it, with its values of the frame: a layer blend
    /// ([`StyleLayerType::layer_blend`]) other than `normal`.
    fn blends(&self) -> bool;

    /// The light the terrain's surface can shade the layer's drapes with
    /// ([`TypedLayer::surface_light`]).
    fn surface_light(&mut self, parameters: &PaintParameters<'_>) -> Option<SurfaceLight>;

    /// Where the tiles the layer's update found are among the frame's
    /// ([`LayerUpdateParameters::found`]); `None` for a type without a
    /// source.
    fn found(&self) -> Option<Range<usize>>;

    /// The encode's update of the layer's draws into `drape`, `draws`, as
    /// [`update_draws`](Self::update_draws) makes the frame's, with the
    /// drape's `parameters`, from the tiles of `source` the layer's update
    /// found that overlap the drape's tile
    /// ([`FrameSourceTiles::overlapping`]), or from the drape's tile itself
    /// for a type without a source that picks its tiles, in the Mercator
    /// projection, a drape's. The drape draws the layer's tile values,
    /// which the frame's other drapes share: for a layer that picks its
    /// tiles, those of the tiles of every drape.
    fn update_drape_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
        drape: DrapeTile<'_>,
        draws: &mut Draws,
    );

    /// Records that the buckets in `changed` changed, for the tiles whose
    /// values were made from them.
    fn mark_changed(&mut self, changed: &ChangedBuckets);

    /// Moves the layer's value of each tile to the tile `wrap_delta` world
    /// copies east of it: the tiles are the same ones under new ids once the
    /// map's center has crossed the antimeridian, and their values stay
    /// rather than being made again.
    fn handle_wrap_jump(&mut self, wrap_delta: i16);

    /// Includes the layer in the frame's drawing or excludes it. A layer
    /// excluded drops its draws and its tiles with their values, previous
    /// ones too: its updates don't run until it is included again, and they
    /// then rebuild everything from the source's tiles.
    fn set_renderable(&mut self, renderable: bool);

    /// Drops the layer's tile values, previous ones too, and the frame's
    /// draws of the layer, which then let go of what they kept for the
    /// values' meshes. The tiles stay.
    fn remove_all_draws(&mut self);

    /// Drops the map frame's draws of the layer, which a frame over terrain
    /// draws only in drapes; its tile values stay, for the drapes.
    fn remove_frame_draws(&mut self);

    /// Visits the groups of the layer's draws of the frame that take a place
    /// in the layers' order and depth, in drawing order: none unless the
    /// frame draws the layer. They are the groups `upload` and `render`
    /// upload and draw.
    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a Group));

    /// Uploads the meshes of the layer's draws, before the frame draws them.
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context);

    /// Draws the pass's draws of the layer's groups into the open pass of
    /// `gpu`, the context's, in `order`, each at its depth, and, in the
    /// translucent pass, lends the layer the pass after its main group where
    /// its draws asked for it ([`LendAt::Pass`]).
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);

    /// Before the frame's passes, outside any pass of `gpu`, the context's:
    /// draws into the targets of the layer's draws of the frame
    /// ([`Draws::target`]), and then lends the layer the frame's commands
    /// where its draws asked for them ([`LendAt::BeforePasses`]).
    fn before_passes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>);

    /// See [`TypedLayer::leave`]. The orchestrator calls it before it drops
    /// the layer.
    fn leave(&mut self, ended: &mut EndedHosts);

    /// See [`TypedLayer::lose_context`].
    fn lose_context(&mut self);

    /// See [`TypedLayer::query_intersects_feature`].
    fn query_intersects_feature(
        &self,
        region: &QueryRegion<'_>,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        feature_state: &mln_style::feature::FeatureState,
    ) -> bool;

    /// Whether the layer draws in a pass and is visible.
    fn needs_rendering(&self) -> bool {
        self.base().passes != RenderPass::None && self.base().style.base().visibility != VisibilityType::None
    }

    /// Whether the layer draws at `zoom`. A minimum zoom of 0 is no
    /// minimum: the globe zooms below 0 toward the poles.
    fn supports_zoom(&self, zoom: f32) -> bool {
        let zoom_or_zero = if zoom < 0.0 { 0.0 } else { zoom };
        self.base().style.base().min_zoom <= zoom_or_zero && zoom < self.base().style.base().max_zoom
    }

    /// Whether the frame draws the layer.
    fn is_renderable(&self) -> bool {
        self.base().is_renderable
    }

    /// What the layer's paint values depend on.
    fn style_dependencies(&self) -> Dependency {
        self.base().style_dependencies
    }
}

impl<L: TypedLayer> RenderLayer for Hosted<L> {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn tile_order(&self) -> TileOrder {
        L::TILE_ORDER
    }

    fn restyle(
        &mut self,
        parameters: &TransitionParameters,
        style: &Arc<dyn ErasedLayerStyle>,
        ended: &mut EndedHosts,
    ) {
        if L::RESTYLE_KEEPS_TILES {
            self.draws.clear();
        } else {
            self.retire_draws();
        }
        if let Some(recovered) = LayerStyle::recover(style) {
            self.values.style = recovered;
            self.base.style = Arc::clone(style);
        }
        self.layer.style_changed(&self.values, ended);
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let values = &mut self.values;
        let prior = std::mem::take(&mut values.unevaluated);
        values.unevaluated = values.style.paint.transitioned(parameters, prior);
        self.base.style_dependencies = <L::Type as StyleLayerType>::PaintProperties::dependencies(&values.unevaluated);
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let mut evaluated = self.values.evaluate(parameters);
        let evaluation =
            self.layer.evaluate(EvaluateCx { values: &self.values, parameters, evaluated: &mut evaluated });
        self.base.passes = evaluation.passes();
        self.base.evaluation = evaluation;
        self.values.set_evaluated(&mut self.base, evaluated);
    }

    fn has_transition(&self) -> bool {
        <L::Type as StyleLayerType>::PaintProperties::has_transition(&self.values.unevaluated)
    }

    fn has_crossfade(&self) -> bool {
        L::Type::CROSSFADES && self.values.evaluated.crossfade.t != 1.0
    }

    fn update(&mut self, parameters: &mut LayerUpdateParameters<'_>) {
        let Hosted { base, values, tiles: map, picked, layer, .. } = self;
        if !has_tiles::<L>() {
            layer.update(UpdateCx { values, parameters, picked }, &[]);
            return;
        }
        let start = parameters.found.len();
        let tiles = match parameters.tiles(L::TILE_ORDER) {
            Some((tiles, order)) => tiles.find::<L::Type>(order, &base.style.base().id, parameters.found),
            None => FrameTiles::new(),
        };
        base.found = start..parameters.found.len();
        for data in tiles.iter().filter_map(|tile| tile.data()) {
            base.passes |= <L::Type as LayerType>::Input::passes(data);
        }
        layer.update(UpdateCx { values, parameters, picked }, &tiles);
        map.set_tiles(tiles.iter().map(|tile| tile.id()));
    }

    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
    ) {
        let stands = L::TERRAIN.stands() && parameters.ground.is_some();
        self.use_projection_variant(ProjectionVariant::new(parameters.view.is_globe(), stands));
        if !has_tiles::<L>() {
            // A layer that picks tiles draws those its update picked.
            if L::PICKS_TILES {
                self.tiles.set_tiles(self.picked.iter().copied());
            }
            if L::PICKS_TILES && self.tiles.is_empty() {
                self.remove_all_draws();
            } else {
                self.draw(gpu, parameters, &[], DrawsFor::Map);
            }
            return;
        }
        match source.map(|source| (source, source.found(self.base.found.clone()))) {
            Some((source, found)) if !found.is_empty() => {
                let tiles = source.at::<L::Type>(found, self.id());
                self.draw(gpu, parameters, &tiles, DrawsFor::Map);
            }
            _ => self.remove_all_draws(),
        }
    }

    fn terrain(&self) -> OnTerrain {
        L::TERRAIN
    }

    fn volume(&self) -> bool {
        L::VOLUME
    }

    fn blends(&self) -> bool {
        L::Type::layer_blend(&self.values.evaluated.paint) != LayerBlendType::Normal
    }

    fn surface_light(&mut self, parameters: &PaintParameters<'_>) -> Option<SurfaceLight> {
        self.layer.surface_light(&self.values, &parameters.frame())
    }

    fn found(&self) -> Option<Range<usize>> {
        has_tiles::<L>().then(|| self.base.found.clone())
    }

    fn update_drape_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        source: Option<FrameSourceTiles<'_>>,
        drape: DrapeTile<'_>,
        draws: &mut Draws,
    ) {
        // Only a layer that drapes builds the code of its draws in a drape.
        if !L::TERRAIN.drapes() {
            debug_assert!(false, "a layer that doesn't drape in a drape");
            return;
        }
        // A drape is flat: the layer's draws are Mercator's.
        self.use_projection_variant(ProjectionVariant::Mercator);
        if !has_tiles::<L>() {
            if L::PICKS_TILES {
                self.tiles.set_tiles(drape.tiles.iter().copied());
            }
            self.draw(gpu, parameters, &[], DrawsFor::Drape(draws, &drape.tile));
            return;
        }
        let tiles = match source {
            Some(source) => source.overlapping::<L::Type>(
                self.base.found.clone(),
                self.id(),
                drape.tile.to_unwrapped(),
                drape.globe,
            ),
            None => FrameTiles::new(),
        };
        if tiles.is_empty() {
            draws.clear();
        } else {
            self.draw(gpu, parameters, &tiles, DrawsFor::Drape(draws, &drape.tile));
        }
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.tiles.mark_changed(changed);
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16) {
        self.tiles.shift_wraps(wrap_delta);
    }

    fn set_renderable(&mut self, renderable: bool) {
        self.base.is_renderable = renderable;
        if !renderable {
            self.tiles.clear_tiles();
            self.draws.clear();
        }
    }

    fn remove_all_draws(&mut self) {
        self.tiles.clear();
        self.draws.clear();
    }

    fn remove_frame_draws(&mut self) {
        self.draws.clear();
    }

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a Group)) {
        if self.base.is_renderable {
            self.draws.visit_groups(visit);
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.base.is_renderable {
            self.draws.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        let Hosted { base, draws, layer, .. } = self;
        if base.is_renderable {
            let lends = draws.lends(LendAt::Pass) && parameters.pass == RenderPass::Translucent;
            let mut lend_pass = |gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>| {
                lend(layer, gpu, parameters, LendAt::Pass);
            };
            draws.render(gpu, parameters, order, lends.then_some(&mut lend_pass));
        }
    }

    fn before_passes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.base.is_renderable {
            self.draws.render_targets(gpu, parameters);
            if self.draws.lends(LendAt::BeforePasses) {
                lend(&mut self.layer, gpu, parameters, LendAt::BeforePasses);
            }
        }
    }

    fn leave(&mut self, ended: &mut EndedHosts) {
        self.layer.leave(ended);
    }

    fn lose_context(&mut self) {
        self.layer.lose_context();
    }

    fn query_intersects_feature(
        &self,
        region: &QueryRegion<'_>,
        feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        self.layer.query_intersects_feature(&self.values, region, feature, geometry, feature_state)
    }
}
