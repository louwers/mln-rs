//! The values a layer keeps of the frame's tiles, which the engine keeps in
//! step with the frame.

use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;

/// A layer's values of the frame's tiles, which the engine keeps: a value
/// `V` of each tile, from when the layer makes one, in the order the layer
/// draws the tiles.
///
/// A layer reaches a tile's value by the tile's index among its tiles. The
/// engine moves a value with its tile when the frame's tiles change or the
/// map wraps, and drops it when the tile leaves the frame. Before a layer
/// of a source makes its draws, the engine drops the value of each tile
/// that shows it nothing to draw, and of each whose bucket or contents
/// another replaced.
pub(crate) struct TileMap<V> {
    tiles: Vec<Entry<V>>,
    /// The entries of the tiles before [`set_tiles`](Self::set_tiles), whose
    /// memory the next change reuses.
    previous: Vec<Entry<V>>,
    /// The ids of `previous`, sorted, each with its index there.
    by_id: Vec<(OverscaledTileID, usize)>,
}

/// A tile, the bucket or contents its value was made from, and the value.
struct Entry<V> {
    id: OverscaledTileID,
    /// `None` until the engine first sees the tile show the layer something
    /// to draw.
    made_from: Option<SimpleIdentity>,
    value: Option<V>,
}

impl<V> Default for TileMap<V> {
    fn default() -> Self {
        TileMap { tiles: Vec::new(), previous: Vec::new(), by_id: Vec::new() }
    }
}

impl<V> TileMap<V> {
    /// Makes `tiles`, in the order the layer draws them, the tiles the map
    /// keeps values of: it keeps the values of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        std::mem::swap(&mut self.tiles, &mut self.previous);
        self.by_id.clear();
        self.by_id.extend(self.previous.iter().enumerate().map(|(index, entry)| (entry.id, index)));
        // A frame's tens of tiles: the stable sort allocates nothing.
        self.by_id.sort_by_key(|&(id, _)| id);
        for id in tiles {
            let kept = self.by_id.binary_search_by_key(&id, |&(id, _)| id).ok().map(|at| &mut self.previous[self.by_id[at].1]);
            let (made_from, value) = kept.map_or((None, None), |entry| (entry.made_from, entry.value.take()));
            self.tiles.push(Entry { id, made_from, value });
        }
        self.previous.clear();
    }

    /// Drops the tiles and their values.
    pub(crate) fn clear_tiles(&mut self) {
        self.tiles.clear();
    }

    /// Moves the tiles' values to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        for entry in &mut self.tiles {
            entry.id = entry.id.unwrap_to(entry.id.wrap.wrapping_add(wrap_delta));
        }
    }

    /// The id of the tile at `index`.
    #[must_use]
    pub(crate) fn id(&self, index: usize) -> Option<OverscaledTileID> {
        self.tiles.get(index).map(|entry| entry.id)
    }

    /// Records what the tile at `index` shows the layer to draw: `data`, the
    /// identity of its bucket or contents, or `None` for nothing. The tile's
    /// value drops for nothing, and where `data` replaced the bucket or
    /// contents it was made from.
    pub(crate) fn show(&mut self, index: usize, data: Option<SimpleIdentity>) {
        let entry = &mut self.tiles[index];
        match data {
            None => entry.value = None,
            Some(data) if entry.made_from != Some(data) => {
                if entry.made_from.is_some() {
                    entry.value = None;
                }
                entry.made_from = Some(data);
            }
            Some(_) => {}
        }
    }

    /// The value of the tile at `index`, if the layer made one.
    #[must_use]
    pub(crate) fn get(&self, index: usize) -> Option<&V> {
        self.tiles.get(index)?.value.as_ref()
    }

    /// The number of tiles.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.tiles.len()
    }

    /// The value of the tile at `index`.
    ///
    /// # Panics
    ///
    /// If `index` is out of range.
    pub(crate) fn get_mut(&mut self, index: usize) -> &mut Option<V> {
        &mut self.tiles[index].value
    }

    /// Drops the tiles' values. The tiles stay.
    pub(crate) fn clear(&mut self) {
        for entry in &mut self.tiles {
            entry.value = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::TileMap;

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

        tiles.show(0, Some(other_bucket));
        assert_eq!(None, get(&mut tiles, &tile));

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
    /// overflows, and as other tiles come and go, and goes when the tile
    /// leaves the frame or the map is cleared.
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
