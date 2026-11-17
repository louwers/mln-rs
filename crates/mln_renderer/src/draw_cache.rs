//! What the renderer keeps of each layer's drawing between frames.
//!
//! The orchestrator owns a [`LayerDraws`] for each render layer and lends it
//! to the layer's calls. Its drawables hold the GPU objects they draw with,
//! which are made again only when what they draw changes: each tile's entry
//! says which bucket the tile's drawables were built from, and what of that
//! bucket's vertex data changed since, as the frame's prepare reports it.

use std::ops::BitOrAssign;

use mln_tile_id::OverscaledTileID;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

use crate::layer_group::LayerGroup;

/// What of a bucket's vertex data changed since a tile's drawables copied it.
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

    fn get(&self, bucket: SimpleIdentity) -> Option<BucketChanges> {
        self.0.get(&bucket).copied()
    }
}

/// What the renderer keeps of a tile a layer draws.
#[derive(Clone, Copy, Debug, Default)]
struct TileEntry {
    /// The bucket the tile's drawables were built from, `None` until a
    /// layer's update sets it.
    bucket: Option<SimpleIdentity>,
    /// What of the bucket's vertex data changed since the drawables copied
    /// it.
    changes: BucketChanges,
}

/// What the renderer keeps of a layer's drawing between frames: the group of
/// its drawables, and an entry for each of the frame's tiles it draws.
#[derive(Default)]
pub(crate) struct LayerDraws {
    /// The group of the layer's drawables, which draws while the layer is
    /// renderable.
    pub(crate) group: Option<LayerGroup>,
    tiles: HashMap<OverscaledTileID, TileEntry>,
}

impl LayerDraws {
    /// Makes `tiles`, the frame's tiles of the layer, those the layer has
    /// entries for: it keeps the entries of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        let previous = std::mem::take(&mut self.tiles);
        self.tiles = tiles.into_iter().map(|id| (id, previous.get(&id).copied().unwrap_or_default())).collect();
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.tiles.clear();
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.tiles.contains_key(tile_id)
    }

    /// The bucket the drawables of `tile_id` were built from, if the tile is
    /// one of the frame's and they were.
    #[must_use]
    pub(crate) fn bucket(&self, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        self.tiles.get(tile_id).and_then(|entry| entry.bucket)
    }

    /// Records that the drawables of `tile_id` are built from `bucket`, which
    /// drops the changes of the bucket they were built from before. Returns
    /// whether that changed it; a tile that isn't one of the frame's takes no
    /// bucket.
    pub(crate) fn set_bucket(&mut self, tile_id: &OverscaledTileID, bucket: SimpleIdentity) -> bool {
        match self.tiles.get_mut(tile_id) {
            Some(entry) if entry.bucket != Some(bucket) => {
                *entry = TileEntry { bucket: Some(bucket), changes: BucketChanges::default() };
                true
            }
            _ => false,
        }
    }

    /// What of the bucket of `tile_id` changed since its drawables last
    /// copied it, which they are about to copy again.
    pub(crate) fn take_changes(&mut self, tile_id: &OverscaledTileID) -> BucketChanges {
        self.tiles.get_mut(tile_id).map(|entry| std::mem::take(&mut entry.changes)).unwrap_or_default()
    }

    /// Adds `changed` to the changes of the tiles whose drawables were built
    /// from a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for entry in self.tiles.values_mut() {
            if let Some(changes) = entry.bucket.and_then(|bucket| changed.get(bucket)) {
                entry.changes |= changes;
            }
        }
    }

    /// Moves the tiles' entries and drawables to the tiles `wrap_delta`
    /// world copies east of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        self.tiles = std::mem::take(&mut self.tiles)
            .into_iter()
            .map(|(id, entry)| (id.unwrap_to(id.wrap.wrapping_add(wrap_delta)), entry))
            .collect();
        if let Some(group) = &mut self.group {
            group.shift_wraps(wrap_delta);
        }
    }

    /// Removes the drawables, and returns how many there were. The tiles'
    /// entries stay.
    pub(crate) fn clear_drawables(&mut self) -> usize {
        self.group.as_mut().map_or(0, LayerGroup::clear_drawables)
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, LayerDraws};

    /// A tile's changes are those of the bucket its drawables were built
    /// from, until the drawables take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut draws = LayerDraws::default();
        draws.set_tiles([tile, other]);
        assert!(draws.set_bucket(&tile, bucket));
        assert!(!draws.set_bucket(&tile, bucket));
        assert!(draws.set_bucket(&other, other_bucket));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        draws.mark_changed(&changed);

        assert_eq!(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }, draws.take_changes(&tile));
        assert_eq!(BucketChanges::default(), draws.take_changes(&tile));
        assert_eq!(BucketChanges::default(), draws.take_changes(&other));

        draws.set_tiles([other]);
        assert!(!draws.has_tile(&tile));
        assert_eq!(Some(other_bucket), draws.bucket(&other));
        draws.shift_wraps(1);
        assert_eq!(Some(other_bucket), draws.bucket(&other.unwrap_to(1)));
    }
}
