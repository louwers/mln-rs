//! Not a contract yet: placement, which decides what of the frame's
//! layers shows where it would collide with what shows already, and fades
//! what it shows between placements. The built-in symbol type places
//! through it. Anything here can change or go in any release.
//!
//! A layer type whose layers place registers with a [`PlacementPass`]
//! ([`register`]): the pass over every layer of the type that the frame
//! draws, which owns the order it places them in and what it keeps of
//! their buckets between frames. The pass runs on the renderer's thread,
//! and nothing of it crosses the worker codec. A placing layer draws the
//! tiles that its source holds while what placement showed of them fades
//! out, besides those of the frame.
//!
//! The engine keeps the placement's schedule: it decides each frame
//! whether the pass places again, keeps the clock its fade runs on
//! ([`fade`]), and swaps in the placement the pass makes. Each frame, it
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
use std::sync::Arc;

use glam::Vec2;
use mln_map_types::ProjectionData;
use mln_style::expression::GlobalStateMap;
use mln_style::expression::functions::RegisterError;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::error::Error;
use mln_util::geo::ScreenCoordinate;

pub(crate) use self::erased::{ErasedFrame, ErasedPlacement, LaidBuckets, PlacingLayerTiles};
pub use self::grid::{Aabb, Grid, Shape};
pub use self::groups::CollisionGroups;
pub(crate) use self::schedule::Schedule;
pub use crate::geometry::feature_index::FeatureIndex;
use crate::geometry::feature_index::{FeatureSelection, QueryLayers, QueryResults};
use crate::bucket_upload::Upload;
use crate::gfx::mesh::Indexes;
use crate::layer::{Bucket, GpuVec, Layer, LayerValues};
use crate::paint_parameters::Frame;
use crate::query::RenderedQueryOptions;
use crate::registry::RegistryBuilder;
use crate::render_source::RenderSource;
use crate::render_tile::{FoundTile, RenderTile, TileContent, ViewPlace};
use crate::renderer::Renderer;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// Adds the layer type of the layer `L`, as
/// [`RegistryBuilder::add_layer_type`] does, whose layers place with a pass
/// `P`, which each renderer makes once. Fails too where the builder has a
/// type whose layers place: a registry has one.
pub fn register<L: Layer, P: PlacementPass<L> + Default>(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_entry::<L>(crate::erased_layer::entry::<L, Places<P>>())
}

/// The placement of a layer type whose layers place with a `P`.
pub(crate) struct Places<P>(PhantomData<fn() -> P>);

impl<L: Layer, P: PlacementPass<L> + Default> crate::erased_layer::NewPlacement<L> for Places<P> {
    const PLACES: bool = true;

    fn new_placement() -> Option<Box<dyn ErasedPlacement>> {
        Some(erased::typed::<L>(Box::<P>::default()))
    }
}

/// How far the fade of the current placement has come in `frame`: 0 as it
/// starts, 1 and past once it is done.
#[must_use]
pub fn fade(frame: &Frame<'_>) -> f32 {
    frame.placement_fade
}

/// `renderer`'s placement pass, if it is a `P`, for its layer type's own
/// API to reach.
#[must_use]
pub fn pass<P: 'static>(renderer: &Renderer) -> Option<&P> {
    renderer.placement()?.pass().downcast_ref()
}

/// [`pass`], to change.
pub fn pass_mut<P: 'static>(renderer: &mut Renderer) -> Option<&mut P> {
    renderer.placement_mut()?.pass_mut().downcast_mut()
}

/// The placement pass of the layer type `T`: the capability of a type whose
/// layers place. The engine makes one pass per renderer, and drives it on
/// its schedule.
pub trait PlacementPass<T: Layer>: Any {
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
    /// frame's update after the schedule placed or didn't. A bucket it
    /// changes uploads again, into the buffers its meshes read.
    fn update_buckets(&mut self, cx: &mut PlacementContext<'_, T>);

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
pub struct PlacementContext<'a, T: Layer> {
    /// The layers of the type that the frame draws, bottom to top.
    pub layers: &'a [PlacingLayer<'a, T>],
    /// The update the frame draws.
    pub update_parameters: &'a UpdateParameters,
    /// The buckets of the frame's tiles, which the layers' updates found.
    pub buckets: PlacementBuckets<'a>,
}

/// A layer that a [`PlacementPass`] places: its values, and its frame's
/// tiles that hold a bucket of it, whether that bucket draws or not.
pub struct PlacingLayer<'a, T: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<T::Style>,
    /// The tiles, the deeper zooms first, then from the top of the screen
    /// at the camera's bearing to its bottom, and from its left to its
    /// right: the order the layer places them in.
    pub tiles: &'a [PlacingTile],
}

