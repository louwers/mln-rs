// C++ `RenderTree` is an abstract class with one implementation
// (`RenderTreeImpl` in render_orchestrator.cpp); the port has one struct.
// `std::set<LayerRenderItem>` (ordered and deduplicated by `index`) is a
// `VecMap<u32, LayerRenderItem>`.

use mln_map_types::{MapDebugOptions, TransformState};
use mln_util::Color;
use mln_vec_map::VecMap;

use crate::gfx::context::Context; // layers: debug
use crate::layer_group::LayerGroupPtr;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::render_layer::RenderLayerPtr;

/// The kinds of debug drawables (layers: debug).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DebugType {
    TextOutline,
    Text,
    Border,
}

/// The debug layer groups, by debug type (layers: debug).
pub(crate) type DebugLayerGroupMap = VecMap<DebugType, LayerGroupPtr>;

/// The render item of a source. The layer render items are the concrete
/// `LayerRenderItem`.
pub(crate) trait RenderItem {
    fn upload(&self, context: &Context);
    /// Updates the item's debug drawables; a no-op for the items that have
    /// none (`LayerRenderItem`, the image source). The orchestrator gives
    /// each debug layer group its tweaker (layers: debug).
    fn update_debug_drawables(&self, _debug_layer_groups: &mut DebugLayerGroupMap, _parameters: &PaintParameters<'_>) {}
}

#[derive(Clone)]
pub(crate) struct LayerRenderItem {
    pub(crate) layer: RenderLayerPtr,
    /// The id of the orchestrator's render source, `None` for layers without
    /// a source.
    pub(crate) source: Option<String>,
}

impl LayerRenderItem {
    pub(crate) fn new(layer: RenderLayerPtr, source: Option<String>) -> Self {
        LayerRenderItem { layer, source }
    }
}

pub(crate) struct RenderTreeParameters {
    pub(crate) transform_params: TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    /// conformance: `EvaluatedLight light`.
    pub(crate) light: crate::render_light::EvaluatedLight,
    pub(crate) sky: Option<crate::render_sky::EvaluatedSky>,
    pub(crate) has_3d: bool,
    pub(crate) opaque_pass_cut_off: u32,
    /// Transparent black unless the style has a solid background.
    pub(crate) background_color: Color,
    pub(crate) symbol_fade_change: f32,
    pub(crate) needs_repaint: bool,
    pub(crate) loaded: bool,
    pub(crate) placement_changed: bool,
}

impl RenderTreeParameters {
    pub(crate) fn new(state: &TransformState, debug_options: MapDebugOptions) -> Self {
        RenderTreeParameters {
            transform_params: TransformParameters::new(state),
            debug_options,
            light: crate::render_light::EvaluatedLight::default(),
            sky: None,
            has_3d: false,
            opaque_pass_cut_off: 0,
            background_color: Color::new_unchecked(0.0, 0.0, 0.0, 0.0),
            symbol_fade_change: 0.0,
            needs_repaint: false,
            loaded: false,
            placement_changed: false,
        }
    }
}

/// What a frame draws, which its update and prepare made for its encode.
pub(crate) struct RenderTree {
    parameters: Box<RenderTreeParameters>,
    layer_render_items: VecMap<u32, LayerRenderItem>,
    source_render_items: Vec<Box<dyn RenderItem>>,
    /// When the frame's update started.
    start_time: mln_util::time::Instant,
    /// When the frame started placing symbols.
    symbols_start: mln_util::time::Instant,
}

impl RenderTree {
    pub(crate) fn new(
        parameters: Box<RenderTreeParameters>,
        layer_render_items: VecMap<u32, LayerRenderItem>,
        source_render_items: Vec<Box<dyn RenderItem>>,
        start_time: mln_util::time::Instant,
        symbols_start: mln_util::time::Instant,
    ) -> Self {
        RenderTree { parameters, layer_render_items, source_render_items, start_time, symbols_start }
    }

    /// When the frame started placing symbols.
    #[must_use]
    pub(crate) fn symbols_start(&self) -> mln_util::time::Instant {
        self.symbols_start
    }

    #[must_use]
    pub(crate) fn get_layer_render_item_map(&self) -> &VecMap<u32, LayerRenderItem> {
        &self.layer_render_items
    }

    #[must_use]
    pub(crate) fn get_source_render_items(&self) -> &[Box<dyn RenderItem>] {
        &self.source_render_items
    }

    #[must_use]
    pub(crate) fn get_parameters(&self) -> &RenderTreeParameters {
        &self.parameters
    }

    /// The time since the render tree was created, in seconds.
    #[must_use]
    pub(crate) fn get_elapsed_time(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }
}
