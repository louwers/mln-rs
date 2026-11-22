//! The layer contract: what a crate implements to add a layer type that the
//! renderer draws. `mln_extension_tests`' `dot` layer type, which draws a
//! geometry source's points as discs, is the smallest whole example; the
//! facade's `mln_rs::layer` gathers every name it uses.
//!
//! A layer type has two halves, which
//! [`RegistryBuilder::add_layer_type::<L>()`](crate::RegistryBuilder::add_layer_type)
//! registers together:
//!
//! - **The style half**, a marker type that implements `mln_style`'s
//!   [`StyleLayerType`]: the type's name, its kind of source, its properties
//!   ([`mln_style::properties!`] declares them) and its
//!   [`Data`](StyleLayerType::Data), what an app gives a layer outside the
//!   style JSON (`Map::edit_style_layer_data`).
//! - **The render half**, a struct that implements [`Layer`], one value per
//!   style layer of the type.
//!
//! # What a layer type writes
//!
//! 1. Its tiles' data. A layer of a geometry source has the input
//!    [`Features<L>`]: on a tile worker, its [`Layout`] `L` selects the
//!    tile's features ([`LayoutCx`]) and builds a [`Bucket`] of them
//!    ([`BuildCx`]): vertices in [`GpuVec`]s, their triangles in
//!    [`Indexes`], and the values of its data-driven paint properties in
//!    [`LayerBinders`], declared with
//!    [`paint_property_binders!`](crate::paint_property_binders!). A layer
//!    without a source has the input [`ViewCover`].
//! 2. Its meshes. [`Layer::tile`] makes the value the layer keeps of a
//!    frame tile, typically a [`Mesh`](crate::draw::Mesh) of the tile's
//!    bucket ([`LayerBucket`]) with the program the type declares. The
//!    engine keeps the values with their tiles and drops them when the
//!    tiles or their data go, or the map switches between Mercator and the
//!    globe. A type whose meshes order across its tiles makes the values in
//!    [`Layer::update`] instead ([`Tiles::get_mut`]).
//! 3. Its draws. [`Layer::prepare`] pushes a [`Draw`](crate::draw::Draw)
//!    of each mesh, with its tile's projection and the values of its
//!    program's blocks, to [`Draws`]. The engine writes what the blocks
//!    hold of the mesh's paint values, and orders, uploads and draws them:
//!    the layer never sees a pass, a depth range, a stencil value or a
//!    uniform buffer.
//!
//! [`Layer::evaluate`] says whether, and how, the layer draws with its
//! freshly evaluated values; [`Layer::update`] brings up to date what the
//! layer keeps for all its tiles, such as an atlas of images.
//!
//! # When the hooks run
//!
//! The engine makes the layer ([`Layer::new`]) when it joins the renderer,
//! again when the renderer makes its shaders again or after it lost its GPU
//! context, and drops it when it leaves the style. It evaluates the
//! layer whenever its values may have changed, its `Data` included. In
//! each frame that draws the layer it runs [`update`](Layer::update), then
//! [`tile`](Layer::tile) for each frame tile with data and no value, then
//! [`prepare`](Layer::prepare). When the layer stops drawing, its tile
//! values go and [`stopped`](Layer::stopped) runs. The layer, its values
//! and every hook but the layout's run on the renderer's thread; the
//! layout and the bucket are `Send`, and a bucket implements [`Wire`]
//! (`#[derive(Wire)]`, with `#[wire(crate = mln_rs::mln_wire)]` through
//! the facade) to cross to a tile worker with memory of its own.
//!
//! # Queries
//!
//! A rendered-feature query finds a tile's features near its geometry in
//! the tile's feature index ([`BuildCx::index`]), padded by the layer's
//! [`query_radius`](Layer::query_radius), and keeps those the layer
//! [`hit`](Layer::hit)s; [`hit_frame`](Layer::hit_frame) adds what the
//! layer draws without tile features. A layer's hits come nearest first.
//! `RenderSession::query_rendered_layer` reads a layer as its renderer has
//! it.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_map_types::{MapMode, TileProjector, View};
use mln_style::PropertyEvaluationParameters;
use mln_style::evaluated::Evaluated;
use mln_style::expression::expr::FeatureState;
use mln_style::expression::image_expression::UnavailableImages;
use mln_style::expression::{EvaluationContext, Value, ValueConverter};
use mln_style::layer_type::{LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style_types::TranslateAnchorType;
use mln_style_types::image::{ImageDependencies, ImageType};
use mln_text::glyph::{FontFaces, GlyphDependencies, GlyphID, GlyphIDType};
use mln_tile::{Feature, TileLayer};
use mln_tile_id::geometry_tile_data::{
    GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature,
};
use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::font_stack::FontStack;
use mln_util::geo::ScreenCoordinate;
use mln_vec_map::VecSet;
use mln_wire::Wire;

pub use crate::bucket_geometry::GpuVec;
pub use crate::bucket_upload::Upload;
use crate::draw::{Buffer, Draws, IndexBuffer, Resources, Texture};
pub use crate::erased_layer::EvaluatedLayer;
use crate::geometry::feature_index::{FeatureIndex, FeatureIndexBuilder};
pub use crate::gfx::mesh::Indexes;
pub use crate::image_store::ImageStore;
use crate::layout::filter_groups::FilterGroups;
pub use crate::layout::tile_layout::Resolved;
pub use crate::paint_parameters::Frame;
pub use crate::paint_property_binder::{LayerBinders, PaintBinders, PaintPropertyBinder, PaintVertices};
pub use crate::render_light::EvaluatedLight;
use crate::render_tile::{TileContent, ViewPlace};
use crate::tile::contents::GeometryContents;
pub use crate::tile_matrix::{TileProjection, TileTransform};
use crate::tile_values::TileMap;

/// The render half of a layer type: one value per style layer of the type,
/// which the engine makes when the layer joins the renderer and drops when
/// it leaves.
pub trait Layer: Sized + 'static {
    /// The style half: the type's name, source kind, properties and data.
    type Style: StyleLayerType;

    /// What the layer draws from ([`Features`] or [`ViewCover`]). Its kind
    /// of source is the style half's
    /// [`INFO.source`](mln_style::layer_type::LayerTypeInfo::source), or
    /// the registration doesn't compile.
    type Input: TileInput<Self::Style>;

    /// What the layer keeps of each frame tile it draws, such as the meshes
    /// of its draws. `()` for a layer that keeps nothing per tile.
    type Tile: 'static;

    /// Whether the layer draws 3D geometry, with draws of
    /// [`Depth::Volume`](crate::draw::Depth::Volume), whether or not it
    /// draws in a frame. The layers above the lowest such layer draw their
    /// opaque draws in the translucent pass, and on the globe the planet's
    /// depth hides 3D geometry behind the horizon while any such layer
    /// renders.
    const VOLUME: bool = false;

    /// The layer for the values `cx.layer`, before they transition. It may
    /// make the GPU resources it keeps for every frame.
    fn new(cx: &mut NewCx<'_, Self>) -> Self;

    /// Says how the layer draws with `cx.evaluated`, its values freshly
    /// evaluated, and derives the values its type derives from others. The
    /// engine calls it whenever the values may have changed: when the
    /// layer is added or restyled, and on zoom, global state, cross-fade and
    /// transition changes. By default the layer draws, translucent.
    fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    /// The frame's update of the layer, before its tiles' values are made:
    /// what the layer keeps for all its tiles, such as an atlas of images,
    /// is brought up to date here, and values of tiles that the layer makes
    /// in an order of its own.
    fn update(&mut self, _cx: &mut UpdateCx<'_, Self>) {}

    /// The value of `tile`, which has `data` for the layer and no value yet:
    /// typically the meshes the layer draws the tile with. `None` keeps the
    /// tile without a value, which [`prepare`](Self::prepare) then doesn't
    /// see, until the next frame asks again.
    fn tile(&mut self, _cx: &mut TileCx<'_, Self>, _tile: FrameTile, _data: TileData<'_, Self>) -> Option<Self::Tile> {
        None
    }

    /// Pushes the frame's draws of the layer to `out`.
    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws);

    /// Called when the layer stops rendering, once the engine dropped its
    /// tile values: the layer drops what else it keeps for drawing.
    fn stopped(&mut self) {}

    /// How far, in logical pixels, outside a feature of `data` a query
    /// still hits it with the layer's values `layer`, seen by `camera`. A
    /// tile's query geometry grows by its layers' largest radius.
    fn query_radius(&self, _layer: &LayerValues<Self::Style>, _data: TileData<'_, Self>, _camera: &View) -> f32 {
        0.0
    }

    /// Whether `test`'s feature, as the layer draws it, is hit by the
    /// query's geometry: its distance in pixels from the query, 0 inside
    /// it, or `None` for a miss.
    fn hit(&self, _test: &HitTest<'_, Self>) -> Option<f32> {
        None
    }

    /// Adds to `hits` what the query's geometry hits of what the layer
    /// draws without tile features, such as a mark it places itself. The
    /// engine asks only a layer that rendered in the last frame.
    fn hit_frame(&self, _query: &FrameQuery<'_, Self>, _hits: &mut Vec<Hit>) {}
}

/// What a layer draws from: a kind of tile and what such a tile holds for
/// the layer, its [`Data`](TileInput::Data). The kinds are the engine's:
/// [`Features`] and [`ViewCover`].
#[expect(private_bounds, reason = "the engine's tile kinds are the only ones: `input::Find` seals the trait")]
pub trait TileInput<S: StyleLayerType>: input::Find<S> + 'static {
    /// What a frame tile holds for the layer.
    type Data<'a>: Copy;
}

/// What a frame tile holds for a layer `L`: its [`Layer::Input`]'s
/// [`Data`](TileInput::Data).
pub type TileData<'a, L> = <<L as Layer>::Input as TileInput<<L as Layer>::Style>>::Data<'a>;

/// The tiles of a geometry source, whose features the layout `L` turns into
/// a bucket per tile. A tile's data is its [`LayerBucket`].
pub struct Features<L>(PhantomData<fn() -> L>);

impl<S: StyleLayerType, L: Layout<S>> TileInput<S> for Features<L> {
    type Data<'a> = LayerBucket<'a, S, L::Bucket>;
}

/// The tiles that cover the view at its integer zoom, for a layer without a
/// source, which draws over the whole map. A tile holds nothing: its data
/// is `()`.
pub enum ViewCover {}

impl<S: StyleLayerType> TileInput<S> for ViewCover {
    type Data<'a> = ();
}

pub(crate) mod input {
    use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
    use mln_style::layer_type::{SourceKind, StyleLayerType};
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;
    use mln_util::range::Range as ZoomRange;
    use smallvec::SmallVec;

    use super::{Features, Found, FrameTile, Layout, TileInput, ViewCover};
    use crate::render_layer::{FrameSourceTiles, LayerUpdate};
    use crate::render_tile::{FoundTile, TileContent, ViewPlace};
    use crate::tile::contents::GeometryContents;
    use crate::tile_values::TileMap;

    /// The engine's side of a [`TileInput`]: which tiles a layer of it draws
    /// in a frame, and what each holds for it. It seals [`TileInput`].
    pub(crate) trait Find<S: StyleLayerType>: crate::erased_layer::LayoutOf<S> {
        /// The kind of source whose tiles hold the input.
        const SOURCE: SourceKind;

        /// What the engine keeps of a layer's frame tiles from its update to
        /// its encode.
        type Frame: Default;

        /// Finds the frame tiles of the layer `layer_id` in `update`, keeps
        /// them in `frame` and their ids in `values`. Returns whether one of
        /// them holds something for the layer to draw.
        fn update<T>(
            frame: &mut Self::Frame,
            update: &mut LayerUpdate<'_>,
            layer_id: &str,
            values: &mut TileMap<T>,
        ) -> bool;

        /// The layer's frame tiles that its update kept in `frame`, with
        /// where what each holds for it is in `source`'s tiles, whose
        /// `found` ones the frame's updates found, into `tiles`; drops the
        /// values in `values` of those whose data changed.
        fn tiles<'a, T>(
            frame: &Self::Frame,
            found: &[FoundTile],
            source: Option<FrameSourceTiles<'a>>,
            values: &mut TileMap<T>,
            tiles: &mut SmallVec<[Found<'a>; 64]>,
        );

        /// What `found` holds for the layer to draw.
        fn data(found: Found<'_>) -> Option<Self::Data<'_>>
        where
            Self: TileInput<S>;

        /// What `contents`, a tile's layout, holds for the layer `layer_id`.
        fn laid<'a>(_contents: &'a GeometryContents, _layer_id: &str) -> Option<Self::Data<'a>>
        where
            Self: TileInput<S>,
        {
            None
        }
    }

    /// The identity of the bucket that `data` shows a layer to draw, which
    /// the layer's value of the tile is made of; `None` where it shows
    /// nothing to draw.
    fn identity<S: StyleLayerType, B>(data: super::LayerBucket<'_, S, B>) -> Option<SimpleIdentity> {
        data.laid_with.draws.then_some(data.id)
    }

    /// What `content` holds at `place` for a layer of the style type `S`
    /// laid out by `L`.
    fn at<S: StyleLayerType, L: Layout<S>>(
        content: TileContent<'_>,
        place: ViewPlace,
    ) -> Option<super::LayerBucket<'_, S, L::Bucket>> {
        let ViewPlace::Laid(index) = place else {
            debug_assert!(false, "a tile's own data's place for a laid-out layer");
            return None;
        };
        let TileContent::Tile(contents) = content else { return None };
        crate::erased_layer::laid::<S, L>(contents.geometry()?, index)
    }

    impl<S: StyleLayerType, L: Layout<S>> Find<S> for Features<L> {
        const SOURCE: SourceKind = SourceKind::Geometry;

        /// Where the layer's tiles are among the frame's found tiles.
        type Frame = std::ops::Range<usize>;

        fn update<T>(
            frame: &mut Self::Frame,
            update: &mut LayerUpdate<'_>,
            layer_id: &str,
            values: &mut TileMap<T>,
        ) -> bool {
            let start = update.found.len();
            let mut draws = false;
            let mut ids: SmallVec<[OverscaledTileID; 64]> = SmallVec::new();
            if let Some(source) = update.source
                && let Some(order) = source.tile_order()
            {
                let tiles = source.render_tiles();
                let placing;
                let order = if update.places {
                    placing = crate::placement::tile_order(tiles, update.view.angle() as f32);
                    &placing
                } else {
                    order
                };
                for &index in order {
                    let Some(tile) = tiles.get(index) else { continue };
                    let content = update.contents.get(index).copied().flatten();
                    let found = content.and_then(|content| {
                        let TileContent::Tile(contents) = content else { return None };
                        let contents = contents.geometry()?;
                        let place = contents.result.layer_render_data.find(layer_id)?;
                        Some((crate::erased_layer::laid::<S, L>(contents, place)?, ViewPlace::Laid(place)))
                    });
                    draws |= found.is_some_and(|(data, _)| identity(data).is_some());
                    update.found.push(FoundTile { tile: index, view: found.map(|(_, place)| place) });
                    ids.push(*tile.overscaled_tile_id());
                }
            }
            values.set_tiles(ids);
            *frame = start..update.found.len();
            draws
        }

        fn tiles<'a, T>(
            frame: &Self::Frame,
            found: &[FoundTile],
            source: Option<FrameSourceTiles<'a>>,
            values: &mut TileMap<T>,
            tiles: &mut SmallVec<[Found<'a>; 64]>,
        ) {
            let Some(source) = source else { return };
            for (index, &FoundTile { tile, view }) in found.get(frame.clone()).unwrap_or_default().iter().enumerate() {
                let Some(render_tile) = source.tiles.get(tile) else { continue };
                let content = source.contents.get(tile).copied().flatten();
                let at = content.zip(view);
                values.show(index, at.and_then(|(content, place)| identity(self::at::<S, L>(content, place)?)));
                let id = *render_tile.overscaled_tile_id();
                tiles.push(Found { tile: FrameTile { index, id, unwrapped: render_tile.id }, at });
            }
        }

        fn data(found: Found<'_>) -> Option<<Self as TileInput<S>>::Data<'_>> {
            let (content, place) = found.at?;
            let data = at::<S, L>(content, place)?;
            identity(data).is_some().then_some(data)
        }

        fn laid<'a>(contents: &'a GeometryContents, layer_id: &str) -> Option<<Self as TileInput<S>>::Data<'a>> {
            let index = contents.result.layer_render_data.find(layer_id)?;
            crate::erased_layer::laid::<S, L>(contents, index)
        }
    }

    impl<S: StyleLayerType> Find<S> for ViewCover {
        const SOURCE: SourceKind = SourceKind::None;

        /// The tiles that cover the view at its integer zoom.
        type Frame = Vec<OverscaledTileID>;

        fn update<T>(
            frame: &mut Self::Frame,
            update: &mut LayerUpdate<'_>,
            _layer_id: &str,
            values: &mut TileMap<T>,
        ) -> bool {
            let (view, parameters) = (update.view, update.update_parameters);
            let zoom = view.integer_zoom();
            let cover = TileCoverParameters {
                view: view.clone(),
                tile_lod_min_radius: parameters.tile_lod_min_radius,
                tile_lod_scale: parameters.tile_lod_scale,
                tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
                tile_lod_mode: parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
                max_content_elevation: 0.0,
            };
            *frame = tile_cover(&cover, zoom, ZoomRange::new(0u8, zoom), None);
            values.set_tiles(frame.iter().copied());
            false
        }

        fn tiles<'a, T>(
            frame: &Self::Frame,
            _found: &[FoundTile],
            _source: Option<FrameSourceTiles<'a>>,
            _values: &mut TileMap<T>,
            tiles: &mut SmallVec<[Found<'a>; 64]>,
        ) {
            for (index, &id) in frame.iter().enumerate() {
                tiles.push(Found { tile: FrameTile { index, id, unwrapped: id.to_unwrapped() }, at: None });
            }
        }

        fn data(_found: Found<'_>) -> Option<<Self as TileInput<S>>::Data<'_>> {
            Some(())
        }
    }
}

