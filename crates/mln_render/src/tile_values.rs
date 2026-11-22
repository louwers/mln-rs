//! The values a layer keeps of the frame's tiles, which the engine keeps in
//! step with the frame.
//!
//! The engine keeps a [`TileMap`] for each layer: a slot for each of the
//! frame's tiles of the layer, holding the layer type's value of the tile
//! ([`LayerType::Tile`](crate::layer::LayerType::Tile)), if it made one,
//! with the bucket the value was made from and what of that bucket changed
//! since. The slots are in tile id order, and a layer reaches them in the
//! order it draws its tiles, through a permutation from that order to the
//! slots. Each frame's update gives the map the frame's tiles: the values
//! of the tiles that stay move with them, and those of the tiles that left
//! go; a wrap jump moves them to their tiles' new ids.

use std::ops::BitOrAssign;

use mln_tile_id::OverscaledTileID;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

/// What of a bucket's vertex data changed since a tile's draws copied it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketChanges {
    /// The values of the paint properties that read feature state.
    pub paint: bool,
    /// The opacities of what a bucket places, which placement sets.
    pub opacity: bool,
    /// The positions that placement lays out, such as those of labels along
    /// lines or at variable anchors.
    pub dynamic: bool,
    /// The order of the bucket's triangles, which placement sorts.
    pub sort: bool,
}

impl BucketChanges {
    /// Everything changed.
    pub const ALL: BucketChanges = BucketChanges { paint: true, opacity: true, dynamic: true, sort: true };

    /// Whether anything changed.
    #[must_use]
    pub fn any(self) -> bool {
        self != BucketChanges::default()
    }
}

impl BitOrAssign for BucketChanges {
    fn bitor_assign(&mut self, other: BucketChanges) {
        self.paint |= other.paint;
        self.opacity |= other.opacity;
        self.dynamic |= other.dynamic;
        self.sort |= other.sort;
    }
}

/// The buckets whose vertex data changed, by bucket id, with what changed,
/// whose draws copy it again.
#[derive(Default)]
pub struct ChangedBuckets(HashMap<SimpleIdentity, BucketChanges>);

