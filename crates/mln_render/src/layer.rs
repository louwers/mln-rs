//! The layer type contract: what a crate implements to add a layer type
//! that the renderer draws.
//!
//! A layer type is a marker type that implements `mln_style`'s
//! `StyleLayerType`, its style half, and [`LayerType`], its render half. A
//! [`RegistryBuilder`](crate::RegistryBuilder) registers it by name with
//! [`add_layer_type`](crate::RegistryBuilder::add_layer_type), with the
//! sources of its programs ([`LayerType::add_shader_sources`]), which fails
//! for a name that another type took. Every registry that reads a style of
//! the type registers it, tile workers' too.
//!
//! The engine holds each layer of the type: its values ([`LayerValues`]),
//! which it transitions and evaluates, the passes it draws in, whether the
//! frame draws it, and the layer's [`Drawing`](LayerType::Drawing) and
//! [`State`](LayerType::State), which the layer type keeps. Each frame, the
//! engine evaluates the layer ([`LayerType::passes`]), finds its tiles and
//! runs [`LayerType::update`] with an [`UpdateContext`], and in the encode
//! runs [`LayerType::make_draws`] with a [`DrawContext`]. The engine then
//! uploads and draws the drawing's draws.
//!
//! The style half declares the type's properties with
//! [`mln_style::properties!`]. A bucket binds the data-driven paint
//! properties to its programs' vertex attributes with a struct that
//! [`paint_property_binders!`](crate::paint_property_binders!) declares,
//! with a [`PaintPropertyBinder`] or a [`CrossFadedPaintPropertyBinder`]
//! for each property.
//!
//! A layer type that lays out makes a [`Bucket`] of each layer's features
//! in a tile, which the engine moves between threads, uploads, rewrites for
//! feature state and copies for the tile's other world copies; a type that
//! doesn't lay out has [`NoBucket`] and [`NoLayout`]. Its [`Layout`], which
//! [`LayerType::create_layout`] makes of a group of layers with the tile's
//! [`LayoutParameters`] and [`SourceLayer`], selects the features and
//! records the images they need, and once those are [`Resolved`], makes the
//! bucket, keeping its geometry in [`GeometryVec`]s, indexed in segments of
//! a draw each ([`Indexed`]), and adds the features to the tile's
//! [`FeatureIndexBuilder`].
//!
//! A layer's tiles are those of its source, each as a [`FrameTile`] with
//! what it shows the layer, the type's [`TileView`]: its bucket in the
//! tile's layout ([`LayerBucket`]), a raster tile's image or an image
//! source's image ([`RasterView`] of [`RasterContents`]), or a raster-dem
//! tile's DEM ([`DemContents`]). The engine masks a raster or raster-dem
//! tile where better tiles cover it, and keeps the geometry that draws the
//! rest ([`MaskedGeometry`]). A layer type without a source gets no tiles.
//!
//! What only some layer types do, a type declares as a capability: it
//! draws 3D geometry ([`LayerType::IS_3D`]), clears the frame to a solid
//! color ([`LayerType::solid_background`]), answers feature queries
//! ([`FeatureQuery`]), draws into textures of its own before the frame's
//! passes ([`OffscreenPass`]), or draws through a host outside the
//! renderer ([`LayerHost`]). The engine asks for each, and does without it
//! for a type that has none.
//!
//! [`LayerType::add_shader_sources`] runs once, when the type registers,
//! on the thread that registers it. The engine runs the other hooks one at
//! a time on the renderer's thread, but for a tile's layout
//! ([`LayerType::layout_can_need_dependencies`],
//! [`LayerType::create_layout`] and the [`Layout`]), which runs with the
//! tile's other work on the thread that runs the tile's job: another
//! thread, or the renderer's thread for a source whose updates are
//! synchronous, when no worker could start, or when the host drains the
//! renderer's pool there. So the layout and the bucket are `Send`. A worker
//! that shares the renderer's memory hands the bucket back as it is. One
//! that doesn't, such as a browser's Web Worker with its own wasm instance,
//! gets the layer's values and sends the bucket back through the worker
//! codec, so the bucket and the style half's property sets implement
//! [`mln_wire::Wire`]. `#[derive(Wire)]` writes the impl for a struct or an
//! enum whose fields implement it; a crate that reaches `mln_wire` through
//! the `mln_rs` facade names it with `#[wire(crate = mln_rs::mln_wire)]`.
//! The codec only runs between instances of one build, so its encoding
//! needn't stay the same from one release to the next. A layer's
//! [`State`](LayerType::State) and [`Drawing`](LayerType::Drawing) stay on
//! the renderer's thread.
//!
//! The types a layer type draws with, which the hooks name, are the
//! [draw services](crate::draw), and the geometry that drawing on the globe
//! needs is in [`globe`](crate::globe).

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::source::ShaderSources;
use mln_map_types::{TileProjector, View};
use mln_style::PropertyEvaluationParameters;
use mln_style::expression::expr::FeatureState;
use mln_style::layer_type::{LayerStyle, PaintSet, StyleLayerType};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_util::Color;
use mln_util::error::Error;
use mln_wire::Wire;