/// A layer's values: its style, its paint values in transition, and their
/// evaluation for the last frame. The engine keeps and changes them; the
/// hooks read them.
pub struct LayerValues<S: StyleLayerType> {
    /// The layer's style, with its [`Data`](StyleLayerType::Data).
    pub style: Arc<LayerStyle<S>>,
    /// The paint values, in transition.
    pub unevaluated: <S::PaintProperties as PaintSet>::Unevaluated,
    /// The paint values evaluated for the last frame.
    pub evaluated: Arc<EvaluatedLayer<S>>,
}

impl<S: StyleLayerType> LayerValues<S> {
    /// The layer's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.style.id
    }
}

/// What a new layer reads, and the resources it may make.
pub struct NewCx<'a, L: Layer> {
    /// The layer's values, before they transition.
    pub layer: &'a LayerValues<L::Style>,
    /// The renderer's resources.
    pub res: &'a mut Resources,
}

/// What a layer's evaluation reads and writes.
pub struct EvaluateCx<'a, L: Layer> {
    /// The layer's values: its style, and its paint values in transition.
    /// Their `evaluated` is still the last evaluation's.
    pub layer: &'a LayerValues<L::Style>,
    /// What the values were evaluated for.
    pub parameters: &'a PropertyEvaluationParameters,
    /// The new evaluation of the values, which becomes the layer's once the
    /// hook returns.
    pub evaluated: &'a mut EvaluatedLayer<L::Style>,
}

