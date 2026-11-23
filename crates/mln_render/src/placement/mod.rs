//! Placement: which of the frame's marks show, each where it collides with
//! nothing that shows already, and how what shows fades between placements.
//!
//! A layer type whose layers place registers a participant, a
//! [`Placement`], with
//! [`RegistryBuilder::add_placing_layer_type`](crate::RegistryBuilder::add_placing_layer_type).
//! Each renderer makes one of each type's, keeps it on the renderer's
//! thread, and calls it for every layer of the type that the frame draws and
//! that [places](Placement::places). Nothing of it crosses the worker codec.
//!
//! The engine keeps the schedule: each frame it decides whether to place
//! again, keeps the clock that what shows fades on, and makes the
//! placement's [`CollisionIndex`]. A placement visits every placing layer of
//! every type, topmost first, one call per layer, so a layer's shapes give
//! way to those of the layers above it. Without the map's cross-source
//! collisions, they give way only to those of layers on their source. Each
//! layer's call places shapes of the features of the tiles whose bucket it
//! leads: a layer whose layout another layer of its group leads draws that
//! layer's bucket and places nothing of its own. In tile mode, a first round
//! visits every layer for the shapes that cross their tile's borders
//! ([`Placement::place_crossing`]), so neighboring tiles agree on them, and
//! a second round visits every layer for the rest.
//!
//! A frame of a continuous map places when its schedule is due and placing
//! would show something else: the view, the map mode, the style's
//! transitions, cross-source collisions, the collision debug option, the
//! layers' buckets or their layouts changed, or a participant
//! [says so](Prepared::changed). A still image places every frame. What a placement inserts in its index stands
//! for an [`Item`], a feature of a tile; rendered-feature queries of the
//! placing layers return the features of the current placement's shapes
//! that the query touches.
//!
//! A placing layer draws the tiles that its source holds while what
//! placement showed of them fades out, besides those of the frame, and a
//! tile's world copy doesn't draw it before the copy's own layout lands,
//! since its bucket holds what placement made of it where its tile is.

mod erased;
mod grid;
mod index;
mod run;
mod schedule;

use std::sync::Arc;

use glam::Vec2;
use mln_map_types::{MapMode, ProjectionData, View};
use mln_style::layer_type::LayerStyle;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};

pub(crate) use self::erased::{ErasedFrame, ErasedPlacement, LaidBuckets, erase};
pub use self::grid::{Aabb, Shape};
pub use self::index::{CollisionIndex, Item, Overlap};
pub(crate) use self::run::Placements;
#[cfg(feature = "bench")]
pub(crate) use self::run::{PlacingFrameLayer, place_round};
pub(crate) use self::schedule::Schedule;
use crate::layer::{Bucket, Layer, LayerValues};
use crate::render_tile::RenderTile;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// A placing layer type's part of placement: per layer, it turns the
/// features of the layer's tiles into [`Shape`]s, shows those that collide
/// with nothing in the placement's [`CollisionIndex`] and inserts them, and
/// it keeps what each of its items showed as, which its buckets draw.
///
/// Each frame, the engine first [prepares](Self::prepare) every type. When
/// the schedule places, it then [begins](Self::begin) the placement for
/// each type that has a layer leading a tile's bucket, calls
/// [`place`](Self::place) for each such layer of every type, topmost first
/// (in tile mode after [`place_crossing`](Self::place_crossing) for each of
/// them), and [commits](Self::commit) it for each type that began it. Then,
/// each frame, it has every type [`update`](Self::update) its buckets. A type
/// that has nothing to place [forgets](Self::forget) in place of placing,
/// and a still image's frame has every type forget after it placed.
pub trait Placement: Default + 'static {
    /// The layer type whose layers place.
    type Layer: Layer;

    /// Whether a layer of `style` places. A layer that doesn't is drawn,
    /// copied to world copies and queried as a layer of a type without
    /// placement. It is read from the style, so a change to it lays the
    /// layer out again.
    #[must_use]
    fn places(_style: &LayerStyle<<Self::Layer as Layer>::Style>) -> bool {
        true
    }

    /// Readies the frame's placement, before the schedule decides whether
    /// to place: brings what the participant keeps of its layers' buckets
    /// up to date with `cx`'s, and says what of its own inputs changed
    /// since the current placement.
    fn prepare(&mut self, _cx: &mut PlacementCx<'_, Self::Layer>) -> Prepared {
        Prepared::default()
    }

    /// Starts a placement, which replaces the current one when it commits.
    fn begin(&mut self, _cx: &mut PlaceCx<'_>) {}

    /// Tile mode's first round: places what of `layer`'s items crosses its
    /// tile's borders, or a neighbor's, so that the tiles on either side
    /// agree on it. [`place`](Self::place) then places the rest.
    fn place_crossing(&mut self, _layer: &PlacingLayer<'_, Self::Layer>, _cx: &mut PlaceCx<'_>) {}

    /// Places `layer`'s items: each shows where its shapes collide with
    /// nothing in `cx.index`, which it then inserts them in. The layer's
    /// [`tiles`](PlacingLayer::tiles) are those whose bucket it leads.
    fn place(&mut self, layer: &PlacingLayer<'_, Self::Layer>, cx: &mut PlaceCx<'_>);

    /// Ends the placement: it is now the current one. Adds to
    /// [`cx.placed`](PlaceCx::placed) what the placement changed that
    /// [`place`](Self::place) didn't add.
    fn commit(&mut self, _cx: &mut PlaceCx<'_>) {}

    /// Brings `cx`'s layers' buckets to the current placement, each frame
    /// after the schedule placed or didn't. A bucket it changes uploads
    /// again, into the buffers its meshes read.
    fn update(&mut self, _cx: &mut PlacementCx<'_, Self::Layer>) {}

    /// Forgets what the participant keeps of its layers' buckets from frame
    /// to frame: the renderer's tiles went, the frame is a still image's,
    /// which keeps none, or the type has nothing to place. With `placement`,
    /// it forgets the current placement too: a continuous map has nothing
    /// of the type to place, so what it showed is gone.
    fn forget(&mut self, placement: bool);
}

/// What a [`Placement`] reads in [`prepare`](Placement::prepare) and
/// [`update`](Placement::update): its type's layers that the frame draws,
/// the frame's update and the buckets of its tiles.
pub struct PlacementCx<'a, L: Layer> {
    /// The layers of the type that the frame draws and that place, bottom
    /// to top, each with every tile that holds a bucket of it.
    pub layers: &'a [PlacingLayer<'a, L>],
    /// The update the frame draws.
    pub update_parameters: &'a UpdateParameters,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
    /// The current placement's index.
    placed: &'a mut CollisionIndex,
}