pub use crate::bucket::{Bucket, BucketBase, BucketValue, FeatureStates, NoBucket};
pub use crate::bucket_geometry::{GeometryUpload, GeometryVec};
pub use crate::bucket_parameters::BucketParameters;
pub use crate::erased_layer::EvaluatedLayer;
pub use crate::geometry::dem_data::DemData;
pub use crate::geometry::feature_index::{FeatureIndex, FeatureIndexBuilder};
use crate::gfx::context::Context;
pub use crate::gfx::mesh::Indexed;
pub use crate::image_store::ImageStore;
use crate::layer_drawing::LayerDrawing;
pub use crate::layout::filter_groups::FilterGroups;
pub use crate::layout::layout::{BuildParams, Layout, LayoutParameters, NoLayout, SourceLayer, layers_by_id};
pub use crate::layout::tile_layout::Resolved;
use crate::paint_parameters::{DrawFrame, PaintParameters};
pub use crate::paint_property_binder::{
    BinderValue, CrossFadedPaintPropertyBinder, InterleavedVertexBuffer, PaintPropertyBinder, PaintPropertyBinderBase,
    PatternDependency, bind_paint_property,
};
pub use crate::paint_property_statistics::StatisticValue;
use crate::placement::{BucketKey, NewPlacement};
pub use crate::render_layer::LayerBucket;
pub use crate::render_pass::RenderPass;
pub use crate::render_source::TileOrder;
use crate::render_tile::FindView as _;
pub use crate::render_tile::{FrameTile, NoTileView, RasterView, TileView};
pub use crate::text::glyph_store::ShapeResults;
pub use crate::tile::dem::DemContents;
use crate::tile::job::SourceKey;
pub use crate::tile::raster::{MaskedGeometry, RasterContents, RasterLayoutVertex, StoredQuads};
pub use crate::tile_mask::TileMask;
use crate::update_parameters::UpdateParameters;

/// The render half of a layer type, whose style half is its
/// [`StyleLayerType`]: how a tile lays out the type's layers, what a layer
/// of the type keeps, and how it draws.
///
/// The hooks are associated functions that get the layer's values, what
/// the type keeps of the layer, and a context of the frame; the engine does
/// everything around them that is the same for every type.
pub trait LayerType: StyleLayerType {
    /// How a tile lays out a group of the type's layers.
    /// [`NoLayout`] for a type that doesn't lay out.
    type Layout: Layout<Self>;

    /// The bucket the layout makes, which crosses the worker codec
    /// ([`Wire`]). [`NoBucket`] for a type that doesn't lay out.
    type Bucket: Bucket + Wire;

    /// What a frame's tile shows a layer of the type, which its draws draw:
    /// its bucket in the tile's layout, a raster or raster-dem tile's own
    /// data or an image source's image, or nothing for a type without a
    /// source ([`NoTileView`]).
    type View<'a>: TileView<'a>;

    /// What a layer of the type keeps of its draws: their GPU objects and
    /// its draw groups of the frame, which the engine uploads and draws.
    type Drawing: LayerDrawing;

    /// What else a layer of the type keeps from frame to frame, such as a
    /// color ramp or the textures of its tiles.
    type State;

    /// Whether the type has cross-faded properties, whose evaluation takes
    /// the frame's cross-fade.
    const CROSSFADES: bool = false;