/// What a layer's evaluation says of how it draws with its values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Evaluation {
    /// Whether the layer draws anything.
    pub draws: bool,
    /// Whether the layer's opaque draws
    /// ([`Draw::opaque`](crate::draw::Draw::opaque)) may draw in the opaque
    /// pass, which the frame draws front to back before it blends the rest.
    pub opaque: bool,
    /// The one solid color the layer is, which the frame clears to instead
    /// of drawing the layer when it is the style's bottom layer.
    pub solid: Option<Color>,
}

impl Evaluation {
    /// The layer draws nothing.
    pub const NONE: Evaluation = Evaluation { draws: false, opaque: false, solid: None };
    /// The layer draws, translucent.
    pub const TRANSLUCENT: Evaluation = Evaluation { draws: true, opaque: false, solid: None };
    /// The layer draws, and may draw opaque.
    pub const OPAQUE: Evaluation = Evaluation { draws: true, opaque: true, solid: None };
}

/// What a layer's update reads of the frame, and the resources it may
/// change.
pub struct UpdateCx<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The frame.
    pub frame: &'a Frame<'a>,
    /// The style's images.
    pub images: &'a ImageStore,
    /// The layer's frame tiles and its values of them.
    pub tiles: Tiles<'a, L>,
    /// The renderer's resources.
    pub res: &'a mut Resources,
}

