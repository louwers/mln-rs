//! The layer type contract: what a crate implements to add a layer type
//! that the renderer draws.
//!
//! A layer type is a marker type that implements `mln_style`'s
//! `StyleLayerType`, its style half, and [`LayerType`], its render half. A
//! [`RegistryBuilder`](crate::RegistryBuilder) registers it by name with
//! [`add_layer_type`](crate::RegistryBuilder::add_layer_type).
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
//! A layer's tiles are those of its source, each as a [`FrameTile`] with
//! what it shows the layer, the type's [`TileView`]: its bucket in the
//! tile's layout, a raster or raster-dem tile's own data, or an image
//! source's image. A layer type without a source gets no tiles.
//!
//! The drawing types that the hooks name are in the engine's unstable
//! module until they become part of the contract.

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_map_types::{TileProjector, View};
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::{LayerStyle, PaintSet, StyleLayerType};
use mln_util::Color;
use mln_util::error::Error;
use mln_wire::Wire;

use crate::bucket::Bucket;
use crate::gfx::context::Context;
use crate::image_store::ImageStore;
use crate::layer_drawing::LayerDrawing;
pub use crate::layer_type::EvaluatedLayer;
use crate::layers::render_custom_layer::EndedHosts;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{Atlases, BucketPlacementData};
pub use crate::render_pass::RenderPass;
use crate::render_source::RenderSource;
pub use crate::render_source::TileOrder;
pub use crate::render_tile::{FrameTile, NoTileBucket, TileContent, TileView, ViewPlace};
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
    /// [`NoLayout`](crate::unstable::NoLayout) for a type that doesn't lay
    /// out.
    type Layout: Layout<Self>;

    /// The bucket the layout makes.
    /// [`NoBucket`](crate::unstable::NoBucket) for a type that doesn't lay
    /// out.
    type Bucket: Bucket + Wire;

    /// What a frame's tile shows a layer of the type, which its draws draw:
    /// its bucket in the tile's layout, a raster or raster-dem tile's own
    /// data or an image source's image, or nothing for a type without a
    /// source ([`NoTileBucket`]).
    type TileBucket<'a>: TileView<'a>;

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

    /// Whether a layer of the type draws 3D geometry, whose depth it writes
    /// in the range of `PaintParameters::depth_mode_for_3d`, and which the
    /// planet's depth hides behind the horizon.
    const IS_3D: bool = false;

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

    /// How far outside a feature of `bucket` a query still hits it, from
    /// `layer`'s current values and the bucket's statistics of them.
    fn query_radius(_bucket: &Self::Bucket, _layer: &EvaluatedLayer<Self>) -> f32 {
        0.0
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
    /// layer adds what its draws need to the frame's atlases here; a layer
    /// without a source picks the tiles it keeps.
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
        cx: &mut DrawContext<'_, '_, '_, Self>,
    );

    /// Called in the encode when the frame draws in `variant`, another
    /// projection than the last, once the engine dropped the layer's draws,
    /// for the layer to drop what else it made for the last projection.
    fn projection_changed(_state: &mut Self::State, _variant: ProjectionVariant) {}

    /// Drops what a layer keeps for its draws besides the drawing, when it
    /// stops rendering.
    fn release(_state: &mut Self::State) {}

    /// Called when the engine drops a layer's draws because its source shows
    /// it nothing.
    fn nothing_to_draw(_state: &mut Self::State) {}

    /// Called when a layer's style changed to another of the same id and
    /// type, now `layer`'s, before its values transition. A custom layer
    /// whose host the style replaced puts the old host that drew in `ended`.
    fn style_changed(_drawing: &mut Self::Drawing, _layer: &LayerValues<Self>, _ended: &mut EndedHosts) {}

    /// A background layer's color, where it is a solid color the frame can
    /// clear to instead of drawing the layer.
    fn solid_background(_evaluated: &EvaluatedLayer<Self>) -> Option<Color> {
        None
    }

    /// Uploads what draws into a layer's render targets, which draw before
    /// the frame's passes, of a layer the frame draws.
    fn upload_render_targets(
        _state: &mut Self::State,
        _drawing: &mut Self::Drawing,
        _gpu: &mut dyn Gpu,
        _context: &mut Context,
    ) {
    }

    /// Draws a layer's render targets through `gpu`, the context's, ahead
    /// of the frame's passes, which sample them, of a layer the frame draws.
    fn render_targets(
        _state: &mut Self::State,
        _drawing: &mut Self::Drawing,
        _gpu: &mut dyn Gpu,
        _parameters: &mut PaintParameters<'_>,
    ) {
    }

    /// Runs a custom layer's prerender through `gpu`, the context's, after
    /// the layers' draws upload and before the frame's first pass.
    fn prerender(_drawing: &mut Self::Drawing, _gpu: &mut dyn Gpu, _parameters: &PaintParameters<'_>) {}

    /// Lets go of a custom layer's host as the layer leaves the renderer: a
    /// host that drew goes to `ended`, to hear that the renderer stopped.
    fn leave(_drawing: &mut Self::Drawing, _ended: &mut EndedHosts) {}

    /// Tells a custom layer's host that the renderer's graphics objects are
    /// lost, and lets go of it.
    fn lose_context(_drawing: &mut Self::Drawing) {}

    /// The buckets a layer places, which only a symbol layer has.
    fn placement_data(_state: &Self::State) -> &[BucketPlacementData] {
        &[]
    }

    /// The buckets that another layer places and a layer draws, at its own
    /// translation.
    fn follower_data(_state: &Self::State) -> &[BucketPlacementData] {
        &[]
    }

    /// Whether the feature `feature` with `geometry`, in a tile whose
    /// `projector` projects it, intersects `query_geometry` as a layer with
    /// `evaluated` draws it at `zoom`; false unless a layer type overrides
    /// it.
    #[expect(
        clippy::too_many_arguments,
        reason = "the layer's values, the query geometry, the feature and its state, and the zoom, camera and \
                  projector that test them"
    )]
    fn query_intersects_feature(
        _evaluated: &EvaluatedLayer<Self>,
        _query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        _geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        _zoom: f32,
        _view: &View,
        _pixels_to_tile_units: f32,
        _projector: &TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        false
    }
}

/// A layer's values of its type `T`: its style, its paint values in
/// transition, and their evaluation for the last frame. The engine keeps
/// them and changes them; the hooks read them.
pub struct LayerValues<T: LayerType> {
    pub style: Arc<LayerStyle<T>>,
    pub unevaluated: <T::PaintProperties as PaintSet>::Unevaluated,
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

/// What a layer's update reads of the frame, and the atlases it adds to.
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
    /// The atlases the layers' draws share, which the encode uploads.
    pub atlases: &'a mut Atlases,
    /// The layer's source, `None` for a layer without one.
    pub(crate) source: Option<&'a dyn RenderSource>,
}

impl<T: LayerType> UpdateContext<'_, '_, T> {
    /// The deepest zoom the layer's source has tiles of, if it says.
    #[must_use]
    pub fn source_max_zoom(&self) -> Option<u8> {
        self.source.and_then(RenderSource::max_zoom)
    }
}

/// What a layer's draws read of the frame, and the GPU services that make
/// them.
pub struct DrawContext<'a, 'f, 'p, T: LayerType> {
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
    /// The atlases the layers' draws share.
    pub atlases: &'a Atlases,
    /// The encode's GPU state, camera and frame values.
    pub paint: &'a mut PaintParameters<'p>,
}