impl<'a, L: Layer> PlacementCx<'a, L> {
    pub(crate) fn new(
        layers: &'a [PlacingLayer<'a, L>],
        update_parameters: &'a UpdateParameters,
        buckets: PlacementBuckets<'a>,
        placed: &'a mut CollisionIndex,
    ) -> Self {
        PlacementCx { layers, update_parameters, buckets, placed }
    }

    /// Has queries of the current placement return the features of the
    /// bucket at `key` in the reverse of `order`, the feature indexes in
    /// the order the bucket now draws them, rather than by their
    /// [`Item::sort`]. Does nothing where the placement placed nothing of
    /// the bucket as it is now.
    pub fn set_query_order(&mut self, key: &BucketKey, order: Option<Arc<[u32]>>) {
        self.placed.set_query_order(&*self.buckets.store, key, order);
    }
}

/// What a [`Placement`] reads and changes while it places.
pub struct PlaceCx<'a> {
    /// The placement's shapes so far, which those placed now may not
    /// collide with. Its shapes are in the collision group of the source of
    /// the layer being placed.
    pub index: &'a mut CollisionIndex,
    /// The view placed in.
    pub view: &'a View,
    pub mode: MapMode,
    /// How far the fade of the placement this one replaces had come: 0 as
    /// it started, 1 and past once it was done. A continuous map's
    /// opacities fade on from where they were by as much.
    pub fade: f32,
    /// Whether the map's debug options show collision shapes.
    pub collision_boxes: bool,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
    /// What the placement changed of what shows, which every participant
    /// adds to.
    pub placed: Placed,
    /// The records of the candidates a tile-mode placement decided, while
    /// the renderer keeps them
    /// ([`Renderer::record_placements`](crate::Renderer::record_placements)).
    pub records: Option<&'a mut Vec<PlacementRecord>>,
}

/// A layer that a [`Placement`] places: its values and its tiles.
pub struct PlacingLayer<'a, L: Layer> {
    /// The layer's values.
    pub layer: &'a LayerValues<L::Style>,
    /// The frame's tiles that hold a bucket of the layer: first those
    /// whose bucket the layer leads, and then, in [`PlacementCx`], those
    /// whose bucket another layer of its layout leads, which the layer
    /// draws. Within each, the deeper zooms come first, then the tiles from
    /// the top of the screen at the camera's bearing to its bottom, and
    /// from its left to its right: the order the layer places them in.
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

/// What a [`Placement::prepare`] says of the participant's own inputs: the
/// engine compares the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Whether the layers' buckets include some the participant hadn't
    /// seen, which a continuous map with a flat view places soon.
    pub added: bool,
    /// Whether placing now would place from other inputs than the current
    /// placement did, which a continuous map places again for.
    pub changed: bool,
}

/// What the schedule of a continuous map reads of a placement. A still
/// image shows its placement at once, and reads neither.
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

/// A placement that changed nothing and is settled, which participants add
/// to.
impl Default for Placed {
    fn default() -> Self {
        Placed { changed: false, settled: true }
    }
}

/// A candidate that a tile-mode placement decided, for checks that the
/// tiles on either side of a border agree on what crosses it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementRecord {
    /// The layer placed, or the layer that leads its bucket.
    pub layer: String,
    /// What the candidate shows, which names it across tiles, such as a
    /// label's text.
    pub key: String,
    /// Which of the candidate's parts this is, such as `"text"` or
    /// `"icon"`.
    pub part: &'static str,
    /// The part's bounds in the collision index's grid.
    pub bounds: Aabb,
    /// The index's [padding](CollisionIndex::padding): where the viewport's
    /// top-left corner is in the grid.
    pub padding: f32,
    /// Whether the part shows.
    pub placed: bool,
    /// Whether the part crosses its tile's borders, or a neighbor's.
    pub crosses_border: bool,
}

/// Where a bucket of a frame's layer is among the frame's tiles, which
/// a [`Placement`] finds it by ([`PlacementBuckets`]).
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

/// The buckets of the frame's tiles as a [`Placement`] reads and changes
/// them, by their [`BucketKey`]s.
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

    /// [`get`](Self::get), to change: the bucket's kept vectors
    /// ([`Upload::keep`](crate::layer::Upload::keep))
    /// that the participant changes upload again, into the buffers its
    /// meshes read.
    pub fn get_mut<B: Bucket>(&mut self, key: &BucketKey) -> Option<&mut B> {
        let bucket = self.store.bucket_mut(key)?;
        bucket.changed();
        bucket.downcast_mut()
    }
}