    /// The order a layer of the type gets its source's tiles in.
    const TILE_ORDER: TileOrder = TileOrder::ById;

    /// Adds the sources of the programs the type draws with, in each
    /// shading language it has them in, which a renderer's backend compiles
    /// the programs from. [`add_layer_type`](crate::RegistryBuilder::add_layer_type)
    /// calls it.
    fn add_shader_sources(_sources: &mut ShaderSources) {}

    /// Whether the layout of `group` can need glyphs or images. A tile
    /// makes such a group's layout before the dependencies are resolved,
    /// and every other group's once they are.
    fn layout_can_need_dependencies(_group: &[Arc<EvaluatedLayer<Self>>]) -> bool {
        false
    }

    /// The layout of `group`, layers of the same layout on `source_layer`,
    /// with the dependencies it records in `parameters`. `None` for a type
    /// that doesn't lay out.
    fn create_layout(
        _group: &[Arc<EvaluatedLayer<Self>>],
        _parameters: &mut LayoutParameters<'_>,
        _source_layer: SourceLayer,
    ) -> Result<Option<Self::Layout>, Error> {
        Ok(None)
    }

    /// What a new layer with `layer`'s values, before they transition,
    /// keeps.
    fn new_layer(layer: &LayerValues<Self>) -> (Self::State, Self::Drawing);

    /// The passes a layer of `layer`'s values draws in with `evaluated`, its
    /// values of a frame.
    fn passes(layer: &LayerValues<Self>, evaluated: &EvaluatedLayer<Self>) -> RenderPass;

    /// Called once a layer's values are evaluated for a frame of
    /// `parameters`, before they become the layer's: the layer derives the
    /// values of `evaluated` that its type derives from others, and remakes
    /// what it makes of them, such as a color ramp.
    fn complete_evaluation(
        _state: &mut Self::State,
        _layer: &LayerValues<Self>,
        _parameters: &PropertyEvaluationParameters,
        _evaluated: &mut EvaluatedLayer<Self>,
    ) {
    }

    /// The frame's update of a layer the frame draws, once the engine found
    /// its tiles and kept their entries in the drawing's tile map. The
    /// layer prepares in its state what its draws need, such as the images
    /// it packs; a layer without a source picks the tiles it keeps.
    fn update(_state: &mut Self::State, _drawing: &mut Self::Drawing, _cx: &mut UpdateContext<'_, '_, Self>) {}

    /// Makes a layer's draws of the frame, in the frame's encode, once the
    /// engine started them ([`LayerDrawing::begin_frame`]): from the
    /// context's tiles, for a layer that draws in a pass and whose source
    /// shows it tiles, or a layer without a source that keeps tiles in its
    /// drawing and picked some. The layer keeps in its drawing what it makes
    /// of them, with the values of their uniforms.
    fn make_draws(
        state: &mut Self::State,
        drawing: &mut Self::Drawing,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Self>,
    );

    /// Called in the encode when the frame draws in `variant`, another
    /// projection than the last, once the engine dropped the layer's draws,
    /// for the layer to drop what else it made for the last projection.
    fn projection_changed(_state: &mut Self::State, _variant: ProjectionVariant) {}

    /// Drops what a layer keeps for its draws besides the drawing, when it
    /// stops rendering.
    fn release(_state: &mut Self::State) {}

    /// How far above the ground `bucket`'s highest content is raised, for
    /// a layer whose content elevation
    /// ([`StyleLayerType::content_elevation`]) is an expression.
    fn max_content_elevation(_bucket: &Self::Bucket) -> f32 {
        0.0
    }

    // The capabilities.

    /// Whether a layer of the type draws 3D geometry, with draws of
    /// [`DrawDepth::Volume`](crate::draw::DrawDepth::Volume). The layers
    /// above the lowest 3D layer draw their opaque draws in the translucent
    /// pass, and on the globe, the planet's depth hides 3D geometry behind
    /// the horizon.
    const IS_3D: bool = false;

    /// The color a layer with `evaluated` is, where it is one solid color,
    /// which the frame can clear to instead of drawing the layer when the
    /// layer is the style's bottom one.
    fn solid_background(_evaluated: &EvaluatedLayer<Self>) -> Option<Color> {
        None
    }

