//! The placement contract: which of the frame's labels and other marks
//! show, each where it collides with nothing placed before it, and how they
//! fade between placements. A tile-mode placement can record what it placed
//! ([`PlacedSymbolData`]).
//!
//! A layer type whose layers place registers a participant, a [`Placement`],
//! with
//! [`RegistryBuilder::add_placing_layer_type::<P>()`](crate::RegistryBuilder::add_placing_layer_type),
//! which registers its layer type too. Each renderer makes one of each
//! type's and calls it for the layers of the type that the frame draws.
//!
//! The engine owns what is the same for every participant: the schedule,
//! which decides when to place and keeps the clock that what shows fades
//! on; each placement's [`CollisionIndex`], with its grids, padding, overlap
//! rule ([`Overlap`]), collision groups and the query data of the buckets
//! whose shapes it holds; the order the layers place in; committing a
//! placement; and tile mode's records. A participant owns what its type
//! places: it turns its layers' features into [`Shape`]s, decides which
//! show, and keeps what each showed as, which its buckets draw. It reads and
//! changes the frame's buckets by their [`BucketKey`]s
//! ([`PlacementBuckets`], [`LaidBuckets`]), and where terrain draws, stands
//! its marks on the ground the frame drew
//! ([`TerrainElevation`]).
//!
//! Each frame, the engine calls every participant:
//!
//! 1. [`collect`](Placement::collect), in the frame's update of each layer
//!    of its type, once the layer's own update found its tiles.
//! 2. [`prepare`](Placement::prepare), with the layers that place, which
//!    brings what the participant keeps up to date with the frame's buckets
//!    and says whether its own inputs changed since the current placement.
//! 3. When the schedule places: [`begin`](Placement::begin); in tile mode
//!    [`place_crossing`](Placement::place_crossing); for each layer of every
//!    type, topmost first, [`place`](Placement::place); and then
//!    [`commit`](Placement::commit). In a continuous map, a type with
//!    nothing to place forgets its buckets and its placement instead.
//! 4. [`update`](Placement::update), which brings its buckets to the
//!    current placement.
//!
//! A still image places every frame, and every type forgets its buckets after
//! it. The layers place topmost first, so a layer's shapes give way to those
//! of the layers above it, whatever their types. Without the map's
//! cross-source collisions, they give way only to those of layers on their
//! source ([`PlacingLayer::group`]).
//!
//! A continuous map places when its schedule is due and placing would show
//! something else: the view, the map mode, the style's transitions,
//! cross-source collisions or the ground changed; the sources hold tiles that
//! wait for placement to fade them out; what showed was still fading; or a
//! participant says so ([`Prepared::changed`]).

mod grid;
mod index;
mod run;
mod schedule;

use std::time::Duration;

pub use self::grid::{Aabb, Shape};
pub use self::index::{CollisionGroup, CollisionIndex, Item, Overlap, RetainedQueryData};
pub(crate) use self::run::{ErasedPlacement, Placements, participant};
#[cfg(feature = "bench")]
pub(crate) use self::run::{place_alone, placing_layers};
pub(crate) use self::schedule::{PlacementController, PlacementInputs};
use crate::camera::View;
use crate::draw::identity::SimpleIdentity;
use crate::layer::bucket::{Bucket, ErasedBucket};
pub use crate::layer::layer_drawing::ChangedBuckets;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::render_layer::Layer;
use crate::map::MapMode;
use crate::render::terrain::TerrainElevation;
use crate::render::tile::FrameTile;
use crate::tile::OverscaledTileID;
use crate::worker::job::SourceKey;

