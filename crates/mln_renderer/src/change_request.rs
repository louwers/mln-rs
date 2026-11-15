// The C++ class hierarchy (`ChangeRequest::execute(RenderOrchestrator&)`) is
// an enum here; `RenderOrchestrator::process_changes` executes it.

use std::cell::RefCell;
use std::rc::Weak;

use crate::layer_group::LayerGroupPtr;
use crate::layer_tweaker::LayerTweaker;

pub enum ChangeRequest {
    AddLayerGroup(LayerGroupPtr),
    RemoveLayerGroup(LayerGroupPtr),
    UpdateLayerGroupIndex(LayerGroupPtr, i32),
    /// Adds a tweaker that runs for the layer group each frame. The
    /// orchestrator keeps the lists, and runs the request before the
    /// tweakers first run in the same frame.
    AddLayerTweaker(LayerGroupPtr, Weak<RefCell<dyn LayerTweaker>>),
    // dem-heatmap: render targets (hillshade, heatmap).
    AddRenderTarget(crate::render_target::RenderTargetPtr),
    RemoveRenderTarget(crate::render_target::RenderTargetPtr),
}

pub(crate) type UniqueChangeRequestVec = Vec<ChangeRequest>;
