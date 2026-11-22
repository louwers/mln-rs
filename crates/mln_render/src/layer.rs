//! The layer contract: what a crate implements to add a layer type that the
//! renderer draws.
//!
//! A layer type has two halves. Its style half is a marker type that
//! implements `mln_style`'s [`StyleLayerType`]: its name, the kind of source
//! it draws, its properties, which [`mln_style::properties!`] declares, and
//! its [`Data`](StyleLayerType::Data). Its render half is a struct that
//! implements [`Layer`], one value per style layer of the type. A
//! [`RegistryBuilder`](crate::RegistryBuilder) registers both with
//! [`add_layer_type::<L>()`](crate::RegistryBuilder::add_layer_type).
//!
//! The engine keeps each layer's values ([`LayerValues`]), transitions and
//! evaluates them, and asks the layer what the evaluation means for its
//! drawing ([`Layer::evaluate`]). In each frame that draws the layer it runs
//! [`Layer::update`], then [`Layer::tile`] for each of the frame's tiles that
//! has data for the layer and no value of it yet, then [`Layer::prepare`],
//! which pushes the frame's draws ([`Draws`]). The engine sorts, uploads and
//! draws them: the layer never sees a pass, a depth range, a stencil value
//! or a uniform buffer.
//!
//! # Tiles
//!
//! A layer's [`Input`](Layer::Input) says what it draws from:
//!
//! - [`Features<L>`]: the tiles of a geometry source, whose features the
//!   layout `L` turns into a [`Bucket`] per tile, on a tile worker.
//! - [`ViewCover`]: no source; the tiles that cover the view at its integer
//!   zoom, with nothing in them.
//!
//! The layer keeps a [`Tile`](Layer::Tile) value per frame tile, typically
//! the meshes it draws the tile with. The engine holds the values: it keeps
//! each with its tile while the tile stays in the frame, moves it when the
//! map wraps around the antimeridian, and drops it when the tile leaves the
//! frame or its data is replaced. It drops them all when the layer is
//! restyled, when the map switches between Mercator and the globe, and when
//! the layer stops rendering. A bucket that the engine rewrites in place,
//! for feature state, keeps its value: the meshes over its buffers see the
//! new data.
//!
//! # Buckets
//!
//! A [`Layout`] runs on a tile worker, from owned inputs to an owned
//! [`Bucket`]. It selects the tile's features, then builds the bucket:
//! vertices and indexes in [`GpuVec`]s, and the values of the data-driven
//! paint properties in [`LayerBinders`], which a struct that
//! [`paint_property_binders!`](crate::paint_property_binders!) declares
//! writes per feature. The engine sends the bucket to the renderer's thread,
//! through the worker codec where the worker has its own memory (so a
//! bucket implements [`Wire`]: `#[derive(Wire)]`, with
//! `#[wire(crate = mln_rs::mln_wire)]` through the facade), and uploads it
//! ([`Bucket::upload`]). A tile's [`LayerBucket`] then gives the bucket's
//! [`Buffer`](crate::draw::Buffer)s to the meshes the layer makes.
//!
//! # Threads
//!
//! The layer, its tile values and every hook but the layout's run on the
//! renderer's thread. The layout and the bucket are `Send`.
//!
//! # Queries
//!
//! A rendered-feature query finds a tile's features near the query's
//! geometry in the tile's feature index, padded by the layer's
//! [`query_radius`](Layer::query_radius), and keeps those the layer
//! [`hit`](Layer::hit)s, nearest first.
//!
//! # Not built yet
//!
//! Inputs of raster and raster-dem tiles, layers without tiles, the
//! overlay group, groups drawn into textures of the layer's own and lends
//! of the encoder to a host are parts of this contract that no layer type
//! uses yet; they are left out until one does.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_map_types::{MapMode, TileProjector, View};
use mln_style::PropertyEvaluationParameters;
use mln_style::expression::expr::FeatureState;
use mln_style::layer_type::{LayerStyle, PaintSet, SourceKind, StyleLayerType};
use mln_style_types::TranslateAnchorType;
use mln_tile::{Feature, TileLayer};
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::error::Error;
use mln_wire::Wire;

