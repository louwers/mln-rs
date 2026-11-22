//! The `probe` layer type: a layer without a source that draws with the
//! backend's own commands, as a host outside the renderer does. It keeps
//! what the last frame lent it ([`ProbeLayer::lent`]), counts the ends of
//! the GPU's work that it gave in its data ([`ProbeEnds`]), and is hit by a
//! query at the center of the view, as a mark it drew there would be.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_rs::layer::{
    AfterGpu, Draws, EvaluateCx, Evaluation, FrameQuery, GpuEnd, Hit, Layer, LayerStyle, LayerTypeInfo, LendAt, Lent,
    NewCx, NoProperties, NoTiles, PrepareCx, PropertyTable, PropertyVisitor, SourceKind, StyleLayerType, UpdateCx,
};
use mln_rs::feature::Identifier;
use mln_rs::mln_util::feature::Feature;

/// How far from the view's center, in logical pixels, a query point hits a
/// probe layer.
pub const HIT_RADIUS: f64 = 4.0;

/// The `probe` layer type's style half.
pub struct Probe;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Probe {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "probe", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ProbeEnds;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
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

    /// One feature, of id 1, where the query's first point is within
    /// [`HIT_RADIUS`] of the view's center.
    fn hit_frame(&self, query: &FrameQuery<'_, Self>, hits: &mut Vec<Hit>) {
        let size = query.camera.size();
        let Some(point) = query.geometry.first() else { return };
        let (dx, dy) = (point.x - f64::from(size.width) / 2.0, point.y - f64::from(size.height) / 2.0);
        let distance = dx.hypot(dy);
        if distance <= HIT_RADIUS {
            hits.push(Hit { feature: Feature { id: Identifier::Uint(1), ..Feature::default() }, distance: distance as f32 });
        }
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
