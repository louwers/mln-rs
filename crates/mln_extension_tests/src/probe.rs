//! The `probe` layer type: a layer without a source that draws with the
//! backend's own commands, as a host outside the renderer does. It keeps
//! what the last frame lent it ([`ProbeLayer::lent`]) and counts the ends of
//! the GPU's work that it gave in its data ([`ProbeEnds`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_rs::layer::{
    AfterGpu, Draws, EvaluateCx, Evaluation, GpuEnd, Layer, LayerTypeInfo, LendAt, Lent, NewCx, NoProperties, NoTiles,
    PrepareCx, SourceKind, StyleLayerType, UpdateCx,
};

/// The `probe` layer type's style half.
pub struct Probe;

impl StyleLayerType for Probe {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "probe", source: SourceKind::None, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ProbeEnds;
}

/// How the GPU's work that probe layers gave ended, once they dropped: an
/// app gives a layer its counts through the layer's data
/// ([`Map::edit_style_layer_data`](mln_rs::Map::edit_style_layer_data)).
#[derive(Clone, Debug, Default)]
pub struct ProbeEnds(Arc<[AtomicUsize; 2]>);

impl ProbeEnds {
    /// How many ended `end`.
    #[must_use]
    pub fn count(&self, end: GpuEnd) -> usize {
        self.0[end as usize].load(Ordering::Relaxed)
    }
}

/// A probe layer.
pub struct ProbeLayer {
    lent: Vec<(LendAt, Option<f64>)>,
    ends: ProbeEnds,
    after_gpu: AfterGpu,
}

impl ProbeLayer {
    /// When the last frame lent the layer the backend's commands, each with
    /// the depth of the layer's place among the layers in a pass.
    #[must_use]
    pub fn lent(&self) -> &[(LendAt, Option<f64>)] {
        &self.lent
    }
}

impl Layer for ProbeLayer {
    type Style = Probe;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        ProbeLayer { lent: Vec::new(), ends: cx.layer.style.data.clone(), after_gpu: cx.after_gpu.clone() }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        self.ends = cx.layer.style.data.clone();
        Evaluation::TRANSLUCENT
    }

    fn update(&mut self, _cx: &mut UpdateCx<'_, Self>) {
        self.lent.clear();
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        out.lend(LendAt::BeforePasses).lend(LendAt::Pass);
    }

    fn lend(&mut self, at: LendAt, lent: Lent<'_>) {
        self.lent.push((at, lent.layer_depth));
    }
}

impl Drop for ProbeLayer {
    fn drop(&mut self) {
        let ends = self.ends.clone();
        self.after_gpu.push(move |end| {
            ends.0[end as usize].fetch_add(1, Ordering::Relaxed);
        });
    }
}
