//! What the engine keeps of a layer's tiles between frames: its value of
//! each tile it draws, a value of the layer's type, in a [`TileMap`], which
//! also says what the tile's value was made from and what of that bucket's
//! vertex data changed since. The frame and every drape the layer draws in
//! draw the same values.

use std::ops::BitOrAssign;

use mln_style::hash::HashMap;

use crate::draw::identity::SimpleIdentity;
use crate::tile::OverscaledTileID;
/// What of a bucket's vertex data changed since a tile's draws copied it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BucketChanges {
    /// The values of the paint properties that read feature state.
    pub(crate) paint: bool,
    /// A symbol bucket's opacities, which placement sets.
    pub(crate) opacity: bool,
    /// A symbol bucket's positions of labels along lines or at variable
    /// anchors.
    pub(crate) dynamic: bool,
    /// The order of a symbol bucket's triangles.
    pub(crate) sort: bool,
}

impl BucketChanges {
    pub(crate) const ALL: BucketChanges = BucketChanges { paint: true, opacity: true, dynamic: true, sort: true };

    /// Whether anything changed.
    #[must_use]
    pub(crate) fn any(self) -> bool {
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

/// The buckets whose vertex data changed, by bucket id, with what changed.
#[derive(Default)]
pub(crate) struct ChangedBuckets(HashMap<SimpleIdentity, BucketChanges>);

impl ChangedBuckets {
    /// Records that `changes` of the bucket `bucket` changed.
    pub(crate) fn mark(&mut self, bucket: SimpleIdentity, changes: BucketChanges) {
        if changes.any() {
            *self.0.entry(bucket).or_default() |= changes;
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the bucket `bucket` changed.
    #[must_use]
    pub(crate) fn contains(&self, bucket: SimpleIdentity) -> bool {
        self.0.contains_key(&bucket)
    }

    fn get(&self, bucket: SimpleIdentity) -> Option<BucketChanges> {
        self.0.get(&bucket).copied()
    }
}

/// What the tile showed a layer that its value was made from: the
/// identity of its bucket, or of its raster or raster-dem contents, with the
/// generations of their mask or mesh and of their DEM's border.
pub(crate) type MadeFrom = (SimpleIdentity, [u64; 2]);

/// What the engine keeps of a tile a layer draws, whatever the layer's type.
#[derive(Default)]
struct TileState {
    /// What the tile's value was made from, `None` until the engine first
    /// sees the tile show the layer something.
    made_from: Option<MadeFrom>,
    /// What of the bucket's vertex data changed since the draws copied it.
    changes: BucketChanges,
}

/// The frame's tiles a layer draws, in id order, each with its
/// [`TileState`]: the part of a [`TileMap`] that doesn't depend on what the
/// layer keeps of a tile.
#[derive(Default)]
struct TileSet {
    /// The tiles' ids, in order.
    ids: Vec<OverscaledTileID>,
    /// Each tile's state, at the tile's index.
    states: Vec<TileState>,
    /// How the tiles' indexes last changed.
    moves: TileMoves,
    /// The ids that `set_tiles` makes the set's; after it, the ids the set
    /// had, whose memory the next change reuses.
    incoming: Vec<OverscaledTileID>,
    /// The index after the tile that [`find`](Self::find) last found,
    /// where it looks first.
    next: usize,
}

/// How the indexes of a set's tiles change, as swaps of the values at them
/// that move the values of the tiles that stay to their new indexes.
#[derive(Default)]
struct TileMoves {
    /// The swaps: `swaps[..split]` before the values beyond `kept` drop, and
    /// the others after the new tiles' values fill the indexes up to `len`.
    swaps: Vec<(usize, usize)>,
    split: usize,
    kept: usize,
    len: usize,
}

impl TileMoves {
    /// Moves `values`, one at each index of the set's tiles before the
    /// change, to their tiles' indexes after it, drops those of the tiles
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
    /// Makes `incoming` the set's tiles: keeps the states of those it has,
    /// and drops the others. Returns whether that changed any tile's index,
    /// which `moves` then says.
    fn set_tiles(&mut self) -> bool {
        // The stable sort, which the renderer's other sorts of tile ids
        // share, allocates nothing for a frame's tens of tiles.
        self.incoming.sort();
        self.incoming.dedup();
        if self.ids == self.incoming {
            return false;
        }
        let (ids, incoming, moves) = (&self.ids, &self.incoming, &mut self.moves);
        moves.swaps.clear();
        // The tiles that stay move to the front, in order, ahead of those
        // that leave...
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
        // ...and then, from the last to the first, to their new indexes,
        // whose values are the new tiles' until then.
        for (rank, (_, new)) in kept_tiles(ids, incoming).enumerate() {
            if rank != new {
                moves.swaps.push((new, rank));
            }
        }
        moves.swaps[moves.split..].reverse();
        moves.apply(&mut self.states, TileState::default);
        std::mem::swap(&mut self.ids, &mut self.incoming);
        true
    }

    /// Moves the tiles' states to the tiles `wrap_delta` world copies east
    /// of theirs. Returns whether that changed their order, which `moves`
    /// then says; only a wrap that overflows does.
    fn shift_wraps(&mut self, wrap_delta: i16) -> bool {
        for id in &mut self.ids {
            *id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
        }
        if self.ids.is_sorted() {
            return false;
        }
        // The shifted ids, in their old order.
        self.incoming.clone_from(&self.ids);
        self.ids.sort();
        // For each tile's old index, its new one. A wrap overflows rarely
        // enough that this allocates.
        let mut new_index: Vec<usize> =
            self.incoming.iter().map(|id| self.ids.binary_search(id).unwrap_or_default()).collect();
        let moves = &mut self.moves;
        moves.swaps.clear();
        for old in 0..new_index.len() {
            while new_index[old] != old {
                let new = new_index[old];
                moves.swaps.push((old, new));
                new_index.swap(old, new);
            }
        }
        moves.split = moves.swaps.len();
        moves.kept = self.ids.len();
        moves.len = self.ids.len();
        moves.apply(&mut self.states, TileState::default);
        true
    }

    /// The index of `tile_id`, if it is one of the set's.
    fn index_of(&self, tile_id: &OverscaledTileID) -> Option<usize> {
        self.ids.binary_search(tile_id).ok()
    }

    /// [`index_of`](Self::index_of), looking first at the index after the
    /// tile it last found: the engine readies a layer's tile values and the
    /// layer then makes its draws of them in the same order, the tiles' id
    /// order for most layer types.
    fn find(&mut self, tile_id: &OverscaledTileID) -> Option<usize> {
        let index = match self.ids.get(self.next) {
            Some(id) if id == tile_id => self.next,
            _ => self.index_of(tile_id)?,
        };
        self.next = index + 1;
        Some(index)
    }

    fn clear(&mut self) {
        self.ids.clear();
        self.states.clear();
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for state in &mut self.states {
            if let Some(changes) = state.made_from.and_then(|(bucket, _)| changed.get(bucket)) {
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

/// A layer's values of the frame's tiles, which the engine keeps, by tile id:
/// what each tile's value was made from and what of its bucket changed
/// since, and the value, a `T` of the layer's type, from when the layer
/// makes one ([`TypedLayer::tile`]).
///
/// [`TypedLayer::tile`]: crate::layer::render_layer::TypedLayer::tile
pub(crate) struct TileMap<T> {
    set: TileSet,
    /// Each tile's `T`, at the tile's index in the set.
    values: Vec<Option<T>>,
}

impl<T> Default for TileMap<T> {
    fn default() -> Self {
        TileMap { set: TileSet::default(), values: Vec::new() }
    }
}

impl<T> TileMap<T> {
    /// Makes `tiles`, the frame's tiles of the layer in any order, those the
    /// map has entries for: it keeps the entries of those it had, and drops
    /// the others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        self.set.incoming.clear();
        self.set.incoming.extend(tiles);
        if self.set.set_tiles() {
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Whether the map has no tiles.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.set.ids.is_empty()
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.set.clear();
        self.values.clear();
    }

    /// Moves the tiles' entries to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        if self.set.shift_wraps(wrap_delta) {
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Adds `changed` to the changes of the tiles whose values were made from
    /// a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.set.mark_changed(changed);
    }

    /// Drops the tiles' values. The tiles' entries stay.
    pub(crate) fn clear(&mut self) {
        self.values.fill_with(|| None);
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.set.index_of(tile_id).is_some()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get(&self, tile_id: &OverscaledTileID) -> Option<&T> {
        self.values.get(self.set.index_of(tile_id)?)?.as_ref()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    pub(crate) fn get_mut(&mut self, tile_id: &OverscaledTileID) -> Option<&mut T> {
        self.values.get_mut(self.set.find(tile_id)?)?.as_mut()
    }

    /// Drops what the layer keeps of `tile_id`.
    pub(crate) fn drop_value(&mut self, tile_id: &OverscaledTileID) {
        if let Some(value) = self.set.find(tile_id).and_then(|index| self.values.get_mut(index)) {
            *value = None;
        }
    }

    /// The tiles, in order, each with its entry.
    pub(crate) fn entries(&mut self) -> impl Iterator<Item = (OverscaledTileID, TileEntry<'_, T>)> {
        let TileSet { ids, states, .. } = &mut self.set;
        ids.iter().zip(states).zip(&mut self.values).map(|((&id, state), value)| (id, TileEntry { state, value }))
    }

    /// The entry of `tile_id`, if the tile is one of the frame's.
    pub(crate) fn entry(&mut self, tile_id: &OverscaledTileID) -> Option<TileEntry<'_, T>> {
        let index = self.set.find(tile_id)?;
        Some(TileEntry { state: self.set.states.get_mut(index)?, value: self.values.get_mut(index)? })
    }
}

/// The entry of a tile in a [`TileMap`].
pub(crate) struct TileEntry<'a, T> {
    state: &'a mut TileState,
    /// What the layer keeps of the tile, if anything.
    pub(crate) value: &'a mut Option<T>,
}

impl<T> TileEntry<'_, T> {
    /// What the tile's value was made from, if the engine saw it show the
    /// layer something.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn made_from(&self) -> Option<MadeFrom> {
        self.state.made_from
    }

    /// Records what the tile shows the layer: `made_from`, or `None` for
    /// nothing. The tile's value drops for nothing, and where it was made
    /// from something else, which drops the changes of what it was made
    /// from too.
    pub(crate) fn show(&mut self, made_from: Option<MadeFrom>) {
        match made_from {
            None => *self.value = None,
            Some(made_from) if self.state.made_from != Some(made_from) => {
                if self.state.made_from.is_some() {
                    *self.value = None;
                }
                self.state.made_from = Some(made_from);
                self.state.changes = BucketChanges::default();
            }
            Some(_) => {}
        }
    }

    /// What of the tile's bucket changed since its value last copied it,
    /// which the layer is about to copy again.
    pub(crate) fn take_changes(&mut self) -> BucketChanges {
        std::mem::take(&mut self.state.changes)
    }
}

#[cfg(test)]
mod tests {
    use super::{BucketChanges, ChangedBuckets, TileMap};
    use crate::draw::identity::SimpleIdentity;
    use crate::tile::OverscaledTileID;

    /// Sets what `tiles` keeps of `tile_id` to `value`, if the tile is one
    /// of the map's; returns whether it is.
    fn set<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID, value: T) -> bool {
        tiles.entry(tile_id).map(|entry| *entry.value = Some(value)).is_some()
    }

    /// The bucket that the value of `tile_id` in `tiles` was made from.
    fn bucket_of<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        Some(tiles.entry(tile_id)?.made_from()?.0)
    }

    /// Records that `tile_id` shows `bucket` in `tiles`; returns whether the
    /// tile is one of the map's.
    fn show<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID, bucket: SimpleIdentity) -> bool {
        tiles.entry(tile_id).map(|mut entry| entry.show(Some((bucket, [0, 0])))).is_some()
    }

    /// A tile's changes are those of the bucket its draws were built from,
    /// until the draws take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<()>::default();
        tiles.set_tiles([tile, other]);
        assert!(show(&mut tiles, &tile, bucket));
        assert!(show(&mut tiles, &tile, bucket));
        assert!(show(&mut tiles, &other, other_bucket));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);

        let mut take = |tile_id| tiles.entry(tile_id).map(|mut entry| entry.take_changes());
        assert_eq!(Some(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }), take(&tile));
        assert_eq!(Some(BucketChanges::default()), take(&tile));
        assert_eq!(Some(BucketChanges::default()), take(&other));