/// A layer's frame tiles, in render order, with its values of them.
pub struct Tiles<'a, L: Layer> {
    pub(crate) found: &'a [Found<'a>],
    pub(crate) values: &'a mut TileMap<L::Tile>,
}

/// One of a layer's frame tiles, with where its content holds the layer's
/// data.
#[derive(Clone, Copy)]
pub(crate) struct Found<'a> {
    pub(crate) tile: FrameTile,
    pub(crate) at: Option<(TileContent<'a>, ViewPlace)>,
}

impl<'a> Found<'a> {
    /// What the tile holds for a layer `L` to draw.
    pub(crate) fn data<L: Layer>(self) -> Option<TileData<'a, L>> {
        <L::Input as input::Find<L::Style>>::data(self)
    }
}

impl<L: Layer> Tiles<'_, L> {
    /// The number of tiles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.found.len()
    }

    /// Whether the frame has no tiles of the layer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }

    /// Drops the values of all the tiles, which the frame then makes again
    /// ([`Layer::tile`]).
    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// The `i`th tile, what it holds for the layer to draw, and the layer's
    /// value of it.
    pub fn get_mut(&mut self, i: usize) -> (FrameTile, Option<TileData<'_, L>>, &mut Option<L::Tile>) {
        let found = self.found[i];
        (found.tile, found.data::<L>(), self.values.get_mut(found.tile.index))
    }
}

