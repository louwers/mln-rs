//! Unstable: placement, which decides what of the frame's layers shows
//! where it would collide with what shows already, and fades what it shows
//! between placements. Anything here can change or go in any release.
//!
//! A layer type whose layers place declares a [`PlacementPass`]
//! ([`LayerType::PLACEMENT`]): the pass over every layer of the type that
//! the frame draws, which owns the order it places them in and what it
//! keeps of their buckets between frames. A registry has at most one such
//! type: [`add_layer_type`](crate::RegistryBuilder::add_layer_type) refuses
//! a second. The pass runs on the renderer's thread, and nothing of it
//! crosses the worker codec.
//!
//! The engine keeps the placement's schedule: it decides each frame
//! whether the pass places again, keeps the clock its fade runs on, and
//! swaps in the placement the pass makes. Each frame, it
//! [prepares](PlacementPass::prepare) the pass, has it
//! [place](PlacementPass::place) when a placement is due, and then
//! [brings the layers' buckets](PlacementPass::update_buckets) to the
//! placement. A pass places into a [`Grid`] of [`Shape`]s, with the
//! sources' [`CollisionGroups`]. The pass answers rendered-feature queries
//! of its type's layers from what it placed
//! ([`PlacementPass::query_rendered_features`]).

mod erased;
mod grid;
mod groups;
mod schedule;

use std::any::Any;
use std::marker::PhantomData;

use mln_style::expression::GlobalStateMap;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::geo::ScreenCoordinate;

pub(crate) use self::erased::{ErasedFrame, ErasedPlacement, LaidBuckets, typed};
pub use self::grid::{Aabb, Grid, Shape};
pub use self::groups::CollisionGroups;
pub(crate) use self::schedule::Schedule;
use crate::erased_layer::{recover_bucket, recover_bucket_mut};
use crate::geometry::feature_index::{FeatureIndex, FeatureSelection, QueryLayers, QueryResults};
use crate::layer::{LayerType, LayerValues};
use crate::layer_drawing::ChangedBuckets;
use crate::query::RenderedQueryOptions;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// What makes a [`PlacementPass`] of the layer type `T`, which a type whose
/// layers place declares ([`LayerType::PLACEMENT`]).
pub type NewPlacement<T> = fn() -> Box<dyn PlacementPass<T>>;

/// The placement pass of the layer type `T`: the capability of a type whose
/// layers place, which it declares with [`LayerType::PLACEMENT`]. The
/// engine makes one pass per renderer, and drives it on its schedule.
pub trait PlacementPass<T: LayerType>: Any {
    /// Readies the frame's placement, in each frame's update, before the
    /// schedule decides whether to place: brings what the pass keeps of the
    /// layers' buckets from frame to frame up to date with `cx`'s.
    fn prepare(&mut self, cx: &mut PlacementContext<'_, T>) -> Prepared;

    /// Places `cx`'s layers, topmost first, each where it collides with
    /// nothing placed before it in `groups`, in a placement that replaces
    /// the current one, for a frame that the schedule places. `fade` is how
    /// far the fade of the placement it replaces had come, which a
    /// continuous map's placement fades on from: 0 as the fade starts, 1
    /// and past once it is done.
    fn place(&mut self, cx: &mut PlacementContext<'_, T>, groups: CollisionGroups, fade: f32) -> Placed;

    /// Brings `cx`'s layers' buckets to the current placement, in each
    /// frame's update after the schedule placed or didn't, and adds what it
    /// changed of them to `changed`.
    fn update_buckets(&mut self, cx: &mut PlacementContext<'_, T>, changed: &mut ChangedBuckets);

    /// Forgets what it keeps of the layers' buckets from frame to frame,
    /// keeping the current placement, which the next one fades from: the
    /// renderer's tiles went, or the frame is a still image's, which keeps
    /// none.
    fn forget_buckets(&mut self);

    /// Discards the current placement, and forgets what
    /// [`forget_buckets`](Self::forget_buckets) forgets: no layer has
    /// anything to place.
    fn reset(&mut self);

    /// Adds the features of `query`'s layers that the current placement
    /// shows within its geometry.
    fn query_rendered_features(&self, query: &mut PlacementQuery<'_>) -> Result<(), Error>;
}

/// What a [`PlacementPass`] reads of the frame: its layers of the pass's
/// type, the frame's update and the buckets of its tiles.
pub struct PlacementContext<'a, T: LayerType> {
    /// The layers of the type that the frame draws, bottom to top.
    pub layers: &'a [PlacingLayer<'a, T>],
    /// The update the frame draws.
    pub update_parameters: &'a UpdateParameters,
    /// The buckets of the frame's tiles, which the layers' updates found.
    pub buckets: PlacementBuckets<'a, T>,
}

/// A layer that a [`PlacementPass`] places: its values, and what its type
/// keeps of it, which the layer's update filled.
pub struct PlacingLayer<'a, T: LayerType> {
    /// The layer's values.
    pub layer: &'a LayerValues<T>,
    /// What the layer's type keeps of it besides its drawing.
    pub state: &'a T::State,
}

/// What a frame's [`PlacementPass::prepare`] tells the schedule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Whether a layer has something to place. Without, a continuous map
    /// discards its placement.
    pub places: bool,
    /// Whether the layers' buckets include some the pass hadn't seen, which
    /// a continuous map with a flat view places soon.
    pub added: bool,
    /// Whether placing now would place from other inputs than the current
    /// placement did, which a continuous map places again for.
    pub changed: bool,
}