impl ChangedBuckets {
    /// Records that `changes` of the bucket `bucket` changed.
    pub fn mark(&mut self, bucket: SimpleIdentity, changes: BucketChanges) {
        if changes.any() {
            *self.0.entry(bucket).or_default() |= changes;
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, bucket: SimpleIdentity) -> Option<BucketChanges> {
        self.0.get(&bucket).copied()
    }
}

/// What the engine keeps of a tile of a layer, whatever the layer's type.
#[derive(Default)]
struct TileState {
    /// The bucket or contents the tile's value was made from, `None` until
    /// the engine first sees the tile show the layer something to draw.
    bucket: Option<SimpleIdentity>,
    /// What of the bucket's vertex data changed since the value copied it.
    changes: BucketChanges,
}

/// The frame's tiles of a layer, each with its [`TileState`] at its slot:
/// the part of a [`TileMap`] that doesn't depend on the layer's values.
#[derive(Default)]
struct TileSet {
    /// The tiles' ids, sorted: a tile's slot is its index here.
    ids: Vec<OverscaledTileID>,
    /// Each tile's state, at its slot.
    states: Vec<TileState>,
    /// The tiles' ids in the order the layer draws them.
    drawn: Vec<OverscaledTileID>,
    /// The slot of each tile of `drawn`, at its index there.
    slots: Vec<usize>,
    /// How the tiles' slots last changed.
    moves: TileMoves,
    /// The ids that `set_tiles` makes the set's; after it, the ids the set
    /// had, whose memory the next change reuses.
    incoming: Vec<OverscaledTileID>,
}

/// How the slots of a set's tiles change, as swaps of the values at them
/// that move the values of the tiles that stay to their new slots.
#[derive(Default)]
struct TileMoves {
    /// The swaps: `swaps[..split]` before the values beyond `kept` drop, and
    /// the others after the new tiles' values fill the slots up to `len`.
    swaps: Vec<(usize, usize)>,
    split: usize,
    kept: usize,
    len: usize,
}

impl TileMoves {
    /// Moves `values`, one at each slot of the set's tiles before the
    /// change, to their tiles' slots after it, drops those of the tiles
    /// that left, and gives the new tiles `fill()`.
    fn apply<V>(&self, values: &mut Vec<V>, fill: impl FnMut() -> V) {
        let (before, after) = self.swaps.split_at(self.split);
        for &(a, b) in before {
            values.swap(a, b);
        }
        values.truncate(self.kept);
        values.resize_with(self.len, fill);
        for &(a, b) in after {
            values.swap(a, b);
        }
    }
}

impl TileSet {
    /// Makes `drawn` the set's tiles: keeps the states of those it has, and
    /// drops the others. Returns whether that changed any tile's slot, which
    /// `moves` then says. The slots of `drawn` are found again either way.
    fn set_tiles(&mut self) -> bool {
        self.incoming.clone_from(&self.drawn);
        // The stable sort, which the renderer's other sorts of tile ids
        // share, allocates nothing for a frame's tens of tiles.
        self.incoming.sort();
        self.incoming.dedup();
        let moved = self.ids != self.incoming;
        if moved {
            let (ids, incoming, moves) = (&self.ids, &self.incoming, &mut self.moves);
            moves.swaps.clear();
            // The tiles that stay move to the front, in order, ahead of
            // those that leave...
            let mut kept = 0;
            for (old, _) in kept_tiles(ids, incoming) {
                if kept != old {
                    moves.swaps.push((kept, old));
                }
                kept += 1;
            }
            moves.split = moves.swaps.len();
            moves.kept = kept;
            moves.len = incoming.len();
            // ...and then, from the last to the first, to their new slots,
            // whose values are the new tiles' until then.
            for (rank, (_, new)) in kept_tiles(ids, incoming).enumerate() {
                if rank != new {
                    moves.swaps.push((new, rank));
                }
            }
            moves.swaps[moves.split..].reverse();
            moves.apply(&mut self.states, TileState::default);
            std::mem::swap(&mut self.ids, &mut self.incoming);
        }
        self.find_slots();
        moved
    }

    /// Moves the tiles' states to the tiles `wrap_delta` world copies east
    /// of theirs. Returns whether that changed their slots, which `moves`
    /// then says; only a wrap that overflows does.
    fn shift_wraps(&mut self, wrap_delta: i16) -> bool {
        for id in self.ids.iter_mut().chain(&mut self.drawn) {
            *id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
        }
        if self.ids.is_sorted() {
            return false;
        }
        // The shifted ids, in their old order.
        self.incoming.clone_from(&self.ids);
        self.ids.sort();
        // For each tile's old slot, its new one. A wrap overflows rarely
        // enough that this allocates.
        let mut new_slot: Vec<usize> =
            self.incoming.iter().map(|id| self.ids.binary_search(id).unwrap_or_default()).collect();
        let moves = &mut self.moves;
        moves.swaps.clear();
        for old in 0..new_slot.len() {
            while new_slot[old] != old {
                let new = new_slot[old];
                moves.swaps.push((old, new));
                new_slot.swap(old, new);
            }
        }
        moves.split = moves.swaps.len();
        moves.kept = self.ids.len();
        moves.len = self.ids.len();
        moves.apply(&mut self.states, TileState::default);
        self.find_slots();
        true
    }

    /// Finds the slot of each tile of `drawn`.
    fn find_slots(&mut self) {
        let ids = &self.ids;
        self.slots.clear();
        self.slots.extend(self.drawn.iter().map(|id| ids.binary_search(id).unwrap_or_default()));
        debug_assert!(self.slots_are_a_permutation(), "a layer draws a tile twice, or a tile without a slot");
    }

    /// Whether each tile of `drawn` has its own slot, and each slot is a
    /// drawn tile's: the layer draws each tile once.
    fn slots_are_a_permutation(&self) -> bool {
        let mut taken = vec![false; self.ids.len()];
        self.slots.len() == self.ids.len()
            && self
                .slots
                .iter()
                .zip(&self.drawn)
                .all(|(&slot, id)| self.ids.get(slot) == Some(id) && !std::mem::replace(&mut taken[slot], true))
    }