/// What making a tile's value reads, and the resources it makes.
pub struct TileCx<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The frame.
    pub frame: &'a Frame<'a>,
    /// The renderer's resources.
    pub res: &'a mut Resources,
}

/// What a layer's draws read of the frame.
pub struct PrepareCx<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The frame.
    pub frame: &'a Frame<'a>,
    /// The renderer's resources, to read: sizes and capabilities.
    pub res: &'a Resources,
    pub(crate) found: &'a [Found<'a>],
    pub(crate) values: &'a TileMap<L::Tile>,
}

impl<'a, L: Layer> PrepareCx<'a, L> {
    /// The tiles with data for the layer and a value of it, in render
    /// order, each with its data and the value.
    pub fn tiles(&self) -> impl Iterator<Item = (FrameTile, TileData<'a, L>, &'a L::Tile)> + '_ {
        (0..self.found.len()).filter_map(|i| self.tile(i))
    }

    /// The `i`th tile, with its data and the layer's value, if it has both.
    #[must_use]
    pub fn tile(&self, i: usize) -> Option<(FrameTile, TileData<'a, L>, &'a L::Tile)> {
        let found = *self.found.get(i)?;
        Some((found.tile, found.data::<L>()?, self.values.get(found.tile.index)?))
    }
}

/// One of a layer's frame tiles: where it is among them, and its ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTile {
    pub(crate) index: usize,
    pub(crate) id: OverscaledTileID,
    pub(crate) unwrapped: UnwrappedTileID,
}

impl FrameTile {
    /// The tile's index among the layer's frame tiles, in render order.
    #[must_use]
    pub fn index(self) -> usize {
        self.index
    }

    /// The tile's id.
    #[must_use]
    pub fn id(self) -> OverscaledTileID {
        self.id
    }

    /// The tile's id in the world copy the frame draws it in.
    #[must_use]
    pub fn unwrapped_id(self) -> UnwrappedTileID {
        self.unwrapped
    }

    /// Where the tile's geometry draws in `frame`, placed as `how` says. A
    /// draw pushed with it can clip to the tile
    /// ([`Draw::clip`](crate::draw::Draw::clip)).
    #[must_use]
    pub fn project(self, frame: &Frame<'_>, how: &TileTransform) -> TileProjection {
        let data = crate::tile_matrix::projection_data(&self.unwrapped, frame, how);
        TileProjection::of(&self.unwrapped, frame.camera, &data, Some(self.id))
    }
}

/// Where a tile is in the world's pixels at the camera's integer zoom: the
/// origin a pattern repeats from, so that it meets itself across tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternOrigin {
    /// The high 16 bits of the tile's north-west corner, x and y, which a
    /// shader adds to [`lower`](Self::lower) at a precision a float of
    /// the whole coordinate lacks.
    pub upper: [f32; 2],
    /// The low 16 bits of the corner.
    pub lower: [f32; 2],
    /// The pixels a tile unit spans at that zoom.
    pub tile_units_to_pixels: f32,
}

impl FrameTile {
    /// Where the tile is in `frame`'s world pixels, which its patterns
    /// repeat from.
    #[must_use]
    pub fn pattern_origin(self, frame: &Frame<'_>) -> PatternOrigin {
        let (id, zoom) = (self.unwrapped, frame.camera.integer_zoom());
        let scale = |z: f64| mln_map_types::zoom_scale(z);
        let tile_size = (TILE_SIZE_D * scale(f64::from(zoom) - f64::from(id.canonical.z))) as i32;
        let x = (f64::from(tile_size)
            * (f64::from(id.canonical.x) + f64::from(id.wrap) * scale(f64::from(id.canonical.z))))
            as i32;
        let y = tile_size.wrapping_mul(id.canonical.y as i32);
        let pixels_to_tile_units = id.pixels_to_tile_units(1.0, f32::from(zoom));
        PatternOrigin {
            upper: [(x >> 16) as f32, (y >> 16) as f32],
            lower: [(x & 0xFFFF) as f32, (y & 0xFFFF) as f32],
            tile_units_to_pixels: if pixels_to_tile_units == 0.0 { 0.0 } else { 1.0 / pixels_to_tile_units },
        }
    }
}

/// A tile's bucket of a layer of the style type `S`, with the layer's values
/// it was laid out with.
pub struct LayerBucket<'a, S: StyleLayerType, B> {
    /// The bucket.
    pub bucket: &'a B,
    /// The layer's values that the bucket was laid out with.
    pub laid_with: &'a EvaluatedLayer<S>,
    pub(crate) id: mln_util::identity::SimpleIdentity,
    pub(crate) contents: &'a GeometryContents,
}

impl<S: StyleLayerType, B> Clone for LayerBucket<'_, S, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: StyleLayerType, B> Copy for LayerBucket<'_, S, B> {}