pub use crate::bucket_geometry::GpuVec;
pub use crate::bucket_upload::Upload;
use crate::draw::{Buffer, Draws, Resources};
pub use crate::erased_layer::EvaluatedLayer;
use crate::geometry::feature_index::{FeatureIndex, FeatureIndexBuilder};
pub use crate::gfx::mesh::Indexed;
pub use crate::image_store::ImageStore;
use crate::layout::filter_groups::FilterGroups;
pub use crate::layout::tile_layout::Resolved;
pub use crate::paint_parameters::Frame;
pub use crate::paint_property_binder::{
    CrossFadedPaintPropertyBinder, LayerBinders, PaintBinders, PaintPropertyBinder, PaintVertices, PatternDependency,
};
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
    /// is brought up to date here.
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
    /// still hits it with the layer's values `layer`. A tile's query
    /// geometry grows by its layers' largest radius.
    fn query_radius(&self, _layer: &LayerValues<Self::Style>, _data: TileData<'_, Self>) -> f32 {
        0.0
    }

    /// Whether `test`'s feature, as the layer draws it, is hit by the
    /// query's geometry: its distance in pixels from the query, 0 inside
    /// it, or `None` for a miss.
    fn hit(&self, _test: &HitTest<'_, Self>) -> Option<f32> {
        None
    }
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
    use mln_style::layer_type::{SourceKind, StyleLayerType};
    use mln_util::identity::SimpleIdentity;

    use super::{Features, Layout, TileInput, ViewCover};
    use crate::render_tile::{TileContent, ViewPlace};
    use crate::tile::contents::GeometryContents;

    /// The engine's side of a [`TileInput`]: where a tile holds the input's
    /// data. It seals [`TileInput`].
    pub(crate) trait Find<S: StyleLayerType>: crate::erased_layer::LayoutOf<S> {
        /// The kind of source whose tiles hold the input.
        const SOURCE: SourceKind;

        /// What `content` holds for the layer `layer_id`, if anything, and
        /// where it is in it.
        fn find<'a>(content: TileContent<'a>, layer_id: &str) -> Option<(Self::Data<'a>, ViewPlace)>
        where
            Self: TileInput<S>;

        /// What `content` holds at `place`, where [`find`](Self::find)
        /// found what it holds for a layer.
        fn at<'a>(content: TileContent<'a>, place: ViewPlace) -> Option<Self::Data<'a>>
        where
            Self: TileInput<S>;

        /// What `contents`, a tile's layout, holds for the layer `layer_id`.
        fn laid<'a>(_contents: &'a GeometryContents, _layer_id: &str) -> Option<Self::Data<'a>>
        where
            Self: TileInput<S>,
        {
            None
        }

        /// What a tile of the view's cover holds; `None` for an input of a
        /// source's tiles.
        fn cover<'a>() -> Option<Self::Data<'a>>
        where
            Self: TileInput<S>,
        {
            None
        }

        /// The identity of the bucket or contents that `data` shows the
        /// layer to draw, which the layer's value of the tile is made of;
        /// `None` where it shows nothing to draw.
        fn identity(data: Self::Data<'_>) -> Option<SimpleIdentity>
        where
            Self: TileInput<S>;
    }

    impl<S: StyleLayerType, L: Layout<S>> Find<S> for Features<L> {
        const SOURCE: SourceKind = SourceKind::Geometry;

        fn find<'a>(content: TileContent<'a>, layer_id: &str) -> Option<(<Self as TileInput<S>>::Data<'a>, ViewPlace)> {
            let TileContent::Tile(contents) = content else { return None };
            let contents = contents.geometry()?;
            let index = contents.result.layer_render_data.find(layer_id)?;
            Some((crate::erased_layer::laid::<S, L>(contents, index)?, ViewPlace::Laid(index)))
        }

        fn laid<'a>(contents: &'a GeometryContents, layer_id: &str) -> Option<<Self as TileInput<S>>::Data<'a>> {
            let index = contents.result.layer_render_data.find(layer_id)?;
            crate::erased_layer::laid::<S, L>(contents, index)
        }

        fn at<'a>(content: TileContent<'a>, place: ViewPlace) -> Option<<Self as TileInput<S>>::Data<'a>> {
            let ViewPlace::Laid(index) = place else {
                debug_assert!(false, "a tile's own data's place for a laid-out layer");
                return None;
            };
            let TileContent::Tile(contents) = content else { return None };
            crate::erased_layer::laid::<S, L>(contents.geometry()?, index)
        }

        fn identity(data: <Self as TileInput<S>>::Data<'_>) -> Option<SimpleIdentity> {
            data.laid_with.draws.then_some(data.id)
        }
    }

    impl<S: StyleLayerType> Find<S> for ViewCover {
        const SOURCE: SourceKind = SourceKind::None;

        fn find<'a>(
            _content: TileContent<'a>,
            _layer_id: &str,
        ) -> Option<(<Self as TileInput<S>>::Data<'a>, ViewPlace)> {
            None
        }

        fn at<'a>(_content: TileContent<'a>, _place: ViewPlace) -> Option<<Self as TileInput<S>>::Data<'a>> {
            None
        }

        fn cover<'a>() -> Option<<Self as TileInput<S>>::Data<'a>> {
            Some(())
        }

        fn identity(_data: <Self as TileInput<S>>::Data<'_>) -> Option<SimpleIdentity> {
            None
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
    /// Whether the layer may push opaque draws
    /// ([`Draw::opaque`](crate::draw::Draw::opaque)), which the frame draws
    /// front to back before it blends the rest.
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
        if input_source::<L>() == SourceKind::None {
            return <L::Input as input::Find<L::Style>>::cover();
        }
        let (content, place) = self.at?;
        let data = <L::Input as input::Find<L::Style>>::at(content, place)?;
        <L::Input as input::Find<L::Style>>::identity(data).is_some().then_some(data)
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
        (found.tile, found.data::<L>(), self.values.get_mut(i))
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
        Some((found.tile, found.data::<L>()?, self.values.get(i)?))
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
        TileProjection::new(&self.unwrapped, frame, how, Some(self.id))
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
    /// ([`Upload::vec`]). Panics for a vector the bucket didn't upload.
    #[must_use]
    pub fn buffer<V: bytemuck::Pod>(self, vec: &GpuVec<V>) -> &'a Buffer<V> {
        self.contents.bucket_geometry.buffer(vec).expect("a vector that its bucket's `upload` didn't name")
    }

    /// The binders of the layer that the bucket was laid out with among
    /// `binders`, the bucket's, with the buffer of their values. Panics
    /// for binders that the bucket didn't upload ([`Upload::binders`]).
    #[must_use]
    pub fn binders<P: PaintBinders>(self, binders: &'a LayerBinders<P>) -> PaintVertices<'a, P> {
        let of_layer = binders.of_layer(&self.laid_with.style.id);
        PaintVertices::new(of_layer, self.contents.bucket_geometry.paint(of_layer.vertices()))
    }

    /// The tile's feature index, which queries search.
    #[must_use]
    pub(crate) fn feature_index(self) -> &'a Arc<FeatureIndex> {
        &self.contents.result.feature_index
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
    /// What projects the tile's units onto the screen.
    pub projector: &'a TileProjector<'a>,
}

