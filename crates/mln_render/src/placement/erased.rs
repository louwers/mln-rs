//! A participant as the engine holds it, whichever its layer type.

use std::any::Any;

use smallvec::SmallVec;

use super::{
    BucketKey, CollisionIndex, PlaceCx, Placement, PlacementBuckets, PlacementCx, PlacingLayer, PlacingTile, Prepared,
};
use crate::bucket::ErasedBucket;
use crate::layer::{Layer, LayerValues};
use crate::render_layer::{Hosted, RenderLayer};
use crate::tile::contents::LayoutResult;
use crate::update_parameters::UpdateParameters;

/// The buckets of laid-out tiles, by their keys: the tile store's, or the
/// copies that the benchmarks hold.
pub(crate) trait LaidBuckets {
    /// The layout that has the bucket at `key`.
    fn layout(&self, key: &BucketKey) -> Option<&LayoutResult>;
    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn ErasedBucket>;

    fn bucket(&self, key: &BucketKey) -> Option<&dyn ErasedBucket> {
        self.layout(key)?.layer_render_data.bucket(key.bucket)
    }
}

/// What the engine hands a participant's [`prepare`](Placement::prepare)
/// and [`update`](Placement::update) of a frame, besides its layers.
pub(crate) struct ErasedFrame<'a> {
    pub(crate) update_parameters: &'a UpdateParameters,
    pub(crate) buckets: &'a mut dyn LaidBuckets,
    /// The current placement's index.
    pub(crate) placed: &'a mut CollisionIndex,
}

/// A [`Placement`], whichever its layer type: its hooks over layers as the
/// engine holds them, each with its tiles.
pub(crate) trait ErasedPlacement {
    /// See [`Placement::prepare`].
    fn prepare(&mut self, layers: &[(&dyn RenderLayer, &[PlacingTile])], frame: &mut ErasedFrame<'_>) -> Prepared;

    /// See [`Placement::begin`].
    fn begin(&mut self, cx: &mut PlaceCx<'_>);

    /// [`Placement::place_crossing`] if `crossing`, else
    /// [`Placement::place`].
    fn place(&mut self, layer: &dyn RenderLayer, tiles: &[PlacingTile], crossing: bool, cx: &mut PlaceCx<'_>);

    /// See [`Placement::commit`].
    fn commit(&mut self, cx: &mut PlaceCx<'_>);

    /// See [`Placement::update`].
    fn update(&mut self, layers: &[(&dyn RenderLayer, &[PlacingTile])], frame: &mut ErasedFrame<'_>);

    /// See [`Placement::forget`].
    fn forget(&mut self, placement: bool);
}

/// A new participant `P`, as the engine holds it.
pub(crate) fn erase<P: Placement>() -> Box<dyn ErasedPlacement> {
    Box::new(P::default())
}

/// `layer`'s values, if it is a layer of the type `P` places.
fn values<P: Placement>(layer: &dyn RenderLayer) -> Option<&LayerValues<<P::Layer as Layer>::Style>> {
    let layer: &dyn Any = layer;
    let hosted = layer.downcast_ref::<Hosted<P::Layer>>();
    debug_assert!(hosted.is_some(), "a layer of another type than its participant's");
    Some(hosted?.parts().1)
}

/// Runs `hook` with the context of `frame` for `layers`, as `P` sees them.
fn with_cx<P: Placement, R>(
    layers: &[(&dyn RenderLayer, &[PlacingTile])],
    frame: &mut ErasedFrame<'_>,
    hook: impl FnOnce(&mut PlacementCx<'_, P::Layer>) -> R,
) -> R {
    let mut typed: SmallVec<[PlacingLayer<'_, P::Layer>; 8]> = SmallVec::with_capacity(layers.len());
    typed.extend(layers.iter().filter_map(|&(layer, tiles)| Some(PlacingLayer { layer: values::<P>(layer)?, tiles })));
    let buckets = PlacementBuckets::new(&mut *frame.buckets);
    hook(&mut PlacementCx::new(&typed, frame.update_parameters, buckets, &mut *frame.placed))
}

impl<P: Placement> ErasedPlacement for P {
    fn prepare(&mut self, layers: &[(&dyn RenderLayer, &[PlacingTile])], frame: &mut ErasedFrame<'_>) -> Prepared {
        with_cx::<P, _>(layers, frame, |cx| Placement::prepare(self, cx))
    }

    fn begin(&mut self, cx: &mut PlaceCx<'_>) {
        Placement::begin(self, cx);
    }

    fn place(&mut self, layer: &dyn RenderLayer, tiles: &[PlacingTile], crossing: bool, cx: &mut PlaceCx<'_>) {
        let Some(layer) = values::<P>(layer) else { return };
        let layer = PlacingLayer { layer, tiles };
        if crossing { Placement::place_crossing(self, &layer, cx) } else { Placement::place(self, &layer, cx) }
    }

    fn commit(&mut self, cx: &mut PlaceCx<'_>) {
        Placement::commit(self, cx);
    }

    fn update(&mut self, layers: &[(&dyn RenderLayer, &[PlacingTile])], frame: &mut ErasedFrame<'_>) {
        with_cx::<P, _>(layers, frame, |cx| Placement::update(self, cx));
    }

    fn forget(&mut self, placement: bool) {
        Placement::forget(self, placement);
    }
}