impl<'a, S: StyleLayerType, B> LayerBucket<'a, S, B> {
    /// The buffer that `vec`, one of the bucket's, uploaded into
    /// ([`Upload::vec`]). Panics for a vector that the bucket's `upload`
    /// didn't name.
    #[must_use]
    pub fn buffer<V: bytemuck::Pod>(self, vec: &GpuVec<V>) -> &'a Buffer<V> {
        self.contents.bucket_geometry.buffer(vec).expect("a vector that the bucket's upload didn't name")
    }

    /// The index buffer that `indexes`, the bucket's, uploaded into
    /// ([`Upload::indexes`]). Panics for indexes that the bucket's `upload`
    /// didn't name.
    #[must_use]
    pub fn indexes(self, indexes: &Indexes) -> &'a IndexBuffer {
        self.contents.bucket_geometry.indexes(indexes).expect("indexes that the bucket's upload didn't name")
    }

    /// The layer's binders among `binders`, the bucket's, which
    /// [`BuildCx::binders`] made for each layer of the bucket's group, with
    /// the buffer their values uploaded into ([`Upload::binders`]).
    #[must_use]
    pub fn binders<P: PaintBinders>(self, binders: &'a LayerBinders<P>) -> PaintVertices<'a, P> {
        let of_layer = binders.of_layer(&self.laid_with.style.id).expect("binders of the bucket's layers");
        PaintVertices::new(of_layer, self.contents.bucket_geometry.paint(of_layer.vertices()))
    }

    /// The texture of the tile's atlas of glyphs, where the glyphs that its
    /// layout asked for are ([`Resolved::glyphs`]), once it uploaded.
    #[must_use]
    pub fn glyph_atlas(self) -> Option<&'a Texture> {
        self.contents.textures.glyph()
    }

    /// The texture of the tile's atlas of images, where the icons and
    /// patterns that its layout asked for are ([`Resolved::icons`],
    /// [`Resolved::patterns`]), once it uploaded.
    #[must_use]
    pub fn icon_atlas(self) -> Option<&'a Texture> {
        self.contents.textures.icon()
    }
}

/// A feature of a tile that a rendered-feature query tests a layer `L`
/// against: the query's geometry and the feature's, in the tile's units,
/// the feature's state, and the camera they are drawn with.
pub struct HitTest<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The query's geometry.
    pub query_geometry: &'a GeometryCoordinates,
    /// The feature, whose properties the layer's values evaluate with.
    pub feature: &'a dyn GeometryTileFeature,
    /// The feature's geometry.
    pub geometry: &'a GeometryCollection,
    /// The feature's state, which the layer's values evaluate with.
    pub feature_state: &'a FeatureState,
    /// The camera's zoom, not the tile's: a feature is as large as it is
    /// drawn.
    pub zoom: f32,
    /// The camera.
    pub camera: &'a View,
    /// The tile units a pixel spans.
    pub pixels_to_tile_units: f32,
    pub(crate) projector: &'a TileProjector<'a>,
}

impl<L: Layer> HitTest<'_, L> {
    /// `value`, one of the layer's evaluated values, for the feature as
    /// drawn: at the camera's zoom, with the feature's properties and
    /// state.
    #[must_use]
    pub fn feature_value<T: Clone + ValueConverter>(&self, value: &Evaluated<T>) -> T {
        value.evaluate(&EvaluationContext::with_zoom_feature_state(
            self.zoom,
            Some(self.feature),
            Some(self.feature_state),
        ))
    }

    /// The query's geometry moved against a layer's `*-translate` of
    /// `translate` pixels, anchored at `anchor`, as the layer's draws move
    /// its features; `None` where the translation is zero.
    #[must_use]
    pub fn translated_query_geometry(
        &self,
        translate: [f32; 2],
        anchor: TranslateAnchorType,
    ) -> Option<GeometryCoordinates> {
        FeatureIndex::translate_query_geometry(
            self.query_geometry,
            translate,
            anchor,
            self.camera.angle() as f32,
            self.pixels_to_tile_units,
        )
    }

    /// `point`, of the tile, on the screen, in the camera's pixels, where a
    /// layer whose features face the camera compares them with the query's
    /// geometry, brought there point by point too.
    #[must_use]
    pub fn to_screen(&self, point: GeometryCoordinate) -> GeometryCoordinate {
        let at = self.projector.project(&geometry::Point::new(f64::from(point.x), f64::from(point.y)), 0.0).point;
        let size = self.camera.size();
        // Scaled in `f32`, halved in `f64`.
        GeometryCoordinate::new(
            (f64::from((at.x as f32 + 1.0) * size.width as f32) * 0.5) as i16,
            (f64::from((at.y as f32 + 1.0) * size.height as f32) * 0.5) as i16,
        )
    }

    /// The points of the feature's geometry, each with how far it is from
    /// the camera, but those that the globe's horizon hides where a draw
    /// placed as `how` says draws them.
    pub fn drawn_points(&self, how: &TileTransform) -> impl Iterator<Item = DrawnPoint> + '_ {
        let [dx, dy] = if how.viewport_pixels {
            [0.0, 0.0]
        } else {
            crate::tile_matrix::tile_unit_translation(self.projector.tile_id(), how.translate, how.anchor, self.camera)
        };
        let globe = self.camera.is_globe();
        self.geometry.iter().flatten().filter_map(move |&point| {
            let drawn = geometry::Point::new(f64::from(f32::from(point.x) + dx), f64::from(f32::from(point.y) + dy));
            if globe && self.projector.project_as_drawn(&drawn, 0.0).occluded {
                return None;
            }
            let at = geometry::Point::new(f64::from(point.x), f64::from(point.y));
            Some(DrawnPoint { point, distance: self.projector.project(&at, 0.0).signed_distance_from_camera })
        })
    }
}