/// A placing layer type's part of placement. Per layer, it turns the
/// features of the layer's tiles into [`Shape`]s, shows those that collide
/// with nothing in the placement's [`CollisionIndex`] and inserts them, and
/// it keeps what each of its items showed as, which its buckets draw. The
/// module's comment says when the engine calls each hook.
pub trait Placement: Default + 'static {
    /// The layer type whose layers place.
    type Layer: Layer;

    /// Collects what the participant places and draws of `tiles`, the
    /// frame's tiles of `layer` in the order the layer draws them, each with
    /// what it holds for the layer. Returns whether the layer places: the
    /// engine then gives it to [`place`](Self::place) by `layer.key`. What
    /// the participant collected is the frame's; it drops the last frame's
    /// in [`begin_frame`](Self::begin_frame).
    fn collect(&mut self, layer: &CollectCx<'_, Self::Layer>, tiles: &[FrameTile<'_, Self::Layer>]) -> bool;

    /// Drops what the participant collected of the last frame's layers,
    /// before the frame's layers update.
    fn begin_frame(&mut self);

    /// Readies the frame's placement, before the schedule decides whether
    /// to place: brings what the participant keeps of `layers`, those of its
    /// layers that place, bottom to top, up to date with the frame's
    /// buckets, and says what of its own inputs changed since the current
    /// placement.
    fn prepare(&mut self, layers: &[PlacingLayer], cx: &mut PrepareCx<'_>) -> Prepared;

    /// Begins a placement: in a continuous map it places against the
    /// current one, which it replaces; in a still image against none.
    fn begin(&mut self, cx: &mut PlaceCx<'_>);

    /// Tile mode's first round: places what of `layers`' items crosses the
    /// borders of its tile, or a neighbor's, so that the tiles on either
    /// side agree on it. [`place`](Self::place) then places the rest.
    fn place_crossing(&mut self, _layers: &[PlacingLayer], _cx: &mut PlaceCx<'_>) {}

    /// Places `layer`'s items: each shows where its shapes collide with
    /// nothing of the layer's [group](PlacingLayer::group) in `cx.index`,
    /// which it then inserts them in.
    fn place(&mut self, layer: &PlacingLayer, cx: &mut PlaceCx<'_>);

    /// Ends the placement: it is now the current one. Returns what it
    /// changed of what shows.
    fn commit(&mut self, cx: &mut CommitCx<'_>) -> Placed;

    /// Brings the buckets of the frame's layers of the type to the current
    /// placement, each frame after the schedule placed or didn't.
    fn update(&mut self, cx: &mut UpdateCx<'_>);

    /// Forgets what the participant keeps of its layers' buckets from frame
    /// to frame: the renderer's tiles went, or the frame is a still image's,
    /// which keeps none.
    fn forget_buckets(&mut self);

    /// Forgets the current placement: the engine dropped it, so what it
    /// showed is gone.
    fn forget_placement(&mut self);
}

/// A layer that a [`Placement`] collects in the frame's update.
pub struct CollectCx<'a, L: Layer> {
    /// What the engine gives the layer to [`Placement::place`] by.
    pub key: usize,
    /// The layer's values of the frame.
    pub values: &'a EvaluatedLayer<L::Style>,
    /// The tiles of the layer's source, if it has tiles.
    pub source: Option<SourceKey>,
}

/// A layer that places in the frame, as the engine gives it to a
/// [`Placement`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacingLayer {
    /// The layer's [`CollectCx::key`].
    pub key: usize,
    /// The layer's position among the frame's placing layers of every
    /// type, topmost first, which the order of their shapes follows.
    pub position: usize,
    /// The shapes the layer's shapes collide with, and are in.
    pub group: CollisionGroup,
}

/// What a [`Placement::prepare`] reads and changes.
pub struct PrepareCx<'a> {
    /// The view the frame draws.
    pub view: &'a View,
    /// The ground the frame's marks stand on, where terrain draws.
    pub ground: Option<&'a TerrainElevation>,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
}

/// What a [`Placement`] reads and changes while it places.
pub struct PlaceCx<'a> {
    /// The placement's shapes so far, which those placed now may not
    /// collide with.
    pub index: &'a mut CollisionIndex,
    /// The view placed in.
    pub view: &'a View,
    /// The map's mode: in [`MapMode::Tile`], the placement has a crossing
    /// round ([`Placement::place_crossing`]) before the rest.
    pub mode: MapMode,
    /// The ground the marks stand on, where terrain draws.
    pub ground: Option<&'a TerrainElevation>,
    /// The buckets of the frame's tiles.
    pub buckets: &'a dyn LaidBuckets,
    /// The records of what a tile-mode placement placed, while the
    /// renderer collects them.
    pub records: Option<&'a mut Vec<PlacedSymbolData>>,
}

/// What a [`Placement::commit`] reads and changes.
pub struct CommitCx<'a> {
    /// The map's mode: a continuous map fades what shows from the placement
    /// before, a still image shows it at once.
    pub mode: MapMode,
    /// How far the fade of the placement this one replaces had come: 0 as
    /// it started, 1 and past once it was done.
    pub fade: f32,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
}

/// What a [`Placement::update`] reads and changes.
pub struct UpdateCx<'a> {
    /// The view the frame draws.
    pub view: &'a View,
    /// The map's mode.
    pub mode: MapMode,
    /// Whether the frame placed, or dropped the placement.
    pub placed: bool,
    /// The ground the current placement's marks stand on, where terrain
    /// draws.
    pub ground: Option<&'a TerrainElevation>,
    /// The current placement's index, whose query data the participant
    /// keeps up to date with how its buckets sort their features.
    pub index: &'a mut CollisionIndex,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
    /// The buckets whose vertices changed, which the draws that copied them
    /// copy again.
    pub changed: &'a mut ChangedBuckets,
}

