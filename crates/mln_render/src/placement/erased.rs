//! The placement pass as the engine holds it, whichever its layer type.

use std::any::Any;

use smallvec::SmallVec;

use super::{
    BucketKey, CollisionIndex, Placed, PlacementBuckets, PlacementContext, PlacementPass, PlacingLayer, PlacingTile,
    Prepared,
};
use crate::bucket::ErasedBucket;
use crate::layer::Layer;
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

/// A placing layer of a frame, by its index among the renderer's layers,
/// with where its tiles that hold a bucket of it are among the frame's
/// placing tiles.
pub(crate) struct PlacingLayerTiles {
    pub(crate) layer: usize,
    pub(crate) tiles: std::ops::Range<usize>,
}

/// What the engine hands its placement pass of a frame: the frame's layers
/// of the pass's type, bottom to top, each with its tiles, its update, its
/// tiles' buckets and the current placement's index.
pub(crate) struct ErasedFrame<'a> {
    pub(crate) layers: &'a [(&'a dyn RenderLayer, &'a [PlacingTile])],
    pub(crate) update_parameters: &'a UpdateParameters,
    pub(crate) buckets: &'a mut dyn LaidBuckets,
    pub(crate) placed: &'a mut CollisionIndex,
}

/// A [`PlacementPass`], whichever its layer type: its hooks over the
/// frame's layers as the engine holds them.
pub(crate) trait ErasedPlacement {
    /// See [`PlacementPass::prepare`].
    fn prepare(&mut self, frame: &mut ErasedFrame<'_>) -> Prepared;

    /// See [`PlacementPass::place`].
    fn place(&mut self, frame: &mut ErasedFrame<'_>, index: &mut CollisionIndex, fade: f32) -> Placed;

    /// See [`PlacementPass::update_buckets`].
    fn update_buckets(&mut self, frame: &mut ErasedFrame<'_>);

    /// See [`PlacementPass::forget_buckets`].
    fn forget_buckets(&mut self);

    /// See [`PlacementPass::reset`].
    fn reset(&mut self);

    /// The pass, for a caller that knows its type.
    fn pass(&self) -> &dyn Any;

    /// [`pass`](Self::pass), to change.
    fn pass_mut(&mut self) -> &mut dyn Any;
}

/// The placement pass of the layer type `T`.
pub(crate) struct TypedPlacement<T: Layer>(Box<dyn PlacementPass<T>>);

/// `pass`, as the engine holds it.
pub(crate) fn typed<T: Layer>(pass: Box<dyn PlacementPass<T>>) -> Box<dyn ErasedPlacement> {
    Box::new(TypedPlacement(pass))
}

/// Runs `hook` with the context of `frame` for a pass of the type `T`: the
/// frame's layers of the type, as the type sees them.
fn with_context<T: Layer, R>(frame: &mut ErasedFrame<'_>, hook: impl FnOnce(&mut PlacementContext<'_, T>) -> R) -> R {
    let layers: SmallVec<[PlacingLayer<'_, T>; 8]> = frame
        .layers
        .iter()
        .filter_map(|&(layer, tiles)| {
            let layer: &dyn Any = layer;
            let (_, values) = layer.downcast_ref::<Hosted<T>>()?.parts();
            Some(PlacingLayer { layer: values, tiles })
        })
        .collect();
    debug_assert_eq!(layers.len(), frame.layers.len(), "a layer of another type than its placement's");
    hook(&mut PlacementContext {
        layers: &layers,
        update_parameters: frame.update_parameters,
        buckets: PlacementBuckets::new(&mut *frame.buckets, &mut *frame.placed),
    })
}

impl<T: Layer> ErasedPlacement for TypedPlacement<T> {
    fn prepare(&mut self, frame: &mut ErasedFrame<'_>) -> Prepared {
        with_context(frame, |cx| self.0.prepare(cx))
    }

    fn place(&mut self, frame: &mut ErasedFrame<'_>, index: &mut CollisionIndex, fade: f32) -> Placed {
        with_context(frame, |cx| self.0.place(cx, index, fade))
    }

    fn update_buckets(&mut self, frame: &mut ErasedFrame<'_>) {
        with_context(frame, |cx| self.0.update_buckets(cx));
    }

    fn forget_buckets(&mut self) {
        self.0.forget_buckets();
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn pass(&self) -> &dyn Any {
        &*self.0
    }

    fn pass_mut(&mut self) -> &mut dyn Any {
        &mut *self.0
    }
}