/// A point of a feature that a layer draws ([`HitTest::drawn_points`]).
#[derive(Clone, Copy, Debug)]
pub struct DrawnPoint {
    /// The point, in its tile's units.
    pub point: GeometryCoordinate,
    /// How far the point is from the camera, in pixels, as
    /// [`View::camera_to_center_distance`] is the view's center's.
    pub distance: f64,
}

/// A rendered-feature query of a layer `L`, for what it draws without tile
/// features ([`Layer::hit_frame`]).
pub struct FrameQuery<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The camera of the last frame.
    pub camera: &'a View,
    /// The query's geometry in the camera's logical pixels: one point, or
    /// a closed ring.
    pub geometry: &'a [ScreenCoordinate],
}

/// A feature a query hits, and how far from the query: in pixels, 0 inside
/// it. A layer's hits come nearest first, in the order found among equals.
#[derive(Clone, Debug)]
pub struct Hit {
    /// The feature. The engine names its layer.
    pub feature: mln_util::feature::Feature,
    /// The distance from the query, in pixels.
    pub distance: f32,
}

/// How a group of layers of the style type `S` that share their layout
/// becomes a tile's bucket, on a tile worker: [`new`](Self::new) selects the
/// tile's features, and once the glyphs and images it asked for are
/// resolved, [`build`](Self::build) makes the bucket.
pub trait Layout<S: StyleLayerType>: Send + Sized + 'static {
    /// The bucket the layout makes.
    type Bucket: Bucket;

    /// Whether the layout of `group` may ask for glyphs or images. The
    /// engine makes such a layout before it resolves them, and every other
    /// once they are.
    fn needs_resources(_group: &[Arc<EvaluatedLayer<S>>]) -> bool {
        false
    }

    /// Selects the features of the tile that the bucket draws.
    fn new(cx: &mut LayoutCx<'_, S>) -> Result<Self, Error>;

    /// Makes the bucket of the selected features; `None` for one with
    /// nothing to draw. A feature that fails to decode fails the bucket,
    /// which the engine reports as the tile's error.
    fn build(self, cx: &mut BuildCx<'_, S>) -> Result<Option<Self::Bucket>, Error>;
}

/// What a layout reads of its tile while it selects features.
pub struct LayoutCx<'a, S: StyleLayerType> {
    pub(crate) layers: &'a [Arc<EvaluatedLayer<S>>],
    pub(crate) tile: OverscaledTileID,
    pub(crate) mode: MapMode,
    pub(crate) pixel_ratio: f32,
    pub(crate) parameters: PropertyEvaluationParameters,
    pub(crate) font_faces: Option<&'a Arc<FontFaces>>,
    pub(crate) glyph_pbfs: bool,
    pub(crate) source_layer: TileLayer<'a>,
    pub(crate) filter_groups: &'a mut FilterGroups,
    pub(crate) glyphs: &'a mut GlyphDependencies,
    pub(crate) images: &'a mut ImageDependencies,
    pub(crate) available_images: &'a VecSet<String>,
    pub(crate) unavailable_images: &'a UnavailableImages,
}

impl<S: StyleLayerType> LayoutCx<'_, S> {
    /// The layers of the group, its leader first, whose style the layout's
    /// layout properties and filter are.
    #[must_use]
    pub fn layers(&self) -> &[Arc<EvaluatedLayer<S>>] {
        self.layers
    }

    /// The tile.
    #[must_use]
    pub fn tile(&self) -> OverscaledTileID {
        self.tile
    }

    /// How the map renders.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.mode
    }

    /// The device pixels per logical pixel that the map draws at.
    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// What the layout properties evaluate with: the tile's zoom and the
    /// style's global state.
    #[must_use]
    pub fn parameters(&self) -> &PropertyEvaluationParameters {
        &self.parameters
    }

    /// The style's font faces, which draw text in the scripts of their
    /// ranges, and its fallback faces after them.
    #[must_use]
    pub fn font_faces(&self) -> Option<&Arc<FontFaces>> {
        self.font_faces
    }

    /// Whether the style has glyphs of its own, a `glyphs` URL, which draw
    /// the text that no font face does.
    #[must_use]
    pub fn has_glyphs(&self) -> bool {
        self.glyph_pbfs
    }

    /// The indexes of the features that the leader's filter selects, in
    /// feature order.
    pub fn selected(&mut self) -> Result<Vec<usize>, Error> {
        crate::layout::layout::select(self)
    }

    /// The feature at `index` of the tile's source layer.
    pub fn feature(&self, index: usize) -> Result<Feature<'_>, Error> {
        Ok(self.source_layer.feature(index)?)
    }

    /// What a layout property evaluates with for `feature`: the tile's zoom
    /// and id, the style's global state, images and font faces. An image
    /// that an `image` expression finds missing is asked for as an icon.
    #[must_use]
    pub fn context<'f>(&'f self, feature: &'f dyn GeometryTileFeature) -> EvaluationContext<'f> {
        EvaluationContext::with_zoom_feature(f32::from(self.tile.overscaled_z), Some(feature))
            .with_canonical_tile_id(Some(&self.tile.canonical))
            .with_global_state(self.parameters.global_state.as_deref())
            .with_available_images(Some(self.available_images))
            .with_unavailable_images(Some(self.unavailable_images))
            .with_font_faces(self.font_faces.map(|faces| &**faces))
    }

    /// Asks for the style's image `id`, as `kind`. A layout of a type whose
    /// [`needs_resources`](Layout::needs_resources) builds once what it
    /// asked for resolved ([`BuildCx::resolved`]).
    pub fn need_image(&mut self, id: &str, kind: ImageType) {
        self.images.entry(id.to_owned()).or_insert(kind);
    }

    /// Asks for `glyphs` of the font stack `font_stack`, as
    /// [`need_image`](Self::need_image) asks for an image.
    pub fn need_glyphs(&mut self, font_stack: FontStack, glyphs: impl IntoIterator<Item = GlyphID>) {
        self.glyphs.glyphs.entry(font_stack).or_default().extend(glyphs);
    }

    /// Asks for the shaping of `text` by the font face `face` of the font
    /// stack `font_stack`, which [`Resolved::shaping`] then holds, as
    /// [`need_image`](Self::need_image) asks for an image.
    pub fn need_shaping(&mut self, font_stack: &FontStack, face: GlyphIDType, text: &[char]) {
        let texts = self.glyphs.shapes.entry(font_stack.clone()).or_default().entry(face).or_default();
        if !texts.contains(text) {
            texts.insert(text.to_vec());
        }
    }
}