/// One of a [`PlacingLayer`]'s tiles: where its bucket of the layer is, and
/// where the frame draws it.
#[derive(Clone)]
pub struct PlacingTile {
    /// Where the bucket is among the frame's tiles.
    pub bucket: BucketKey,
    /// The tile in the world copy the frame draws it in.
    pub id: UnwrappedTileID,
    /// The tile's id.
    pub overscaled_id: OverscaledTileID,
    /// Whether the frame no longer needs the tile, which it keeps while
    /// what placement showed of it fades out.
    pub held_for_fade: bool,
    /// How the frame projects the tile.
    pub projection: ProjectionData,
    /// The tile's feature index, which queries of what the pass places
    /// search.
    pub feature_index: Arc<FeatureIndex>,
}

/// The order a placing layer draws and places the frame's `tiles`, seen at
/// `bearing`, in radians: the deeper zooms first, then by y and by x, each
/// rotated by the bearing.
pub(crate) fn tile_order(tiles: &[RenderTile], bearing: f32) -> Vec<usize> {
    let rotation = Vec2::from_angle(bearing);
    let mut order: Vec<usize> = (0..tiles.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&tiles[a].id.canonical, &tiles[b].id.canonical);
        let pa = rotation.rotate(Vec2::new(a.x as f32, a.y as f32));
        let pb = rotation.rotate(Vec2::new(b.x as f32, b.y as f32));
        b.z.cmp(&a.z).then(pa.y.total_cmp(&pb.y)).then(pa.x.total_cmp(&pb.x))
    });
    order
}

/// Adds to `out` the tiles of a placing layer among `found`, the layer's
/// frame tiles of `source`, whose tiles hold `contents`: those that hold a
/// bucket of it.
pub(crate) fn placing_tiles(
    source: Option<&dyn RenderSource>,
    contents: &[Option<TileContent<'_>>],
    found: &[FoundTile],
    out: &mut Vec<PlacingTile>,
) {
    let Some((source, key)) = source.and_then(|source| Some((source, source.tiles()?))) else {
        return;
    };
    let tiles = source.render_tiles();
    out.extend(found.iter().filter_map(|found| {
        let Some(ViewPlace::Laid(index)) = found.view else { return None };
        let tile = tiles.get(found.tile)?;
        let Some(TileContent::Tile(content)) = contents.get(found.tile).copied().flatten() else { return None };
        let content = content.geometry()?;
        Some(PlacingTile {
            bucket: BucketKey {
                bucket: content.result.layer_render_data.bucket_index(index)?,
                slot: tile.slot(),
                tile: *tile.overscaled_tile_id(),
                source: key,
            },
            id: tile.id,
            overscaled_id: *tile.overscaled_tile_id(),
            held_for_fade: tile.hold_for_fade(),
            projection: tile.projection,
            feature_index: Arc::clone(&content.result.feature_index),
        })
    }));
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
/// its placement finds it by ([`PlacementBuckets`]).
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

/// The buckets of the frame's tiles as a [`PlacementPass`] reads and
/// changes them, by their [`BucketKey`]s.
pub struct PlacementBuckets<'a> {
    store: &'a mut dyn LaidBuckets,
}

impl<'a> PlacementBuckets<'a> {
    pub(crate) fn new(store: &'a mut dyn LaidBuckets) -> Self {
        PlacementBuckets { store }
    }

    /// The bucket at `key`, if it is there and is a `B`.
    #[must_use]
    pub fn get<B: Bucket>(&self, key: &BucketKey) -> Option<&B> {
        self.store.bucket(key)?.downcast_ref()
    }

    /// [`get`](Self::get), to change: the bucket's kept vectors ([`keep`])
    /// that the pass changes upload again, into the buffers its meshes read.
    pub fn get_mut<B: Bucket>(&mut self, key: &BucketKey) -> Option<&mut B> {
        let bucket = self.store.bucket_mut(key)?;
        bucket.changed();
        bucket.downcast_mut()
    }
}

/// Names `vec` to `up`, as [`Upload::vec`] does, keeping its data, which
/// the pass changes ([`GpuVec::get_mut`]) in the buckets it takes
/// ([`PlacementBuckets::get_mut`]): the engine then uploads it again into
/// the same buffer. A tile's world copies each have a buffer of their own
/// of it. Its length stays the one it first uploaded with, as the buffer's
/// and its meshes' do.
pub fn keep<V: bytemuck::Pod>(up: &mut Upload<'_>, vec: &mut GpuVec<V>) {
    if let crate::bucket_upload::Kind::Geometry { geometry, copies, res } = &mut up.kind {
        geometry.upload(vec, true, copies, res);
    }
}

/// Names `indexes` to `up`, as [`Upload::indexes`] does, keeping them, as
/// [`keep`] keeps a vector: the pass orders them anew ([`Indexes::get_mut`]).
pub fn keep_indexes(up: &mut Upload<'_>, indexes: &mut Indexes) {
    if let crate::bucket_upload::Kind::Geometry { geometry, copies, res } = &mut up.kind {
        geometry.upload_indexes(indexes, true, copies, res);
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