    fn clear(&mut self) {
        self.ids.clear();
        self.states.clear();
        self.drawn.clear();
        self.slots.clear();
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for state in &mut self.states {
            if let Some(changes) = state.bucket.and_then(|bucket| changed.get(bucket)) {
                state.changes |= changes;
            }
        }
    }
}

/// The tiles of both `old` and `new`, sorted ids, each with its index in
/// `old` and in `new`, in order.
fn kept_tiles<'a>(
    old: &'a [OverscaledTileID],
    new: &'a [OverscaledTileID],
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let mut old_index = 0;
    new.iter().enumerate().filter_map(move |(new_index, id)| {
        while old.get(old_index).is_some_and(|old_id| old_id < id) {
            old_index += 1;
        }
        (old.get(old_index) == Some(id)).then_some((old_index, new_index))
    })
}

/// A layer's values of the frame's tiles, which the engine keeps: a value
/// `V` of each tile, from when the layer makes one.
///
/// A layer reaches a tile's value by the tile's index among the tiles it
/// draws, in the order it draws them: the order of the frame's tiles it
/// gets, or of the tiles a layer without a source picks. The engine moves a
/// value with its tile when the frame's tiles change or the map wraps, and
/// drops it when the tile leaves the frame. Before a layer of a source makes
/// its draws, the engine drops the value of each tile that shows it nothing
/// to draw, and of each whose bucket or contents another replaced.
pub struct TileMap<V> {
    set: TileSet,
    /// Each tile's value, at its slot.
    values: Vec<Option<V>>,
}

impl<V> Default for TileMap<V> {
    fn default() -> Self {
        TileMap { set: TileSet::default(), values: Vec::new() }
    }
}

impl<V> TileMap<V> {
    /// Makes `tiles`, in the order the layer draws them, the tiles the map
    /// keeps values of: it keeps the values of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        self.set.drawn.clear();
        self.set.drawn.extend(tiles);
        if self.set.set_tiles() {
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Drops the tiles and their values.
    pub(crate) fn clear_tiles(&mut self) {
        self.set.clear();
        self.values.clear();
    }

    /// Moves the tiles' values to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        if self.set.shift_wraps(wrap_delta) {
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Adds `changed` to the changes of the tiles whose values were made
    /// from a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.set.mark_changed(changed);
    }

    /// The id of the tile at `index` of those the layer draws.
    #[must_use]
    pub(crate) fn id(&self, index: usize) -> Option<OverscaledTileID> {
        self.set.drawn.get(index).copied()
    }

    /// Records what the tile at `index` shows the layer to draw: `data`, the
    /// identity of its bucket or contents, or `None` for nothing. The tile's
    /// value drops for nothing, and where `data` replaced the bucket or
    /// contents it was made from, which drops the changes of the one
    /// before.
    pub(crate) fn show(&mut self, index: usize, data: Option<SimpleIdentity>) {
        let slot = self.set.slots[index];
        let Some(data) = data else {
            self.values[slot] = None;
            return;
        };
        let state = &mut self.set.states[slot];
        if state.bucket == Some(data) {
            return;
        }
        if state.bucket.is_some() {
            self.values[slot] = None;
        }
        state.bucket = Some(data);
        state.changes = BucketChanges::default();
    }