    /// How the type's features answer rendered-feature queries; `None` for
    /// a type whose features no query hits.
    #[must_use]
    fn feature_query() -> Option<&'static dyn FeatureQuery<Self>> {
        None
    }

    /// What a layer draws into textures of its own before the frame's
    /// passes, out of what it keeps; `None` for a type that draws nothing
    /// there.
    fn offscreen_pass<'a>(
        _state: &'a mut Self::State,
        _drawing: &'a mut Self::Drawing,
    ) -> Option<&'a mut dyn OffscreenPass> {
        None
    }

    /// The host outside the renderer that a layer draws through, out of
    /// its drawing; `None` for a type whose layers draw themselves.
    fn host(_drawing: &mut Self::Drawing) -> Option<&mut dyn LayerHost<Self>> {
        None
    }

    /// Not a contract yet (see the crate doc): what makes the placement
    /// pass of the type's layers, for a type whose layers place what they
    /// show where it collides with nothing shown already
    /// ([`PlacementPass`](crate::placement::PlacementPass)); `None` for a
    /// type whose layers don't. A renderer makes one pass, of
    /// its registry's one type that places, and runs it over the frame's
    /// layers of the type. Placement changes such a layer's buckets where
    /// they are: a tile that fades out keeps drawing the layer, and a
    /// tile's copy for another world copy leaves its buckets out.
    #[doc(hidden)]
    const PLACEMENT: Option<NewPlacement<Self>> = None;
}

/// How the features of the layer type `T` answer rendered-feature queries:
/// the capability of a type whose features queries hit, which the type
/// returns from [`LayerType::feature_query`].
pub trait FeatureQuery<T: LayerType> {
    /// How far, in pixels, outside a feature of `bucket` a query still hits
    /// it, from `layer`'s values and the bucket's statistics of them. A
    /// tile's query geometry grows by its layers' largest radius.
    fn query_radius(&self, bucket: &T::Bucket, layer: &EvaluatedLayer<T>) -> f32;

    /// Whether `test`'s feature, as a layer with `layer`'s values draws it,
    /// intersects the query's geometry.
    fn intersects(&self, layer: &EvaluatedLayer<T>, test: &HitTest<'_>) -> bool;
}

/// A feature of a tile that a rendered-feature query tests a layer against:
/// the query's geometry and the feature's, in the tile's coordinates, the
/// feature's state, and the camera they are drawn with.
pub struct HitTest<'a> {
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
    pub view: &'a View,
    /// The tile units a pixel spans.
    pub pixels_to_tile_units: f32,
    /// What projects the tile's coordinates onto the screen.
    pub projector: &'a TileProjector<'a>,
}

impl HitTest<'_> {
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
            self.view.angle() as f32,
            self.pixels_to_tile_units,
        )
    }
}

/// What a layer draws into textures of its own before the frame's passes,
/// such as a render target that its draws in the passes sample: the
/// capability of a type that does, whose layers return it from
/// [`LayerType::offscreen_pass`]. The engine runs it for a layer the frame
/// draws.
pub trait OffscreenPass {
    /// Uploads the GPU objects of what the pass draws, after the layers'
    /// draws upload.
    fn upload(&mut self, _gpu: &mut dyn Gpu, _context: &mut Context) {}

    /// Draws through `gpu`, the context's, after the frame's uploads and
    /// before its first pass, with no pass open: what it draws into, it
    /// begins and ends a pass of its own for.
    fn render(&mut self, gpu: &mut dyn Gpu, paint: &mut PaintParameters<'_>);
}

/// A host that drew through a layer and that the layer let go of, which
/// hears that the renderer stopped drawing through it.
pub trait EndedHost: Send {
    /// Tells the host that the renderer stopped, once the GPU finished the
    /// host's work.
    fn end(self: Box<Self>);

    /// Tells the host that the renderer's graphics objects are lost, where
    /// the renderer goes without ending it.
    fn lose(self: Box<Self>);
}

/// The host outside the renderer that a layer of the type `T` draws
/// through, such as a custom layer's: the capability of a type whose layers
/// do, which returns it from [`LayerType::host`]. A host that drew hears
/// when the renderer stops drawing through it: once the GPU finished its
/// work, through the [`EndedHost`] that the layer returns, or at once when
/// the renderer's graphics objects are lost.
pub trait LayerHost<T: LayerType> {
    /// Called when the layer's style changed to `layer`'s, before its values
    /// transition. Where the style replaced the host, the layer takes the
    /// new one, and returns what ends the old one if it drew.
    fn restyled(&mut self, layer: &LayerValues<T>) -> Option<Box<dyn EndedHost>>;

