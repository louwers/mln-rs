// Ports src/mln/renderer/render_tree.hpp and the RenderTreeImpl / LayerRenderItem
// parts of src/mln/renderer/render_orchestrator.cpp.
//
// C++ `RenderTree` is an abstract class with one implementation
// (`RenderTreeImpl` in render_orchestrator.cpp); the port has one struct.
// `std::set<LayerRenderItem>` (ordered and deduplicated by `index`) is a
// `VecMap<u32, LayerRenderItem>`.

use mln_gfx::{LayerGroupBasePtr, UploadPass}; // layers: debug
use mln_map_types::{MapDebugOptions, MapMode, TransformState};
use mln_util::Color;
use mln_util::chrono::TimePoint;
use mln_vec_map::VecMap;

use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::render_layer::RenderLayerPtr;
use crate::render_pass::RenderPass; // layers: debug

/// `RenderItem::DebugType` (layers: debug).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DebugType {
    TextOutline,
    Text,
    Border,
}

/// `RenderItem::DebugLayerGroupMap` (layers: debug).
pub type DebugLayerGroupMap = VecMap<DebugType, LayerGroupBasePtr>;

/// `RenderItem`: the render items of sources (`sourceRenderItems`). The
/// layer render items are the concrete `LayerRenderItem`.
pub trait RenderItem {
    fn upload(&self, upload_pass: &dyn UploadPass);
    fn render(&self, _parameters: &mut PaintParameters<'_>) {}
    fn has_render_pass(&self, pass: RenderPass) -> bool;
    fn get_name(&self) -> &str;
    /// `updateDebugDrawables(DebugLayerGroupMap&, PaintParameters&)`; a
    /// no-op for the items that have none (LayerRenderItem, the image
    /// source). The orchestrator gives each debug layer group its tweaker
    /// (layers: debug).
    fn update_debug_drawables(&self, _debug_layer_groups: &mut DebugLayerGroupMap, _parameters: &PaintParameters<'_>) {}
}

/// `LayerRenderItem`.
#[derive(Clone)]
pub struct LayerRenderItem {
    pub layer: RenderLayerPtr,
    /// `RenderSource* source`: the id of the orchestrator's render source
    /// (C++ points at it), `None` for layers without a source.
    pub source: Option<String>,
    pub index: u32,
}

impl LayerRenderItem {
    pub fn new(layer: RenderLayerPtr, source: Option<String>, index: u32) -> Self {
        LayerRenderItem { layer, source, index }
    }

    pub fn has_render_pass(&self, pass: RenderPass) -> bool {
        self.layer.borrow().has_render_pass(pass)
    }

    pub fn get_name(&self) -> String {
        self.layer.borrow().get_id().to_owned()
    }
}

/// `RenderTreeParameters`.
pub struct RenderTreeParameters {
    pub transform_params: TransformParameters,
    pub map_mode: MapMode,
    pub debug_options: MapDebugOptions,
    pub time_point: TimePoint,
    /// conformance: `EvaluatedLight light`.
    pub light: crate::render_light::EvaluatedLight,
    pub sky: Option<crate::render_sky::EvaluatedSky>,
    pub has_3d: bool,
    pub opaque_pass_cut_off: u32,
    /// C++ default-constructs `Color` (transparent black).
    pub background_color: Color,
    pub symbol_fade_change: f32,
    pub needs_repaint: bool,
    pub loaded: bool,
    pub placement_changed: bool,
}

impl RenderTreeParameters {
    pub fn new(
        state: &TransformState,
        map_mode: MapMode,
        debug_options: MapDebugOptions,
        time_point: TimePoint,
    ) -> Self {
        RenderTreeParameters {
            transform_params: TransformParameters::new(state),
            map_mode,
            debug_options,
            time_point,
            light: Default::default(),
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

/// `RenderTree` / `RenderTreeImpl`.
pub struct RenderTree {
    parameters: Box<RenderTreeParameters>,
    pub(crate) layer_render_items: VecMap<u32, LayerRenderItem>,
    source_render_items: Vec<Box<dyn RenderItem>>,
    // symbols:
    layers_need_placement: Vec<crate::render_layer::RenderLayerPtr>,
    layers_following_placement: Vec<crate::render_layer::RenderLayerPtr>,
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
        layers_need_placement: Vec<crate::render_layer::RenderLayerPtr>,
        layers_following_placement: Vec<crate::render_layer::RenderLayerPtr>,
        placement: std::rc::Rc<crate::text::placement::Placement>,
        update_symbol_opacities: bool,
    ) {
        self.layers_need_placement = layers_need_placement;
        self.layers_following_placement = layers_following_placement;
        self.placement = Some(placement);
        self.update_symbol_opacities = update_symbol_opacities;
    }

    /// `prepare()`: updates the symbol buckets of the layers that need
    /// placement, then the positions of the layers that draw buckets other
    /// layers place.
    pub fn prepare(&mut self) {
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

    pub fn get_layer_render_item_map(&self) -> &VecMap<u32, LayerRenderItem> {
        &self.layer_render_items
    }

    pub fn get_layer_render_items(&self) -> Vec<LayerRenderItem> {
        self.layer_render_items.values().cloned().collect()
    }

    /// `getSourceRenderItems()`.
    pub fn get_source_render_items(&self) -> &[Box<dyn RenderItem>] {
        &self.source_render_items
    }

    pub fn get_parameters(&self) -> &RenderTreeParameters {
        &self.parameters
    }

    /// `getElapsedTime()`, in seconds.
    pub fn get_elapsed_time(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }
}