    /// The number of tiles the layer draws.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set.slots.len()
    }

    /// Whether the layer draws no tiles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.slots.is_empty()
    }

    /// The value of the tile at `index` of those the layer draws.
    ///
    /// # Panics
    ///
    /// If `index` is out of range.
    pub fn get_mut(&mut self, index: usize) -> &mut Option<V> {
        &mut self.values[self.set.slots[index]]
    }

    /// What of the bucket of the tile at `index` of those the layer draws
    /// changed since its value last copied it, which it is about to copy
    /// again.
    ///
    /// # Panics
    ///
    /// If `index` is out of range.
    pub fn take_changes(&mut self, index: usize) -> BucketChanges {
        std::mem::take(&mut self.set.states[self.set.slots[index]].changes)
    }

    /// Drops the tiles' values. The tiles stay.
    pub fn clear(&mut self) {
        self.values.fill_with(|| None);
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, TileMap};

    /// The index of `tile_id` among the tiles `tiles` draws.
    fn index_of<V>(tiles: &TileMap<V>, tile_id: &OverscaledTileID) -> Option<usize> {
        (0..tiles.len()).find(|&index| tiles.id(index) == Some(*tile_id))
    }

    /// Sets the value of `tile_id` in `tiles` to `value`, if the tile is one
    /// of the map's; returns whether it is.
    fn set<V>(tiles: &mut TileMap<V>, tile_id: &OverscaledTileID, value: V) -> bool {
        index_of(tiles, tile_id).map(|index| *tiles.get_mut(index) = Some(value)).is_some()
    }

    /// The value of `tile_id` in `tiles`.
    fn get<V: Copy>(tiles: &mut TileMap<V>, tile_id: &OverscaledTileID) -> Option<V> {
        let index = index_of(tiles, tile_id)?;
        *tiles.get_mut(index)
    }

    /// Records that `tile_id` shows a layer `bucket`.
    fn show<V>(tiles: &mut TileMap<V>, tile_id: &OverscaledTileID, bucket: SimpleIdentity) {
        let index = index_of(tiles, tile_id).expect("the tile is the map's");
        tiles.show(index, Some(bucket));
    }

    /// What of `tile_id`'s bucket changed.
    fn take_changes<V>(tiles: &mut TileMap<V>, tile_id: &OverscaledTileID) -> Option<BucketChanges> {
        Some(tiles.take_changes(index_of(tiles, tile_id)?))
    }

    /// A tile's changes are those of the bucket its value was made from,
    /// until the layer takes them; they follow the tile as it moves.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<()>::default();
        tiles.set_tiles([tile, other]);
        show(&mut tiles, &tile, bucket);
        show(&mut tiles, &other, other_bucket);

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);

        assert_eq!(
            Some(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }),
            take_changes(&mut tiles, &tile)
        );
        assert_eq!(Some(BucketChanges::default()), take_changes(&mut tiles, &tile));
        assert_eq!(Some(BucketChanges::default()), take_changes(&mut tiles, &other));

        let mut changed = ChangedBuckets::default();
        changed.mark(other_bucket, BucketChanges { opacity: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);
        tiles.set_tiles([other]);
        assert_eq!(None, index_of(&tiles, &tile));
        tiles.shift_wraps(1);
        let expected = BucketChanges { opacity: true, ..BucketChanges::default() };
        assert_eq!(Some(expected), take_changes(&mut tiles, &other.unwrap_to(1)));
    }

    /// A tile whose bucket another replaced, or that shows nothing to draw,
    /// drops its value; one that shows the same bucket again keeps it.
    #[test]
    fn another_bucket_drops_the_value() {
        let tile = OverscaledTileID::with_zxy(1, 0, 1, 1, 0);
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile]);
        tiles.show(0, Some(bucket));
        assert!(set(&mut tiles, &tile, 1));
        tiles.show(0, Some(bucket));
        assert_eq!(Some(1), get(&mut tiles, &tile));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);
        tiles.show(0, Some(other_bucket));
        assert_eq!(None, get(&mut tiles, &tile));
        assert_eq!(BucketChanges::default(), tiles.take_changes(0));

        assert!(set(&mut tiles, &tile, 2));
        tiles.show(0, None);
        assert_eq!(None, get(&mut tiles, &tile));
    }

    /// Each tile keeps its value as tiles before and after it come and go,
    /// and the layer reaches it at its index in whatever order the layer
    /// draws the tiles.
    #[test]
    fn values_stay_with_their_tiles() {
        let ids: Vec<OverscaledTileID> = (0..6).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([ids[4], ids[1], ids[2]]);
        for (index, value) in [4, 1, 2].into_iter().enumerate() {
            *tiles.get_mut(index) = Some(value);
        }
        tiles.set_tiles([ids[5], ids[2], ids[0], ids[4], ids[3]]);
        let drawn: Vec<Option<u32>> = (0..tiles.len()).map(|index| *tiles.get_mut(index)).collect();
        assert_eq!(vec![None, Some(2), None, Some(4), None], drawn);
        assert!(set(&mut tiles, &ids[0], 0));
        tiles.set_tiles([ids[4], ids[0]]);
        let values: Vec<Option<u32>> = ids.iter().map(|id| get(&mut tiles, id)).collect();
        assert_eq!(vec![Some(0), None, None, None, Some(4), None], values);
        assert_eq!(Some(4), *tiles.get_mut(0));
        assert_eq!(None, index_of(&tiles, &ids[2]));
    }

    /// Each tile keeps its value from any set of tiles to any other, in
    /// either order.
    #[test]
    fn values_stay_through_every_change() {
        let ids: Vec<OverscaledTileID> = (0..5).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let subsets = |mask: u32| ids.iter().enumerate().filter(move |(i, _)| mask & (1 << i) != 0).map(|(_, id)| *id);
        let mut tiles = TileMap::<usize>::default();
        for from in 0..32 {
            for to in 0..32 {
                tiles.clear_tiles();
                tiles.set_tiles(subsets(from));
                for (i, id) in ids.iter().enumerate() {
                    set(&mut tiles, id, i);
                }
                tiles.set_tiles(subsets(to).rev());
                let drawn: Vec<Option<usize>> = (0..tiles.len()).map(|index| *tiles.get_mut(index)).collect();
                let expected: Vec<Option<usize>> = (0..ids.len())
                    .rev()
                    .filter(|i| to & (1 << i) != 0)
                    .map(|i| (from & (1 << i) != 0).then_some(i))
                    .collect();
                assert_eq!(expected, drawn, "from {from:05b} to {to:05b}");
            }
        }
    }

    /// A tile's value moves with it across a wrap jump, even one that
    /// changes the tiles' slots, and as other tiles come and go, and goes
    /// when the tile leaves the frame or the map is cleared.
    #[test]
    fn values_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile, other]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(set(&mut tiles, &other, 2));
        assert!(!set(&mut tiles, &OverscaledTileID::with_zxy(1, 0, 1, 1, 1), 3));

        tiles.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert_eq!(Some(1), get(&mut tiles, &moved));
        assert_eq!(Some(1), *tiles.get_mut(0));
        tiles.set_tiles([moved]);
        assert_eq!(None, get(&mut tiles, &other.unwrap_to(1)));
        tiles.set_tiles([other, moved]);
        assert_eq!(None, get(&mut tiles, &other));
        assert_eq!(Some(1), get(&mut tiles, &moved));
        tiles.set_tiles([moved]);
        assert_eq!(Some(1), get(&mut tiles, &moved));

        // A wrap that overflows reorders the slots; the layer still reaches
        // each tile's value at the tile's index.
        let (high, low) = (tile.unwrap_to(i16::MAX), other.unwrap_to(i16::MAX - 1));
        tiles.set_tiles([high, low, tile]);
        for (index, value) in [3, 4, 5].into_iter().enumerate() {
            *tiles.get_mut(index) = Some(value);
        }
        tiles.shift_wraps(1);
        assert_eq!(Some(3), get(&mut tiles, &tile.unwrap_to(i16::MIN)));
        assert_eq!(Some(4), get(&mut tiles, &other.unwrap_to(i16::MAX)));
        assert_eq!(Some(5), get(&mut tiles, &tile.unwrap_to(1)));
        let drawn: Vec<Option<u32>> = (0..tiles.len()).map(|index| *tiles.get_mut(index)).collect();
        assert_eq!(vec![Some(3), Some(4), Some(5)], drawn);
        assert_eq!(Some(tile.unwrap_to(i16::MIN)), tiles.id(0));

        tiles.clear();
        assert_eq!(Some(1), index_of(&tiles, &other.unwrap_to(i16::MAX)));
        assert_eq!(None, get(&mut tiles, &other.unwrap_to(i16::MAX)));
    }
}
