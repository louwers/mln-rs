//! The placement pass as the engine holds it, whichever its layer type.

use std::any::Any;

use mln_util::error::Error;
use smallvec::SmallVec;

use super::{
    BucketKey, CollisionGroups, Placed, PlacementBuckets, PlacementContext, PlacementPass, PlacementQuery,
    PlacingLayer, Prepared,
};
use crate::bucket::Bucket;
use crate::layer::LayerType;
use crate::layer_drawing::ChangedBuckets;
use crate::render_layer::{Layer, RenderLayer};
use crate::update_parameters::UpdateParameters;

/// The buckets of laid-out tiles, by their keys: the tile store's, or the
/// copies that the benchmarks hold.
pub(crate) trait LaidBuckets {
    fn bucket(&self, key: &BucketKey) -> Option<&dyn Bucket>;
    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn Bucket>;
}

/// What the engine hands its placement pass of a frame: the frame's layers
/// of the pass's type, bottom to top, its update and its tiles' buckets.
pub(crate) struct PlacementFrame<'a> {
    pub(crate) layers: &'a [&'a dyn RenderLayer],
    pub(crate) update_parameters: &'a UpdateParameters,
    pub(crate) buckets: &'a mut dyn LaidBuckets,
}

/// A [`PlacementPass`], whichever its layer type: its hooks over the
/// frame's layers as the engine holds them.
pub(crate) trait ErasedPlacement {
    /// See [`PlacementPass::prepare`].
    fn prepare(&mut self, frame: &mut PlacementFrame<'_>) -> Prepared;

    /// See [`PlacementPass::place`].
    fn place(&mut self, frame: &mut PlacementFrame<'_>, groups: CollisionGroups, fade: f32) -> Placed;

    /// See [`PlacementPass::update_buckets`].
    fn update_buckets(&mut self, frame: &mut PlacementFrame<'_>, changed: &mut ChangedBuckets);

    /// See [`PlacementPass::forget_buckets`].
    fn forget_buckets(&mut self);

    /// See [`PlacementPass::reset`].
    fn reset(&mut self);

    /// See [`PlacementPass::query_rendered_features`].
    fn query_rendered_features(&self, query: &mut PlacementQuery<'_>) -> Result<(), Error>;

    /// The pass, for a caller that knows its type.
    fn pass(&self) -> &dyn Any;

    /// [`pass`](Self::pass), to change.
    fn pass_mut(&mut self) -> &mut dyn Any;
}

/// The placement pass of the layer type `T`.
pub(crate) struct TypedPlacement<T: LayerType>(Box<dyn PlacementPass<T>>);

/// `pass`, as the engine holds it.
pub(crate) fn typed<T: LayerType>(pass: Box<dyn PlacementPass<T>>) -> Box<dyn ErasedPlacement> {
    Box::new(TypedPlacement(pass))
}

/// Runs `hook` with the context of `frame` for a pass of the type `T`: the
/// frame's layers of the type, as the type sees them.
fn with_context<T: LayerType, R>(
    frame: &mut PlacementFrame<'_>,
    hook: impl FnOnce(&mut PlacementContext<'_, T>) -> R,
) -> R {
    let layers: SmallVec<[PlacingLayer<'_, T>; 8]> = frame
        .layers
        .iter()
        .filter_map(|&layer| {
            let layer: &dyn Any = layer;
            layer.downcast_ref::<Layer<T>>().map(Layer::placing)
        })
        .collect();
    debug_assert_eq!(layers.len(), frame.layers.len(), "a layer of another type than its placement's");
    hook(&mut PlacementContext {
        layers: &layers,
        update_parameters: frame.update_parameters,
        buckets: PlacementBuckets::new(&mut *frame.buckets),
    })
}

impl<T: LayerType> ErasedPlacement for TypedPlacement<T> {
    fn prepare(&mut self, frame: &mut PlacementFrame<'_>) -> Prepared {
        with_context(frame, |cx| self.0.prepare(cx))
    }

    fn place(&mut self, frame: &mut PlacementFrame<'_>, groups: CollisionGroups, fade: f32) -> Placed {
        with_context(frame, |cx| self.0.place(cx, groups, fade))
    }

    fn update_buckets(&mut self, frame: &mut PlacementFrame<'_>, changed: &mut ChangedBuckets) {
        with_context(frame, |cx| self.0.update_buckets(cx, changed));
    }

    fn forget_buckets(&mut self) {
        self.0.forget_buckets();
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn query_rendered_features(&self, query: &mut PlacementQuery<'_>) -> Result<(), Error> {
        self.0.query_rendered_features(query)
    }

    fn pass(&self) -> &dyn Any {
        &*self.0
    }

    fn pass_mut(&mut self) -> &mut dyn Any {
        &mut *self.0
    }
}