        tiles.set_tiles([other]);
        assert!(!tiles.has_tile(&tile));
        assert_eq!(Some(other_bucket), bucket_of(&mut tiles, &other));
        tiles.shift_wraps(1);
        assert_eq!(Some(other_bucket), bucket_of(&mut tiles, &other.unwrap_to(1)));
    }

    /// A tile that shows another bucket, or bucket of another generation,
    /// drops its value and the changes of the bucket before; one that shows
    /// nothing drops its value.
    #[test]
    fn another_bucket_drops_the_value() {
        let tile = OverscaledTileID::with_zxy(1, 0, 1, 1, 0);
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(show(&mut tiles, &tile, bucket));
        assert_eq!(Some(&1), tiles.get(&tile));
        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);
        assert!(show(&mut tiles, &tile, other_bucket));
        assert!(tiles.get(&tile).is_none());
        assert_eq!(Some(BucketChanges::default()), tiles.entry(&tile).map(|mut entry| entry.take_changes()));

        assert!(set(&mut tiles, &tile, 2));
        tiles.entry(&tile).unwrap().show(Some((other_bucket, [1, 0])));
        assert!(tiles.get(&tile).is_none());
        assert!(set(&mut tiles, &tile, 3));
        tiles.entry(&tile).unwrap().show(None);
        assert!(tiles.get(&tile).is_none());
    }

    /// Each tile keeps its value and bucket as tiles before and after it
    /// come and go, and in whatever order the frame gives the tiles.
    #[test]
    fn values_stay_with_their_tiles() {
        let ids: Vec<OverscaledTileID> = (0..6).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([ids[4], ids[1], ids[2]]);
        for &i in &[1, 2, 4] {
            assert!(set(&mut tiles, &ids[i], i as u32));
        }
        let bucket = SimpleIdentity::new();
        assert!(show(&mut tiles, &ids[2], bucket));
        tiles.set_tiles([ids[5], ids[2], ids[0], ids[4], ids[3]]);
        let values: Vec<Option<u32>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
        assert_eq!(vec![None, None, Some(2), None, Some(4), None], values);
        assert_eq!(Some(bucket), bucket_of(&mut tiles, &ids[2]));
        assert!(set(&mut tiles, &ids[0], 0));
        tiles.set_tiles([ids[4], ids[0], ids[4]]);
        let values: Vec<Option<u32>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
        assert_eq!(vec![Some(0), None, None, None, Some(4), None], values);
        assert!(!tiles.has_tile(&ids[2]));
    }

    /// Each tile keeps its value and bucket from any set of tiles to any
    /// other.
    #[test]
    fn values_stay_through_every_change() {
        let ids: Vec<OverscaledTileID> = (0..5).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let buckets: Vec<SimpleIdentity> = ids.iter().map(|_| SimpleIdentity::new()).collect();
        let subsets = |mask: u32| ids.iter().enumerate().filter(move |(i, _)| mask & (1 << i) != 0).map(|(_, id)| *id);
        let mut tiles = TileMap::<usize>::default();
        for from in 0..32 {
            for to in 0..32 {
                tiles.clear_tiles();
                tiles.set_tiles(subsets(from));
                for (i, id) in ids.iter().enumerate() {
                    show(&mut tiles, id, buckets[i]);
                    set(&mut tiles, id, i);
                }
                tiles.set_tiles(subsets(to).rev());
                let values: Vec<Option<usize>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
                let expected: Vec<Option<usize>> =
                    (0..ids.len()).map(|i| (from & to & (1 << i) != 0).then_some(i)).collect();
                assert_eq!(expected, values, "from {from:05b} to {to:05b}");
                let kept_buckets: Vec<Option<SimpleIdentity>> =
                    ids.iter().map(|id| bucket_of(&mut tiles, id)).collect();
                let expected: Vec<Option<SimpleIdentity>> =
                    (0..ids.len()).map(|i| (from & to & (1 << i) != 0).then_some(buckets[i])).collect();
                assert_eq!(expected, kept_buckets, "from {from:05b} to {to:05b}");
            }
        }
    }

    /// What the layer keeps of a tile moves with the tile across a wrap jump,
    /// even one that changes the tiles' order, and as other tiles come and
    /// go, and goes when the tile leaves the frame or the map is cleared.
    #[test]
    fn caches_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile, other]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(set(&mut tiles, &other, 2));
        assert!(!set(&mut tiles, &OverscaledTileID::with_zxy(1, 0, 1, 1, 1), 3));

        tiles.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert_eq!(Some(&mut 1), tiles.get_mut(&moved));
        tiles.set_tiles([moved]);
        assert!(tiles.get_mut(&other.unwrap_to(1)).is_none());
        tiles.set_tiles([other, moved]);
        assert!(tiles.get_mut(&other).is_none());
        assert_eq!(Some(&1), tiles.get(&moved));
        tiles.set_tiles([moved]);
        assert_eq!(Some(&1), tiles.get(&moved));

        let (high, low) = (tile.unwrap_to(i16::MAX), other.unwrap_to(i16::MAX - 1));
        tiles.set_tiles([high, low, tile]);
        assert!(set(&mut tiles, &high, 3));
        assert!(set(&mut tiles, &low, 4));
        assert!(set(&mut tiles, &tile, 5));
        tiles.shift_wraps(1);
        assert_eq!(Some(&3), tiles.get(&tile.unwrap_to(i16::MIN)));
        assert_eq!(Some(&4), tiles.get(&other.unwrap_to(i16::MAX)));
        assert_eq!(Some(&5), tiles.get(&tile.unwrap_to(1)));

        tiles.clear();
        assert!(tiles.has_tile(&other.unwrap_to(i16::MAX)));
        assert!(tiles.get(&other.unwrap_to(i16::MAX)).is_none());
    }
}
