//! Placement: which of the frame's marks show, each where it collides with
//! nothing that shows already, and how what shows fades between placements.
//! The `mln_ngon` crate's `NgonPlacement`, which places the polygons of the
//! `org.maplibre.ngon` layers that collide, is a whole participant written
//! against the facade alone; the built-in symbol layers take part the same
//! way, so labels and n-gons collide with each other.
//!
//! # Participants
//!
//! A layer type whose layers place registers a participant, a
//! [`Placement`], with
//! [`RegistryBuilder::add_placing_layer_type`](crate::RegistryBuilder::add_placing_layer_type).
//! Each renderer makes one of each type's, keeps it on the renderer's
//! thread, and calls it for every layer of the type that the frame draws and
//! that [places](Placement::places). Nothing of it crosses the worker codec.
//!
//! The engine owns what is the same for every participant: the schedule,
//! which decides when to place and keeps the clock that what shows fades on;
//! each placement's [`CollisionIndex`], with its grids, padding, overlap
//! rule and collision groups; the order the layers place in; committing a
//! placement; the rendered-feature queries of the placing layers; and tile
//! mode's records. A participant owns what its type places: it turns its
//! layers' features into [`Shape`]s, decides which show, and keeps what
//! each of its items showed as, which its buckets draw.
//!
//! # A frame
//!
//! Each frame, the engine calls, for every participant:
//!
//! 1. [`prepare`](Placement::prepare), which brings what the participant
//!    keeps up to date with the frame's buckets and says whether its own
//!    inputs changed.
//! 2. When the schedule places: [`begin`](Placement::begin), for each type
//!    that has a layer leading a tile's bucket; for each such layer of every
//!    type, topmost first, [`place`](Placement::place), after
//!    [`place_crossing`](Placement::place_crossing) for each of them in tile
//!    mode; and [`commit`](Placement::commit), for each type that began.
//!    A type with nothing to place [forgets](Placement::forget) instead.
//! 3. [`update`](Placement::update), which brings its buckets to the
//!    current placement.
//!
//! A still image places every frame, and every type forgets after it.
//!
//! The layers place topmost first, so a layer's shapes give way to those of
//! the layers above it, whatever their types. Without the map's
//! cross-source collisions, they give way only to those of layers on their
//! source. Each layer's call places shapes of the features of the tiles
//! whose bucket it leads: a layer whose layout another layer of its group
//! leads draws that layer's bucket and places nothing of its own.
//!
//! A frame of a continuous map places when its schedule is due and placing
//! would show something else: the view, the map mode, the style's
//! transitions, cross-source collisions or the collision debug option
//! changed; a layer leads other tiles' buckets, from other layouts, or
//! their tiles draw elsewhere or are held for fade now; or a participant
//! [says so](Prepared::changed). So a participant compares only what of its
//! own decides where its shapes are, such as its layers' paint values.
//!
//! # Fades
//!
//! In a continuous map, what a placement shows fades in and what it hides
//! fades out. A participant keeps each item's [`Opacity`], steps it at each
//! placement by how far the fade of the one before had come
//! ([`PlaceCx::fade`]), [adds](Placed::add) it to what the placement
//! changed, and writes it into a vector its bucket keeps
//! ([`Upload::keep`](crate::layer::Upload::keep)) as a vertex value
//! ([`Opacity::vertex`]), which its shader fades on between placements by
//! the frame's [`placement_fade`](crate::layer::Frame::placement_fade).
//! Opacities kept in the bucket, in a field the worker codec skips
//! (`#[wire(skip)]`), go with their tile, and each world copy has its own.
//! A still image's fade is 1, so its opacities reach their end at once.
//!
//! A placing layer draws the tiles that its source holds while what
//! placement showed of them fades out, besides those of the frame, and a
//! tile's world copy doesn't draw it before the copy's own layout lands,
//! since its bucket holds what placement made of it where its tile is.
//!
//! # Queries
//!
//! What a placement inserts in its index stands for an [`Item`], a feature
//! of a tile. Rendered-feature queries of the placing layers return the
//! features of the current placement's shapes whose bounding box the query
//! touches, besides those that their layout indexed
//! ([`BuildCx::index`](crate::layer::BuildCx::index)), which a layer type
//! leaves out for the features it places.
//!
//! # Tile mode
//!
//! A still image of a tile places as far past the tile's borders as its
//! [`padding`](CollisionIndex::padding) reaches, so that neighboring tiles,
//! drawn apart, agree on what crosses them. A first round visits every
//! layer for the shapes that cross their tile's borders
//! ([`Placement::place_crossing`]), and a second round visits every layer
//! for the rest. What crosses is the participant's to say: n-gon projects
//! its tile's corners into the grid and places first the circles whose
//! bounds reach across the square they make. While the renderer keeps them
//! ([`Renderer::record_placements`](crate::Renderer::record_placements)),
//! participants record each candidate they decide
//! ([`PlaceCx::records`]), which checks that tiles agree read.

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

/// A placing layer type's part of placement: per layer, it turns the
/// features of the layer's tiles into [`Shape`]s, shows those that collide
/// with nothing in the placement's [`CollisionIndex`] and inserts them, and
/// it keeps what each of its items showed as, which its buckets draw. The
/// [module docs](self#a-frame) say when the engine calls each hook.
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
/// the frame's view and the buckets of its tiles.
pub struct PlacementCx<'a, L: Layer> {
    /// The layers of the type that the frame draws and that place, bottom
    /// to top, each with every tile that holds a bucket of it.
    pub layers: &'a [PlacingLayer<'a, L>],
    /// The view the frame draws.
    pub view: &'a View,
    /// The buckets of the frame's tiles.
    pub buckets: PlacementBuckets<'a>,
    /// The current placement's index.
    placed: &'a mut CollisionIndex,
}

