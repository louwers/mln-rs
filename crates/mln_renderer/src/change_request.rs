// The C++ class hierarchy (`ChangeRequest::execute(RenderOrchestrator&)`) is
// an enum here; `RenderOrchestrator::process_changes` executes it.

use std::cell::RefCell;
use std::rc::Weak;

use mln_gfx::LayerGroupBasePtr;

use crate::layer_tweaker::LayerTweaker;

pub enum ChangeRequest {
    AddLayerGroup(LayerGroupBasePtr),
    RemoveLayerGroup(LayerGroupBasePtr),
    UpdateLayerGroupIndex(LayerGroupBasePtr, i32),
    /// Not a C++ request: `layerGroup->addLayerTweaker(tweaker)`. C++ layer
    /// groups keep a list of weak tweaker pointers; `LayerTweaker` is a
    /// renderer type and the layer groups live in `mln_gfx`, so the
    /// orchestrator keeps the lists. The request is executed by
    /// `processChanges`, before the tweakers first run in the same frame.
    AddLayerTweaker(LayerGroupBasePtr, Weak<RefCell<dyn LayerTweaker>>),
    // dem-heatmap: render targets (hillshade, heatmap).
    AddRenderTarget(crate::render_target::RenderTargetPtr),
    RemoveRenderTarget(crate::render_target::RenderTargetPtr),
}

pub type UniqueChangeRequest = ChangeRequest;
pub type UniqueChangeRequestVec = Vec<ChangeRequest>;