impl<L: Layer> HitTest<'_, L> {
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

    /// The translation in the tile's units that a draw moves the tile's
    /// geometry by for a `*-translate` of `translate` pixels anchored at
    /// `anchor`, as [`TileProjection::translation`] holds it.
    #[must_use]
    pub fn translation(&self, translate: [f32; 2], anchor: TranslateAnchorType) -> [f32; 2] {
        crate::tile_matrix::tile_unit_translation(self.projector.tile_id(), translate, anchor, self.camera)
    }
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
    pub(crate) parameters: PropertyEvaluationParameters,
    pub(crate) source_layer: TileLayer<'a>,
    pub(crate) filter_groups: &'a mut FilterGroups,
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

    /// What the layout properties evaluate with: the tile's zoom and the
    /// style's global state.
    #[must_use]
    pub fn parameters(&self) -> &PropertyEvaluationParameters {
        &self.parameters
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
}

/// What a layout reads and writes while it builds its bucket.
pub struct BuildCx<'a, S: StyleLayerType> {
    pub(crate) layers: &'a [Arc<EvaluatedLayer<S>>],
    pub(crate) tile: OverscaledTileID,
    pub(crate) mode: MapMode,
    pub(crate) first_load: bool,
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
        binders.populate(feature, index, vertices, &self.tile.canonical);
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
    /// Names the bucket's [`GpuVec`]s and [`LayerBinders`] to `up`: the
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