/// What the schedule of a continuous map reads of a placement that
/// [`PlacementPass::place`] made. A still image shows its placement at once,
/// and reads neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    /// Whether what shows changed from the placement it replaced, which
    /// starts its fade.
    pub changed: bool,
    /// Whether everything that shows or hides was at the end of its fade
    /// when the placement was made. A placement that wasn't places again,
    /// once due, to bring it there.
    pub settled: bool,
}

/// Where a bucket a frame's layer draws is among the frame's tiles, which
/// the layer's update takes ([`UpdateContext::bucket_key`](crate::layer::UpdateContext::bucket_key))
/// for its placement to find it by ([`PlacementBuckets`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BucketKey {
    /// The bucket's index among its tile's buckets.
    pub(crate) bucket: usize,
    /// Where the tile is among its source's tiles, as the frame's render
    /// tiles found it.
    pub(crate) slot: Option<usize>,
    pub(crate) tile: OverscaledTileID,
    pub(crate) source: SourceKey,
}

/// The buckets of the frame's tiles as a [`PlacementPass`] of the type `T`
/// reads and changes them, by their [`BucketKey`]s.
pub struct PlacementBuckets<'a, T: LayerType> {
    store: &'a mut dyn LaidBuckets,
    _type: PhantomData<fn() -> T>,
}

impl<'a, T: LayerType> PlacementBuckets<'a, T> {
    pub(crate) fn new(store: &'a mut dyn LaidBuckets) -> Self {
        PlacementBuckets { store, _type: PhantomData }
    }

    /// The bucket at `key`, if it is there and is the type's.
    #[must_use]
    pub fn get(&self, key: &BucketKey) -> Option<&T::Bucket> {
        recover_bucket::<T>(self.store.bucket(key)?)
    }

    /// [`get`](Self::get), to change.
    pub fn get_mut(&mut self, key: &BucketKey) -> Option<&mut T::Bucket> {
        recover_bucket_mut::<T>(self.store.bucket_mut(key)?)
    }
}

/// A rendered-feature query of the layers of a [`PlacementPass`]'s type,
/// which the pass answers from what it placed.
pub struct PlacementQuery<'a> {
    /// The query's geometry, in the view's pixels.
    pub geometry: &'a [ScreenCoordinate],
    /// The queried layers of the pass's type, by id.
    pub(crate) layers: &'a QueryLayers<'a>,
    pub(crate) options: &'a RenderedQueryOptions,
    pub(crate) global_state: Option<&'a GlobalStateMap>,
    /// The features found, by layer id, which the query adds to.
    pub(crate) results: &'a mut QueryResults,
}

/// A feature of a tile's [`FeatureIndex`], which a [`PlacementQuery`] looks
/// up.
#[derive(Clone, Copy, Debug)]
pub struct IndexedFeature<'a> {
    /// The feature's index in its source layer.
    pub index: usize,
    /// The name of the feature's source layer.
    pub source_layer: &'a str,
    /// The id of the leader of the layers whose bucket has the feature.
    pub bucket_leader: &'a str,
}

impl PlacementQuery<'_> {
    /// Adds `features`, of the tile `tile_id` whose feature index is
    /// `index`, that the placement shows within the query's geometry, in
    /// their order, to the results of each queried layer whose bucket has
    /// them and whose filter takes them.
    pub fn add_features<'f>(
        &mut self,
        index: &FeatureIndex,
        tile_id: &OverscaledTileID,
        features: impl IntoIterator<Item = IndexedFeature<'f>>,
    ) -> Result<(), Error> {
        let selection = FeatureSelection {
            layers: self.layers,
            options: self.options,
            global_state: self.global_state,
            tile_id,
            feature_state: None,
        };
        for (layer_id, features) in index.lookup_features(features, &selection)? {
            self.results.entry(layer_id).or_default().extend(features);
        }
        Ok(())
    }
}