    /// Lets go of the host as the layer leaves the renderer, and returns
    /// what ends it if it drew.
    fn leave(&mut self) -> Option<Box<dyn EndedHost>>;

    /// Tells the host that the renderer's graphics objects are lost if it
    /// drew, and lets go of it.
    fn lose_context(&mut self);
}

/// A layer's values of its type `T`: its style, its paint values in
/// transition, and their evaluation for the last frame. The engine keeps
/// them and changes them; the hooks read them.
pub struct LayerValues<T: LayerType> {
    /// The layer's style.
    pub style: Arc<LayerStyle<T>>,
    /// The paint values, in transition.
    pub unevaluated: <T::PaintProperties as PaintSet>::Unevaluated,
    /// The paint values evaluated for the last frame.
    pub evaluated: Arc<EvaluatedLayer<T>>,
}

impl<T: LayerType> LayerValues<T> {
    /// The values of the layer `style`, before any transition.
    pub(crate) fn new(style: Arc<LayerStyle<T>>) -> Self {
        let unevaluated = style.paint.untransitioned();
        let evaluated = Arc::new(EvaluatedLayer::new(Arc::clone(&style)));
        LayerValues { style, unevaluated, evaluated }
    }

    /// The layer's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.style.id
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
}

/// What a layer's update reads of the frame.
pub struct UpdateContext<'a, 'f, T: LayerType> {
    /// The layer's tiles of the frame, in the order of its type's
    /// [`TILE_ORDER`](LayerType::TILE_ORDER), each with what it shows the
    /// layer; none for a layer without a source.
    pub tiles: &'a [FrameTile<'f, T>],
    /// The layer's values.
    pub layer: &'a LayerValues<T>,
    /// The frame's camera.
    pub view: &'a View,
    /// The update the frame draws.
    pub update_parameters: &'a UpdateParameters,
    /// The images of the style.
    pub images: &'a ImageStore,
    /// The tile store's tiles of the layer's source; `None` for a layer
    /// without a source, or a source without tiles.
    pub(crate) source_tiles: Option<SourceKey>,
    /// See [`source_max_zoom`](Self::source_max_zoom).
    pub(crate) source_max_zoom: Option<u8>,
}

impl<T: LayerType> UpdateContext<'_, '_, T> {
    /// Not a contract yet (see the crate doc): the key that the layer's
    /// placement finds `tile`'s bucket by among the frame's tiles
    /// ([`PlacementBuckets`](crate::placement::PlacementBuckets)), for a tile
    /// that shows the layer a bucket of its layout.
    #[doc(hidden)]
    #[must_use]
    pub fn bucket_key(&self, tile: FrameTile<'_, T>) -> Option<BucketKey> {
        Some(BucketKey {
            bucket: tile.view()?.bucket_index()?,
            slot: tile.tile.slot(),
            tile: tile.id(),
            source: self.source_tiles?,
        })
    }

    /// The deepest zoom that has tiles of its own in the layer's source's
    /// [`Cover`](crate::source::Cover), once it has one.
    #[must_use]
    pub fn source_max_zoom(&self) -> Option<u8> {
        self.source_max_zoom
    }
}

/// What a layer's draws read of the frame, and the renderer's GPU state that
/// keeps their objects.
pub struct DrawContext<'a, 'f, T: LayerType> {
    /// The layer's tiles of the frame, as its update got them, each with
    /// what it shows the layer; none for a layer without a source.
    pub tiles: &'a [FrameTile<'f, T>],
    /// The layer's values.
    pub layer: &'a LayerValues<T>,
    /// The passes the layer draws in: those of its values, and those its
    /// tiles' buckets were laid out to draw in.
    pub passes: RenderPass,
    /// The projection the layer's draws are built for.
    pub projection_variant: ProjectionVariant,
    /// The renderer's GPU state, whose stores keep the draws' objects.
    pub context: &'a mut Context,
    /// The frame the draws are made for.
    pub frame: DrawFrame<'a>,
}
