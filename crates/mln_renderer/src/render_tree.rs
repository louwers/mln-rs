// C++ `RenderTree` is an abstract class with one implementation
// (`RenderTreeImpl` in render_orchestrator.cpp); the port has one struct.
// `std::set<LayerRenderItem>` (ordered and deduplicated by `index`) is a
// `VecMap<u32, LayerRenderItem>`.

use mln_gfx::{LayerGroupBasePtr, UploadPass}; // layers: debug
use mln_map_types::{MapDebugOptions, TransformState};
use mln_util::Color;
use mln_vec_map::VecMap;

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
pub(crate) type DebugLayerGroupMap = VecMap<DebugType, LayerGroupBasePtr>;

/// The render item of a source. The layer render items are the concrete
/// `LayerRenderItem`.
pub(crate) trait RenderItem {
    fn upload(&self, upload_pass: &dyn UploadPass);
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

pub struct RenderTree {
    parameters: Box<RenderTreeParameters>,
    pub(crate) layer_render_items: VecMap<u32, LayerRenderItem>,
    source_render_items: Vec<Box<dyn RenderItem>>,
    // symbols:
    layers_need_placement: Vec<RenderLayerPtr>,
    layers_following_placement: Vec<RenderLayerPtr>,
    placement: Option<std::rc::Rc<crate::text::placement::Placement>>,
    update_symbol_opacities: bool,
    start_time: mln_util::time::Instant,
}

impl RenderTree {
    pub(crate) fn new(
        parameters: Box<RenderTreeParameters>,
        layer_render_items: VecMap<u32, LayerRenderItem>,
        source_render_items: Vec<Box<dyn RenderItem>>,
        start_time: mln_util::time::Instant,
    ) -> Self {
        RenderTree {
            parameters,
            layer_render_items,
            source_render_items,
            layers_need_placement: Vec::new(),
            layers_following_placement: Vec::new(),
            placement: None,
            update_symbol_opacities: false,
            start_time,
        }
    }

    /// symbols: the placement half of the C++ `RenderTreeImpl` constructor.
    pub(crate) fn set_placement(
        &mut self,
        layers_need_placement: Vec<RenderLayerPtr>,
        layers_following_placement: Vec<RenderLayerPtr>,
        placement: std::rc::Rc<crate::text::placement::Placement>,
        update_symbol_opacities: bool,
    ) {
        self.layers_need_placement = layers_need_placement;
        self.layers_following_placement = layers_following_placement;
        self.placement = Some(placement);
        self.update_symbol_opacities = update_symbol_opacities;
    }

    /// Updates the symbol buckets of the layers that need placement, then the
    /// positions of the layers that draw buckets other layers place.
    pub(crate) fn prepare(&mut self) {
        let Some(placement) = &self.placement else {
            return;
        };
        for layer in self.layers_need_placement.iter().rev() {
            placement.update_layer_buckets(
                &*layer.borrow(),
                &self.parameters.transform_params.state,
                self.update_symbol_opacities,
            );
        }
        for layer in &self.layers_following_placement {
            placement.update_follower_buckets(&*layer.borrow(), &self.parameters.transform_params.state);
        }
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