impl<'a, L: Layer> PlacementCx<'a, L> {
    pub(crate) fn new(
        layers: &'a [PlacingLayer<'a, L>],
        view: &'a View,
        buckets: PlacementBuckets<'a>,
        placed: &'a mut CollisionIndex,
    ) -> Self {
        PlacementCx { layers, view, buckets, placed }
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
    /// The map's mode: in [`MapMode::Tile`], the placement has a crossing
    /// round ([`Placement::place_crossing`]) before the rest.
    pub mode: MapMode,
    /// How far the fade of the placement this one replaces had come: 0 as
    /// it started, 1 and past once it was done, and 1 in a still image. A
    /// continuous map's opacities fade on from where they were by as much;
    /// a still image's show at once.
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
    /// How the frame projects the tile: a
    /// [`TileProjector`](mln_map_types::TileProjector) of it projects the
    /// tile's points into the normalized device coordinates that
    /// [`CollisionIndex::to_grid`] takes.
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

/// What a [`Placement::prepare`] says of the participant's own inputs,
/// such as its layers' paint values: the engine compares the rest (the
/// [module docs](self) list it).
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

impl Placed {
    /// Adds an item whose opacity went from `prev`, the current
    /// placement's, or `None` for an item it didn't have, to `next`: the
    /// placement changed what shows where the item is placed now and
    /// wasn't, or the other way around, and it isn't settled unless `next`
    /// is. A still image's placement reads neither, so a participant may
    /// add its items in every mode.
    pub fn add(&mut self, prev: Option<Opacity>, next: Opacity) {
        self.changed |= prev.map_or(next.placed, |prev| prev.placed != next.placed);
        self.settled &= next.is_settled();
    }
}

/// An item's opacity from one placement of a continuous map to the next:
/// a placed item fades in, toward 1, and one not placed fades out, toward
/// 0, each placement by how far the fade of the one before had come
/// ([`PlaceCx::fade`]). Between placements, the shader fades it on by the
/// frame's [`placement_fade`](crate::layer::Frame::placement_fade).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opacity {
    /// From 0, hidden, to 1.
    pub opacity: f32,
    /// Whether the item is placed, and so fades in.
    pub placed: bool,
}

impl Opacity {
    /// The opacity of an item that the placement before didn't have,
    /// placed as `placed`: shown at once where it is placed and
    /// `skip_fade`, as an item that shows as its tile does, and otherwise
    /// at 0.
    #[must_use]
    pub fn new(placed: bool, skip_fade: bool) -> Self {
        Opacity { opacity: if skip_fade && placed { 1.0 } else { 0.0 }, placed }
    }

    /// The opacity faded on by `fade` toward where it went, now placed as
    /// `placed`.
    #[must_use]
    pub fn step(self, fade: f32, placed: bool) -> Self {
        let step = if self.placed { fade } else { -fade };
        Opacity { opacity: (self.opacity + step).clamp(0.0, 1.0), placed }
    }

    /// Whether the item has faded out.
    #[must_use]
    pub fn is_hidden(self) -> bool {
        self.opacity == 0.0 && !self.placed
    }

    /// Whether the opacity is at the end of its fade.
    #[must_use]
    pub fn is_settled(self) -> bool {
        self.opacity == if self.placed { 1.0 } else { 0.0 }
    }

    /// The opacity as one vertex attribute value: `opacity` in 1/127 steps,
    /// times 2, plus 1 where the item is placed. A shader reads the
    /// opacity as `floor(value / 2) / 127` and fades it on by the frame's
    /// [`placement_fade`](crate::layer::Frame::placement_fade), up where
    /// `value` is odd and down where it is even, clamped to 0 and 1.
    #[must_use]
    pub fn vertex(self) -> f32 {
        ((u32::from((self.opacity * 127.0) as u8) << 1) | u32::from(self.placed)) as f32
    }
}

/// A candidate that a tile-mode placement decided, for checks that the
/// tiles on either side of a border agree on what crosses it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementRecord {
    /// The layer placed, or the layer that leads its bucket.
    pub layer: String,
    /// What the candidate shows, such as a label's text. Checks match a
    /// candidate across tiles by its key, part and bounds, so a candidate
    /// that its bounds name may leave it empty.
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

#[cfg(test)]
mod tests {
    use super::{Opacity, Placed};

    #[test]
    fn an_opacity_fades_toward_where_it_goes() {
        let shown = Opacity::new(true, true);
        assert_eq!(shown.opacity, 1.0);
        assert!(shown.is_settled());
        let faded = Opacity::new(false, false).step(0.5, false);
        assert_eq!(faded.opacity, 0.0);
        assert!(faded.is_hidden() && faded.is_settled());
        let fading_in = Opacity::new(true, false).step(0.25, true);
        assert_eq!(fading_in.opacity, 0.25);
        assert!(!fading_in.is_settled());
        assert_eq!(Opacity { opacity: 1.0, placed: true }.vertex(), 255.0);
        assert_eq!(Opacity { opacity: 0.5, placed: false }.vertex(), 126.0);
    }

    #[test]
    fn a_placement_changes_where_an_item_shows_or_hides_anew() {
        let (shown, hidden) = (Opacity::new(true, true), Opacity::new(false, true));
        let mut placed = Placed::default();
        placed.add(Some(shown), shown.step(1.0, true));
        placed.add(None, hidden);
        assert_eq!(placed, Placed::default(), "nothing shows or hides anew");
        placed.add(Some(shown), shown.step(0.5, false));
        assert_eq!(placed, Placed { changed: true, settled: false });
    }
}
