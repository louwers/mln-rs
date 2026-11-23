//! A participant as the engine holds it, whichever its layer type.

use std::any::Any;

use mln_map_types::View;
use smallvec::SmallVec;

use super::run::FrameLayer;
use super::{
    BucketKey, CollisionIndex, PlaceCx, Placement, PlacementBuckets, PlacementCx, PlacingLayer, PlacingTile, Prepared,
};
use crate::bucket::ErasedBucket;
use crate::layer::{Layer, LayerValues};
use crate::render_layer::{Hosted, RenderLayer};
use crate::tile::contents::LayoutResult;

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
/// and [`update`](Placement::update) of a frame.
pub(crate) struct ErasedFrame<'a> {
    /// The frame's placing layers of every participant, bottom to top.
    pub(crate) layers: &'a [FrameLayer],
    /// Their tiles, a run per layer.
    pub(crate) tiles: &'a [PlacingTile],
    /// The renderer's layers, which `layers` index.
    pub(crate) render_layers: &'a [Box<dyn RenderLayer>],
    pub(crate) view: &'a View,
    pub(crate) buckets: &'a mut dyn LaidBuckets,
    /// The current placement's index.
    pub(crate) placed: &'a mut CollisionIndex,
}

/// A [`Placement`], whichever its layer type: its hooks over layers as the
/// engine holds them, each with its tiles.
pub(crate) trait ErasedPlacement {
    /// See [`Placement::prepare`], for the frame's layers of the
    /// participant `participant`.
    fn prepare(&mut self, participant: usize, frame: &mut ErasedFrame<'_>) -> Prepared;

    /// [`Placement::place_crossing`] if `crossing`, else
    /// [`Placement::place`].
    fn place(&mut self, layer: &dyn RenderLayer, tiles: &[PlacingTile], crossing: bool, cx: &mut PlaceCx<'_>);

    /// See [`Placement::commit`].
    fn commit(&mut self, cx: &mut PlaceCx<'_>);

    /// See [`Placement::update`], for the frame's layers of the
    /// participant `participant`.
    fn update(&mut self, participant: usize, frame: &mut ErasedFrame<'_>);

    /// See [`Placement::forget_buckets`].
    fn forget_buckets(&mut self);

    /// See [`Placement::forget_buckets`] and then
    /// [`Placement::forget_placement`].
    fn forget(&mut self);
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

/// Runs `hook` with the context of `frame` for its layers of the
/// participant `participant`, as `P` sees them.
fn with_cx<P: Placement, R>(
    participant: usize,
    frame: &mut ErasedFrame<'_>,
    hook: impl FnOnce(&mut PlacementCx<'_, P::Layer>) -> R,
) -> R {
    let (tiles, render_layers) = (frame.tiles, frame.render_layers);
    let own = frame.layers.iter().filter(|layer| layer.participant == participant);
    // Sized at once: a filter leaves the length unknown, and the vector would
    // grow in steps.
    let mut typed: SmallVec<[PlacingLayer<'_, P::Layer>; 8]> = SmallVec::with_capacity(own.clone().count());
    typed.extend(own.filter_map(|layer| {
        let (tiles, followers) = tiles[layer.tiles.clone()].split_at(layer.led);
        Some(PlacingLayer { layer: values::<P>(&*render_layers[layer.layer])?, tiles, followers })
    }));
    let buckets = PlacementBuckets::new(&mut *frame.buckets);
    hook(&mut PlacementCx::new(&typed, frame.view, buckets, &mut *frame.placed))
}

impl<P: Placement> ErasedPlacement for P {
    fn prepare(&mut self, participant: usize, frame: &mut ErasedFrame<'_>) -> Prepared {
        with_cx::<P, _>(participant, frame, |cx| Placement::prepare(self, cx))
    }

    fn place(&mut self, layer: &dyn RenderLayer, tiles: &[PlacingTile], crossing: bool, cx: &mut PlaceCx<'_>) {
        let Some(layer) = values::<P>(layer) else { return };
        let layer = PlacingLayer { layer, tiles, followers: &[] };
        if crossing { Placement::place_crossing(self, &layer, cx) } else { Placement::place(self, &layer, cx) }
    }

    fn commit(&mut self, cx: &mut PlaceCx<'_>) {
        Placement::commit(self, cx);
    }

    fn update(&mut self, participant: usize, frame: &mut ErasedFrame<'_>) {
        with_cx::<P, _>(participant, frame, |cx| Placement::update(self, cx));
    }

    fn forget_buckets(&mut self) {
        Placement::forget_buckets(self);
    }

    fn forget(&mut self) {
        Placement::forget_buckets(self);
        Placement::forget_placement(self);
    }
}