/// What a layout reads and writes while it builds its bucket.
pub struct BuildCx<'a, S: StyleLayerType> {
    pub(crate) layers: &'a [Arc<EvaluatedLayer<S>>],
    pub(crate) tile: OverscaledTileID,
    pub(crate) mode: MapMode,
    pub(crate) first_load: bool,
    pub(crate) collision_boxes: bool,
    pub(crate) source_layer: TileLayer<'a>,
    pub(crate) resolved: Option<&'a Resolved>,
    pub(crate) feature_index: &'a mut FeatureIndexBuilder,
}

impl<S: StyleLayerType> BuildCx<'_, S> {
    /// The layers of the group, its leader first.
    #[must_use]
    pub fn layers(&self) -> &[Arc<EvaluatedLayer<S>>] {
        self.layers
    }

    /// The tile.
    #[must_use]
    pub fn tile(&self) -> CanonicalTileID {
        self.tile.canonical
    }

    /// How the map renders.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.mode
    }

    /// Whether the tile had no layout before this one.
    #[must_use]
    pub fn first_load(&self) -> bool {
        self.first_load
    }

    /// Whether the map's debug options show the boxes that placed features
    /// collide with.
    #[must_use]
    pub fn collision_boxes(&self) -> bool {
        self.collision_boxes
    }

    /// What the glyphs and images the layout asked for resolved to; `None`
    /// for a layout that asked for none.
    #[must_use]
    pub fn resolved(&self) -> Option<&Resolved> {
        self.resolved
    }

    /// The feature at `index` of the tile's source layer.
    pub fn feature(&self, index: usize) -> Result<Feature<'_>, Error> {
        Ok(self.source_layer.feature(index)?)
    }

    /// Binders of type `P` for each layer of the group, of its evaluated
    /// paint values at the tile's zoom.
    #[must_use]
    pub fn binders<P: PaintBinders<Paint = <S::PaintProperties as PaintSet>::Evaluated>>(&self) -> LayerBinders<P> {
        LayerBinders::new(self.layers, f32::from(self.tile.overscaled_z))
    }

    /// Writes the values of `feature`, the feature at `index`, into
    /// `binders` for the vertices up to `vertices`, those added since the
    /// last call.
    pub fn populate<P: PaintBinders>(
        &self,
        binders: &mut LayerBinders<P>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        vertices: usize,
    ) {
        binders.populate(feature, index, vertices, &self.tile.canonical, &Value::Null);
    }

    /// Writes the values of `feature`, as [`populate`](Self::populate) does,
    /// for vertices that draw `section`, a section of its formatted text,
    /// whose options a property's `["get", ..]` of the section reads.
    pub fn populate_section<P: PaintBinders>(
        &self,
        binders: &mut LayerBinders<P>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        vertices: usize,
        section: &Value,
    ) {
        binders.populate(feature, index, vertices, &self.tile.canonical, section);
    }

    /// Adds `geometry`, the geometry of the feature at `index`, to the
    /// tile's feature index, which queries search.
    pub fn index(&mut self, index: usize, geometry: &GeometryCollection) {
        let leader = &self.layers[0].style;
        self.feature_index.insert(geometry, index, &leader.source_layer, &leader.id);
    }
}

/// What a layout makes of a tile for a group of layers: the vertices,
/// indexes and paint values that their draws read.
///
/// The engine moves a bucket from the worker to the renderer's thread,
/// copies it for the tile's other world copies, and uploads it.
pub trait Bucket: Clone + Send + Wire + 'static {
    /// Names the bucket's [`GpuVec`]s, [`Indexes`] and [`LayerBinders`] to
    /// `up`: the
    /// engine uploads them into buffers once the bucket reaches the
    /// renderer's thread, and again where feature state rewrites the
    /// binders' values.
    fn upload(&mut self, up: &mut Upload<'_>);

    /// How far above the ground, in meters, the bucket's highest content
    /// is raised. The tiles that cover the view reach that high.
    fn max_elevation(&self) -> f32 {
        0.0
    }
}

/// `layer`'s source kind, which a layer type's style half and its input
/// must agree on.
pub(crate) const fn input_source<L: Layer>() -> SourceKind {
    <L::Input as input::Find<L::Style>>::SOURCE
}