/// What a [`Placement::prepare`] says of the participant's own inputs: the
/// engine compares the rest (the module's comment lists it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Whether the participant can place items it couldn't before, which in
    /// a flat view shortens the wait of a continuous map for its next
    /// placement to [`Prepared::SOON`].
    pub added: bool,
    /// Whether placing now would place from other inputs than the current
    /// placement did, which a continuous map places again for.
    pub changed: bool,
}

impl Prepared {
    /// How soon after a placement a flat view places again where a
    /// participant [added](Self::added) items: new items usually come from
    /// several tiles in a row, which the short wait places together.
    pub const SOON: Duration = Duration::from_millis(30);
}

/// What a placement changed of what shows, which the schedule of a
/// continuous map reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Whether what shows changed from the placement it replaces, which
    /// starts its fade.
    pub changed: bool,
    /// Whether everything that shows or hides was at the end of its fade
    /// when the placement was made. A placement that wasn't places again,
    /// once due, to bring it there.
    pub settled: bool,
}

/// A placement that changed nothing and is settled.
impl Default for Placed {
    fn default() -> Self {
        Placed { changed: false, settled: true }
    }
}

impl std::ops::BitOrAssign for Placed {
    /// The placement of both: changed where either changed, settled where
    /// both are.
    fn bitor_assign(&mut self, other: Placed) {
        self.changed |= other.changed;
        self.settled &= other.settled;
    }
}

/// What a tile-mode placement placed of a symbol, which a tool reads to
/// check that neighboring tiles agree.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSymbolData {
    /// Contents of the label
    pub key: String,
    /// If symbol contains text, text collision box in viewport coordinates
    pub text_collision_box: Option<Aabb>,
    /// If symbol contains icon, icon collision box in viewport coordinates
    pub icon_collision_box: Option<Aabb>,
    /// Symbol text was placed
    pub text_placed: bool,
    /// Symbol icon was placed
    pub icon_placed: bool,
    /// Symbol text or icon collision box intersects tile borders
    pub intersects_tile_border: bool,
    /// The viewport padding, which is where the tile's top-left corner is,
    /// in x and in y.
    pub viewport_padding: f32,
    /// Layer id (leader of the symbol layout group)
    pub layer: String,
}

/// Where a bucket is: its tile, in the tile store, with where the tile is
/// among its source's tiles as the frame's render tiles found it
/// ([`RenderTile::slot`](crate::render::tile::RenderTile::slot)), and its
/// index among the tile's buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BucketKey {
    pub bucket: usize,
    pub slot: Option<usize>,
    pub tile: OverscaledTileID,
    pub source: SourceKey,
}

/// The buckets of the frame's tiles, by key.
pub trait LaidBuckets {
    /// The identity of the bucket at `key`, which its draws' changes name.
    fn id(&self, key: &BucketKey) -> Option<SimpleIdentity>;
    fn bucket(&self, key: &BucketKey) -> Option<&dyn ErasedBucket>;
    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn ErasedBucket>;
}

impl dyn LaidBuckets + '_ {
    /// The bucket at `key`, if it is there and is a `B`.
    pub fn get<B: Bucket>(&self, key: &BucketKey) -> Option<&B> {
        self.bucket(key)?.downcast_ref()
    }
}

/// The buckets of the frame's tiles as a [`Placement`] reads and changes
/// them, by their [`BucketKey`]s.
pub struct PlacementBuckets<'a> {
    store: &'a mut dyn LaidBuckets,
}

impl<'a> PlacementBuckets<'a> {
    pub(crate) fn new(store: &'a mut dyn LaidBuckets) -> Self {
        PlacementBuckets { store }
    }

    /// The identity of the bucket at `key`, which its draws' changes name.
    #[must_use]
    pub fn id(&self, key: &BucketKey) -> Option<SimpleIdentity> {
        self.store.id(key)
    }

    /// The bucket at `key`, if it is there and is a `B`, to change.
    pub fn get_mut<B: Bucket>(&mut self, key: &BucketKey) -> Option<&mut B> {
        self.store.bucket_mut(key)?.downcast_mut()
    }

    /// The buckets, to read.
    #[must_use]
    pub fn store(&self) -> &dyn LaidBuckets {
        self.store
    }
}
