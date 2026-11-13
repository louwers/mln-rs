// C++ logs `RenderTileAction`s with a reference to the tile object and
// compares them by address. The port's `update_renderables` names tiles by
// their storage key, so the mock gives every MockTileData a serial number and
// `TileRef` (key + serial) stands in for the address: a tile re-created under
// the same key compares unequal, like a new allocation in C++.

use std::collections::{BTreeMap, BTreeSet};

use mln_renderer::algorithm::update_renderables::{PrefetchedTile, RenderablesContext, update_renderables};
use mln_renderer::tile::TileNecessity;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::range::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupResult {
    NotFound,
    Found,
}
use LookupResult::*;

/// The address of a `MockTileData`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TileRef {
    tile_id: OverscaledTileID,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum ActionLogEntry {
    GetTileDataAction(OverscaledTileID, LookupResult),
    CreateTileDataAction(OverscaledTileID),
    RetainTileDataAction(OverscaledTileID, TileNecessity),
    RenderTileAction(UnwrappedTileID, TileRef),
}
use ActionLogEntry::*;

fn oid(overscaled_z: u8, wrap: i16, (z, x, y): (u8, u32, u32)) -> OverscaledTileID {
    OverscaledTileID::with_zxy(overscaled_z, wrap, z, x, y)
}

fn uid(z: u8, x: i64, y: i64) -> UnwrappedTileID {
    UnwrappedTileID::new(z, x, y)
}

/// mock.hpp `MockTileData`.
struct MockTileData {
    renderable: bool,
    tried_optional: bool,
    loaded: bool,
    tile_id: OverscaledTileID,
    serial: u64,
}

impl MockTileData {
    fn new(tile_id: OverscaledTileID, next_serial: &mut u64) -> Self {
        *next_serial += 1;
        MockTileData { renderable: false, tried_optional: false, loaded: false, tile_id, serial: *next_serial }
    }
}

/// mock.hpp `MockSource`, plus the test's `ActionLog log`.
struct MockSource {
    zoom_range: Range<u8>,
    data_tiles: BTreeMap<OverscaledTileID, MockTileData>,
    ideal_tiles: BTreeSet<OverscaledTileID>,
    log: Vec<ActionLogEntry>,
    next_serial: u64,
}

impl MockSource {
    fn new() -> Self {
        MockSource {
            zoom_range: Range::new(0, 16),
            data_tiles: BTreeMap::new(),
            ideal_tiles: BTreeSet::new(),
            log: Vec::new(),
            next_serial: 0,
        }
    }

    /// Creates the tile's data, replacing the existing `MockTileData`
    /// object, if any.
    fn create_tile_data(&mut self, tile_id: OverscaledTileID) -> TileRef {
        let tile = MockTileData::new(tile_id, &mut self.next_serial);
        let serial = tile.serial;
        self.data_tiles.insert(tile_id, tile);
        TileRef { tile_id, serial }
    }

    fn existing_tile(&self, tile_id: OverscaledTileID) -> TileRef {
        let tile = &self.data_tiles[&tile_id];
        TileRef { tile_id, serial: tile.serial }
    }

    /// The tile's data; the tile must still be the object `tile` points to.
    fn tile_mut(&mut self, tile: TileRef) -> &mut MockTileData {
        let data = self.data_tiles.get_mut(&tile.tile_id).expect("dangling tile");
        assert_eq!(data.serial, tile.serial, "dangling tile");
        data
    }

    fn data_tile_mut(&mut self, tile_id: OverscaledTileID) -> &mut MockTileData {
        self.data_tiles.get_mut(&tile_id).expect("null tile")
    }

    fn update_renderables(&mut self, max_parent_overscale_factor: Option<u8>) {
        let ideal_tiles: Vec<OverscaledTileID> = self.ideal_tiles.iter().copied().collect();
        let zoom_range = self.zoom_range;
        let mut ctx =
            Callbacks { data_tiles: &mut self.data_tiles, log: &mut self.log, next_serial: &mut self.next_serial };
        update_renderables(&mut ctx, &ideal_tiles, &zoom_range, max_parent_overscale_factor);
    }
}

/// getTileDataFn, createTileDataFn, retainTileDataFn, renderTileFn, with
/// `source.dataTiles` as the prefetched tiles.
struct Callbacks<'a> {
    data_tiles: &'a mut BTreeMap<OverscaledTileID, MockTileData>,
    log: &'a mut Vec<ActionLogEntry>,
    next_serial: &'a mut u64,
}

impl RenderablesContext for Callbacks<'_> {
    fn get_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        let found = self.data_tiles.contains_key(id);
        self.log.push(GetTileDataAction(*id, if found { Found } else { NotFound }));
        found.then_some(*id)
    }

    fn create_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        self.log.push(CreateTileDataAction(*id));
        let tile = MockTileData::new(*id, self.next_serial);
        self.data_tiles.insert(*id, tile);
        Some(*id)
    }

    fn retain_tile(&mut self, tile: &OverscaledTileID, necessity: TileNecessity) {
        let tile_data = &self.data_tiles[tile];
        self.log.push(RetainTileDataAction(tile_data.tile_id, necessity));
    }

    fn render_tile(&mut self, id: &UnwrappedTileID, tile: &OverscaledTileID) {
        let tile_data = &self.data_tiles[tile];
        self.log.push(RenderTileAction(*id, TileRef { tile_id: tile_data.tile_id, serial: tile_data.serial }));
    }

    fn prefetched_tiles(&self) -> Vec<PrefetchedTile> {
        self.data_tiles.keys().map(|id| (*id, *id)).collect()
    }

    fn is_renderable(&self, tile: &OverscaledTileID) -> bool {
        self.data_tiles[tile].renderable
    }

    fn has_tried_cache(&self, tile: &OverscaledTileID) -> bool {
        self.data_tiles[tile].tried_optional
    }

    fn is_loaded(&self, tile: &OverscaledTileID) -> bool {
        self.data_tiles[tile].loaded
    }
}

// gtest: UpdateRenderables.SingleTile
#[test]
fn single_tile() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 1, 1));

    // Make sure that we're getting the tile back.
    let tile_1_1_1_1 = source.create_tile_data(OverscaledTileID::from_zxy(1, 1, 1));
    source.tile_mut(tile_1_1_1_1).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // found ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render ideal tile
        ],
        source.log
    );

    // Check a repeated render with the same data.
    source.log.clear();
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // found ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render ideal tile
        ],
        source.log
    );

    // Insert a tile we don't have data for.
    source.log.clear();
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 1));
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), NotFound), // missing ideal tile
            CreateTileDataAction(oid(1, 0, (1, 0, 1))),        // create ideal tile
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 2)), NotFound), // four child tiles
            GetTileDataAction(oid(2, 0, (2, 0, 3)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 2)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 3)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // parent tile
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found),    // found ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render found tile
        ],
        source.log
    );

    // Mark the created tile as having the optional request tried.
    source.log.clear();
    source.data_tile_mut(oid(1, 0, (1, 0, 1))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // missing ideal tile
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 2)), NotFound), // four child tiles
            GetTileDataAction(oid(2, 0, (2, 0, 3)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 2)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 3)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // parent tile
            CreateTileDataAction(oid(0, 0, (0, 0, 0))),        // load parent tile
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // found ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render found tile
        ],
        source.log
    );

    // Now insert the missing tile and check that we're rendering it.
    source.log.clear();
    let tile_1_1_0_1 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 1));
    source.tile_mut(tile_1_1_0_1).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // newly added tile
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 1), tile_1_1_0_1), // render ideal tile
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render found tile
        ],
        source.log
    );

    // Insert another tile, and another bucket that has a different name and
    // check that we're not using it.
    source.log.clear();
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 0));
    let tile_1_1_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 0));

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // found tile, not ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // four child tiles
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            // optional parent tile was already created before, but is not renderable
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // ideal tile
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 1), tile_1_1_0_1), // render ideal tile
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1), // render ideal tile
        ],
        source.log
    );

    // Then, add the bucket and check that it's getting used.
    source.log.clear();
    source.tile_mut(tile_1_1_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // found tile, now ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 0), tile_1_1_0_0),
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // ideal tile
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 1), tile_1_1_0_1),
            GetTileDataAction(oid(1, 0, (1, 1, 1)), Found), // ideal tile
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 1, 1), tile_1_1_1_1),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseParentTile
#[test]
fn use_parent_tile() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 1));
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 1, 0));
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 1, 1));

    // Make sure that we're getting the tile back.
    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), NotFound), // missing ideal tile
            CreateTileDataAction(oid(1, 0, (1, 0, 1))),
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 2)), NotFound), // child tile
            GetTileDataAction(oid(2, 0, (2, 0, 3)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 2)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 3)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent found!
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0), // render parent
            GetTileDataAction(oid(1, 0, (1, 1, 0)), NotFound), // missing ideal tile
            CreateTileDataAction(oid(1, 0, (1, 1, 0))),
            RetainTileDataAction(oid(1, 0, (1, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 2, 0)), NotFound), // child tile
            GetTileDataAction(oid(2, 0, (2, 2, 1)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 3, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 3, 1)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 1, 1)), NotFound), // missing tile
            CreateTileDataAction(oid(1, 0, (1, 1, 1))),
            RetainTileDataAction(oid(1, 0, (1, 1, 1)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 2, 2)), NotFound), // child tile
            GetTileDataAction(oid(2, 0, (2, 2, 3)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 3, 2)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 3, 3)), NotFound), // ...
        ],
        source.log
    );
}

// gtest: UpdateRenderables.DontUseWrongParentTile
#[test]
fn dont_use_wrong_parent_tile() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 0, 0));

    let tile_1_1_1_0 = source.create_tile_data(OverscaledTileID::from_zxy(1, 1, 0));
    source.tile_mut(tile_1_1_1_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // missing ideal tile
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(3, 0, (3, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // parent tile, missing
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // parent tile, missing
        ],
        source.log
    );

    // Now mark the created tile as having the optional request tried.
    source.log.clear();
    source.data_tile_mut(oid(2, 0, (2, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // non-ready ideal tile
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(3, 0, (3, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // parent tile, missing
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),        // find optional parent
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // parent tile, missing
        ],
        source.log
    );

    // Add a new child tile and check that it is now used.
    source.log.clear();
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 2, 0));
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // non-ready ideal tile
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            // this tile was added by the previous invocation of updateRenderables
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(3, 0, (3, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found),    // parent tile not ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // missing parent tile
            GetTileDataAction(oid(2, 0, (2, 2, 0)), NotFound), // missing ideal tile
            CreateTileDataAction(oid(2, 0, (2, 2, 0))),
            RetainTileDataAction(oid(2, 0, (2, 2, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (3, 4, 0)), NotFound), // child tile
            GetTileDataAction(oid(3, 0, (3, 4, 1)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 5, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 5, 1)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 1, 0)), Found),    // found parent tile
            RetainTileDataAction(oid(1, 0, (1, 1, 0)), TileNecessity::Optional),
            RenderTileAction(uid(1, 1, 0), tile_1_1_1_0), // render parent tile
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseParentTileWhenChildNotReady
#[test]
fn use_parent_tile_when_child_not_ready() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 1));

    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;

    let tile_1_1_0_1 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 1));
    // Don't create bucket.

    // Make sure that it renders the parent tile.
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // found, but not ready
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 2)), NotFound), // child tile
            GetTileDataAction(oid(2, 0, (2, 0, 3)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 2)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 3)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile, ready
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0), // render parent tile
        ],
        source.log
    );

    // Now insert the bucket and make sure we're now using the matching tile
    source.log.clear();
    source.tile_mut(tile_1_1_0_1).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // found and ready
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 1), tile_1_1_0_1), // render ideal tile
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseOverlappingParentTile
#[test]
fn use_overlapping_parent_tile() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 0));
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 1));

    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;

    let tile_1_1_0_1 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 1));
    source.tile_mut(tile_1_1_0_1).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ideal tile not found
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile found
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
            GetTileDataAction(oid(1, 0, (1, 0, 1)), Found), // ideal tile found
            RetainTileDataAction(oid(1, 0, (1, 0, 1)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 1), tile_1_1_0_1),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseChildTiles
#[test]
fn use_child_tiles() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(0, 0, 0));

    let tile_1_1_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 0));
    source.tile_mut(tile_1_1_0_0).renderable = true;
    let tile_1_1_1_0 = source.create_tile_data(OverscaledTileID::from_zxy(1, 1, 0));
    source.tile_mut(tile_1_1_1_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ideal tile, missing
            CreateTileDataAction(oid(0, 0, (0, 0, 0))),
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // child tile found
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(1, 0, 0), tile_1_1_0_0), // render child tile
            GetTileDataAction(oid(1, 0, (1, 0, 1)), NotFound), // child tile not found
            GetTileDataAction(oid(1, 0, (1, 1, 0)), Found), // child tile found
            RetainTileDataAction(oid(1, 0, (1, 1, 0)), TileNecessity::Optional),
            RenderTileAction(uid(1, 1, 0), tile_1_1_1_0), // render child tile
            GetTileDataAction(oid(1, 0, (1, 1, 1)), NotFound), // child tile not found
                                                          // no parent tile of 0 to consider
        ],
        source.log
    );

    // Remove direct children and add children 2 zoom levels down
    source.log.clear();
    source.data_tiles.remove(&OverscaledTileID::from_zxy(1, 0, 0));
    source.data_tiles.remove(&OverscaledTileID::from_zxy(1, 1, 0));
    let tile_2_2_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(2, 0, 0));
    source.tile_mut(tile_2_2_0_0).renderable = true;
    let tile_2_2_1_0 = source.create_tile_data(OverscaledTileID::from_zxy(2, 1, 0));
    source.tile_mut(tile_2_2_1_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found), // ideal tile not ready
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // child tile not found
            GetTileDataAction(oid(1, 0, (1, 0, 1)), NotFound), // child tile not found
            GetTileDataAction(oid(1, 0, (1, 1, 0)), NotFound), // child tile not found
            GetTileDataAction(oid(1, 0, (1, 1, 1)), NotFound), // child tile not found
            // no parent tile of 0 to consider
            // no parent or child tile found, check prefetched tiles
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0), // render child from 2 levels down
            RetainTileDataAction(oid(2, 0, (2, 1, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 1, 0), tile_2_2_1_0), // render child from 2 levels down
        ],
        source.log
    );
}

// gtest: UpdateRenderables.PreferChildTiles
#[test]
fn prefer_child_tiles() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 0));

    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;
    let tile_2_2_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(2, 0, 0));
    source.tile_mut(tile_2_2_0_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ideal tile, not found
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // child tile, found
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound), // child tile, not found
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile, found
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
        ],
        source.log
    );

    // Now add more children to cover the ideal tile fully, until it is covered
    // fully, and verify that the parent doesn't get rendered.
    source.log.clear();
    let tile_2_2_0_1 = source.create_tile_data(OverscaledTileID::from_zxy(2, 0, 1));
    source.tile_mut(tile_2_2_0_1).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // ideal tile, not ready
            // ideal tile was added in previous invocation, but is not yet ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // child tile, found
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 0, 1)), TileNecessity::Optional), // ...
            RenderTileAction(uid(2, 0, 1), tile_2_2_0_1),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound), // child tile, not found
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile, found
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
        ],
        source.log
    );

    source.log.clear();
    let tile_2_2_1_0 = source.create_tile_data(OverscaledTileID::from_zxy(2, 1, 0));
    source.tile_mut(tile_2_2_1_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // ideal tile, not ready
            // ideal tile was added in first invocation, but is not yet ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // child tile, found
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 0, 1)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 1), tile_2_2_0_1),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 1, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 1, 0), tile_2_2_1_0),
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound), // child tile, not found
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),    // parent tile, found
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
        ],
        source.log
    );

    // Adding the last child tile covers 1/0/0 fully, so we don't need 0/0/0 anymore.
    source.log.clear();
    let tile_2_2_1_1 = source.create_tile_data(OverscaledTileID::from_zxy(2, 1, 1));
    source.tile_mut(tile_2_2_1_1).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // ideal tile, not ready
            // ideal tile was added in first invocation, but is not yet ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // child tile, found
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 0, 1)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 1), tile_2_2_0_1),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 1, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 1, 0), tile_2_2_1_0),
            GetTileDataAction(oid(2, 0, (2, 1, 1)), Found), // ...
            RetainTileDataAction(oid(2, 0, (2, 1, 1)), TileNecessity::Optional),
            RenderTileAction(uid(2, 1, 1), tile_2_2_1_1),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseParentAndChildTiles
#[test]
fn use_parent_and_child_tiles() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(1, 0, 0));

    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;
    let tile_2_2_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(2, 0, 0));
    source.tile_mut(tile_2_2_0_0).renderable = true;

    // Check that it uses the child tile and the parent tile to cover the rest.
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ideal tile, missing
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // child tile
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found), // parent tile
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
        ],
        source.log
    );

    // Then, remove the child tile and check that it now only the parent tile.
    source.log.clear();
    source.data_tiles.remove(&OverscaledTileID::from_zxy(2, 0, 0));
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found), // parent tile
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.DontUseTilesLowerThanMinzoom
#[test]
fn dont_use_tiles_lower_than_minzoom() {
    let mut source = MockSource::new();

    source.zoom_range.min = 2;
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 0, 0));

    let tile_1_1_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 0));
    source.tile_mut(tile_1_1_0_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ideal tile, missing
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 0, 1)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 1, 0)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 1, 1)), NotFound),
            // no requests for zoom 1 tiles
        ],
        source.log
    );
}

// gtest: UpdateRenderables.UseOverzoomedTileAfterMaxzoom
#[test]
fn use_overzoomed_tile_after_maxzoom() {
    let mut source = MockSource::new();

    source.zoom_range.max = 2;
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 0, 0));

    // Add a child tile (that should never occur in practice) and make sure it's not selected.
    let tile_3_3_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(3, 0, 0));
    source.tile_mut(tile_3_3_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ideal tile, missing
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (2, 0, 0)), NotFound), // overzoomed tile, not children!
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound),
        ],
        source.log
    );

    // Mark the created tile as having tried the optional request.
    source.log.clear();
    source.data_tile_mut(oid(2, 0, (2, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // ideal tile, missing
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (2, 0, 0)), NotFound), // overzoomed tile, not children!
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound),
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound),
        ],
        source.log
    );

    // Switch to overzoomed tile and only add a non-overzoomed ("parent") tile at first.
    source.log.clear();
    source.ideal_tiles.clear();
    source.ideal_tiles.insert(oid(3, 0, (2, 0, 0)));
    let tile_2_2_0_0 = source.create_tile_data(oid(2, 0, (2, 0, 0)));
    source.tile_mut(tile_2_2_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), NotFound), // ideal tile, missing
            CreateTileDataAction(oid(3, 0, (2, 0, 0))),
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(4, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
        ],
        source.log
    );

    // Then add the overzoomed tile matching the zoom level we're rendering.
    source.log.clear();
    let tile_3_2_0_0 = source.create_tile_data(oid(3, 0, (2, 0, 0)));
    source.tile_mut(tile_3_2_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            RenderTileAction(uid(2, 0, 0), tile_3_2_0_0),
        ],
        source.log
    );

    // Check that it's switching back to the tile that has the matching overzoom value.
    source.log.clear();
    source.ideal_tiles.clear();
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 0, 0));
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
        ],
        source.log
    );

    // Now remove the best match.
    source.log.clear();
    source.data_tiles.remove(&oid(2, 0, (2, 0, 0)));
    // C++: tile_2_2_0_0 = nullptr;

    // Use the overzoomed tile even though it doesn't match the zoom level.
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found), // use overzoomed tile!
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_3_2_0_0),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.AscendToNonOverzoomedTiles
#[test]
fn ascend_to_non_overzoomed_tiles() {
    let mut source = MockSource::new();

    source.zoom_range.max = 2;
    source.ideal_tiles.insert(oid(3, 0, (2, 0, 0)));

    // Add a matching overzoomed tile and make sure it gets selected.
    let tile_3_2_0_0 = source.create_tile_data(oid(3, 0, (2, 0, 0)));
    source.tile_mut(tile_3_2_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            RenderTileAction(uid(2, 0, 0), tile_3_2_0_0),
        ],
        source.log
    );

    // Then, swap it with a non-overzoomed tile.
    source.log.clear();
    source.data_tiles.remove(&oid(3, 0, (2, 0, 0)));
    // C++: tile_3_2_0_0 = nullptr;
    let tile_2_2_0_0 = source.create_tile_data(oid(2, 0, (2, 0, 0)));
    source.tile_mut(tile_2_2_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), NotFound),
            CreateTileDataAction(oid(3, 0, (2, 0, 0))),
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(4, 0, (2, 0, 0)), NotFound), // prefer using a child first
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(2, 0, 0), tile_2_2_0_0),
        ],
        source.log
    );

    // Then, swap it with a parent tile.
    source.log.clear();
    source.data_tiles.remove(&oid(2, 0, (2, 0, 0)));
    // C++: tile_2_2_0_0 = nullptr;
    let tile_1_1_0_0 = source.create_tile_data(oid(1, 0, (1, 0, 0)));
    source.tile_mut(tile_1_1_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(4, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(1, 0, 0), tile_1_1_0_0),
        ],
        source.log
    );

    // Now, mark the created tile as found.
    source.log.clear();
    source.data_tile_mut(oid(3, 0, (2, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(4, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(1, 0, 0), tile_1_1_0_0),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.DoNotAscendMultipleTimesIfNotFound
#[test]
fn do_not_ascend_multiple_times_if_not_found() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(8, 0, 0));
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(8, 1, 0));

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(8, 0, (8, 0, 0)), NotFound), // ideal tile
            CreateTileDataAction(oid(8, 0, (8, 0, 0))),
            RetainTileDataAction(oid(8, 0, (8, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(9, 0, (9, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(9, 0, (9, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(6, 0, (6, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(8, 0, (8, 1, 0)), NotFound), // ideal tile
            CreateTileDataAction(oid(8, 0, (8, 1, 0))),
            RetainTileDataAction(oid(8, 0, (8, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(9, 0, (9, 2, 0)), NotFound), // child tile
            GetTileDataAction(oid(9, 0, (9, 2, 1)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 3, 0)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 3, 1)), NotFound), // ...
                                                               // no second ascent to 0
        ],
        source.log
    );

    // Now add a mid-level tile that stops the ascent
    source.log.clear();
    let tile_4_0_0_0 = source.create_tile_data(oid(4, 0, (4, 0, 0)));
    source.tile_mut(tile_4_0_0_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(8, 0, (8, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(8, 0, (8, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(9, 0, (9, 0, 0)), NotFound), // child tile
            GetTileDataAction(oid(9, 0, (9, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(6, 0, (6, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(4, 0, (4, 0, 0)), Found),    // stops ascent
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(4, 0, 0), tile_4_0_0_0),
            GetTileDataAction(oid(8, 0, (8, 1, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(8, 0, (8, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(9, 0, (9, 2, 0)), NotFound), // child tile
            GetTileDataAction(oid(9, 0, (9, 2, 1)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 3, 0)), NotFound), // ...
            GetTileDataAction(oid(9, 0, (9, 3, 1)), NotFound), // ...
                                                               // no second ascent to 0
        ],
        source.log
    );
}

// gtest: UpdateRenderables.DontRetainUnusedNonIdealTiles
#[test]
fn dont_retain_unused_non_ideal_tiles() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(2, 0, 0));

    source.create_tile_data(OverscaledTileID::from_zxy(1, 0, 0));
    source.create_tile_data(OverscaledTileID::from_zxy(2, 0, 0));

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 0, 1)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 1, 0)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 1, 1)), NotFound),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found), // parent tile, not ready
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.WrappedTiles
#[test]
fn wrapped_tiles() {
    let mut source = MockSource::new();

    let tile_ids = [
        UnwrappedTileID::new(1, -1, 0), // 'wrap' -> -1
        UnwrappedTileID::new(1, 0, 0),  // 'wrap' -> 0
        UnwrappedTileID::new(1, 1, 0),  // 'wrap' -> 0
        UnwrappedTileID::new(1, 2, 0),  // 'wrap' -> 1
    ];

    for id in &tile_ids {
        source.ideal_tiles.insert(OverscaledTileID::new(1, id.wrap, id.canonical));
    }

    let tile_0_0_0_0 = source.create_tile_data(oid(0, 0, (0, 0, 0)));
    source.tile_mut(tile_0_0_0_0).renderable = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(1, -1, (1, 1, 0)), NotFound), // ideal tile 1/-1/0 (wrapped to -1)
            CreateTileDataAction(oid(1, -1, (1, 1, 0))),
            RetainTileDataAction(oid(1, -1, (1, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, -1, (2, 2, 0)), NotFound),
            GetTileDataAction(oid(2, -1, (2, 2, 1)), NotFound),
            GetTileDataAction(oid(2, -1, (2, 3, 0)), NotFound),
            GetTileDataAction(oid(2, -1, (2, 3, 1)), NotFound),
            GetTileDataAction(oid(0, -1, (0, 0, 0)), NotFound), // { 0, 0, 0 } exists, but not the version wrapped to -1
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound),  // ideal tile 1/0/0
            CreateTileDataAction(oid(1, 0, (1, 0, 0))),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 0, 1)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 1, 1)), NotFound),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0),
            GetTileDataAction(oid(1, 0, (1, 1, 0)), NotFound), // ideal tile 1/1/0, doesn't match 1/-/1/0
            CreateTileDataAction(oid(1, 0, (1, 1, 0))),
            RetainTileDataAction(oid(1, 0, (1, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 2, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 2, 1)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 3, 0)), NotFound),
            GetTileDataAction(oid(2, 0, (2, 3, 1)), NotFound),
            // do not ascent; 0/0/0 has been rendered already for 1/0/0
            GetTileDataAction(oid(1, 1, (1, 0, 0)), NotFound), // ideal tile 1/2/0 (wrapped to 1)
            CreateTileDataAction(oid(1, 1, (1, 0, 0))),
            RetainTileDataAction(oid(1, 1, (1, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 1, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(2, 1, (2, 0, 1)), NotFound),
            GetTileDataAction(oid(2, 1, (2, 1, 0)), NotFound),
            GetTileDataAction(oid(2, 1, (2, 1, 1)), NotFound),
            GetTileDataAction(oid(0, 1, (0, 0, 0)), NotFound), // { 0, 0, 0 } exists, but not the version wrapped to -1
        ],
        source.log
    );
}

// gtest: UpdateRenderables.RepeatedRenderWithMissingOptionals
#[test]
fn repeated_render_with_missing_optionals() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(6, 0, 0));

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), NotFound), // ideal tile, not found
            CreateTileDataAction(oid(6, 0, (6, 0, 0))),
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Repeat.
    source.log.clear();
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Mark next level has having tried optional.
    source.log.clear();
    source.data_tile_mut(oid(6, 0, (6, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ascent
            CreateTileDataAction(oid(5, 0, (5, 0, 0))),
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Repeat.
    source.log.clear();
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), Found),    // ascent
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Mark next level has having tried optional.
    source.log.clear();
    source.data_tile_mut(oid(5, 0, (5, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), Found),    // ascent
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            CreateTileDataAction(oid(4, 0, (4, 0, 0))),
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Mark next level has having tried optional.
    source.log.clear();
    source.data_tile_mut(oid(4, 0, (4, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), Found),    // ascent
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), Found), // ...
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            CreateTileDataAction(oid(3, 0, (3, 0, 0))),
            RetainTileDataAction(oid(3, 0, (3, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Mark next level has having tried optional.
    source.log.clear();
    source.data_tile_mut(oid(3, 0, (3, 0, 0))).tried_optional = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), Found),    // ascent
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), Found), // ...
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), Found), // ...
            RetainTileDataAction(oid(3, 0, (3, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            CreateTileDataAction(oid(2, 0, (2, 0, 0))),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );

    // Mark as found
    source.log.clear();
    let tile_3_3_0_0 = source.existing_tile(oid(3, 0, (3, 0, 0)));
    source.tile_mut(tile_3_3_0_0).renderable = true;
    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not ready
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (7, 0, 0)), NotFound), // children
            GetTileDataAction(oid(7, 0, (7, 0, 1)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 0)), NotFound), // ...
            GetTileDataAction(oid(7, 0, (7, 1, 1)), NotFound), // ...
            GetTileDataAction(oid(5, 0, (5, 0, 0)), Found),    // ascent
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), Found), // ...
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Optional),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), Found), // ...
            RetainTileDataAction(oid(3, 0, (3, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(3, 0, 0), tile_3_3_0_0),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.LoadRequiredIfIdealTileCantBeFound
#[test]
fn load_required_if_ideal_tile_cant_be_found() {
    let mut source = MockSource::new();

    source.zoom_range.max = 6;
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(6, 0, 0));

    let tile_6_6_0_0 = source.create_tile_data(oid(6, 0, (6, 0, 0)));
    source.tile_mut(tile_6_6_0_0).tried_optional = true;
    source.tile_mut(tile_6_6_0_0).loaded = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(6, 0, (6, 0, 0)), Found), // ideal tile, not found
            RetainTileDataAction(oid(6, 0, (6, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(7, 0, (6, 0, 0)), NotFound), // overzoomed child
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // ascent
            CreateTileDataAction(oid(5, 0, (5, 0, 0))),
            RetainTileDataAction(oid(5, 0, (5, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound), // ...
            GetTileDataAction(oid(0, 0, (0, 0, 0)), NotFound), // ...
        ],
        source.log
    );
}

// gtest: UpdateRenderables.LoadOverscaledMaxZoomTile
#[test]
fn load_overscaled_max_zoom_tile() {
    let mut source = MockSource::new();

    source.zoom_range.max = 2;
    source.ideal_tiles.insert(oid(4, 0, (2, 0, 0)));

    let tile_4_2_0_0 = source.create_tile_data(oid(4, 0, (2, 0, 0)));
    source.tile_mut(tile_4_2_0_0).renderable = false;
    source.tile_mut(tile_4_2_0_0).tried_optional = true;
    source.tile_mut(tile_4_2_0_0).loaded = true;

    let tile_3_2_0_0 = source.create_tile_data(oid(3, 0, (2, 0, 0)));
    source.tile_mut(tile_3_2_0_0).renderable = false;
    source.tile_mut(tile_3_2_0_0).tried_optional = true;
    source.tile_mut(tile_3_2_0_0).loaded = true;

    let tile_2_2_0_0 = source.create_tile_data(oid(2, 0, (2, 0, 0)));
    source.tile_mut(tile_2_2_0_0).renderable = false;
    source.tile_mut(tile_2_2_0_0).tried_optional = true;
    source.tile_mut(tile_2_2_0_0).loaded = true;

    // Tile level 1 won't be overscaled.
    let tile_1_1_0_0 = source.create_tile_data(oid(1, 0, (1, 0, 0)));
    source.tile_mut(tile_1_1_0_0).renderable = true;
    source.tile_mut(tile_1_1_0_0).tried_optional = true;
    source.tile_mut(tile_1_1_0_0).loaded = true;

    source.update_renderables(None);
    assert_eq!(
        vec![
            GetTileDataAction(oid(4, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(4, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(5, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(3, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(3, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(2, 0, (2, 0, 0)), Found),
            RetainTileDataAction(oid(2, 0, (2, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), Found),
            RetainTileDataAction(oid(1, 0, (1, 0, 0)), TileNecessity::Required),
            RenderTileAction(uid(1, 0, 0), tile_1_1_0_0),
        ],
        source.log
    );
}

// gtest: UpdateRenderables.MaxParentOverscaleFactor
#[test]
fn max_parent_overscale_factor() {
    let mut source = MockSource::new();

    source.ideal_tiles.insert(OverscaledTileID::from_zxy(4, 0, 0));
    source.ideal_tiles.insert(OverscaledTileID::from_zxy(4, 1, 0));

    let tile_0_0_0_0 = source.create_tile_data(OverscaledTileID::from_zxy(0, 0, 0));
    source.tile_mut(tile_0_0_0_0).renderable = true;

    // Set max parent overscale factor to 4, so that tile 0,0,0 would be rendered
    source.update_renderables(Some(4));
    assert_eq!(
        vec![
            GetTileDataAction(oid(4, 0, (4, 0, 0)), NotFound), // ideal tile
            CreateTileDataAction(oid(4, 0, (4, 0, 0))),
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // child tiles
            GetTileDataAction(oid(5, 0, (5, 0, 1)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 1, 0)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 1, 1)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound),
            GetTileDataAction(oid(0, 0, (0, 0, 0)), Found),
            RetainTileDataAction(oid(0, 0, (0, 0, 0)), TileNecessity::Optional),
            RenderTileAction(uid(0, 0, 0), tile_0_0_0_0), // render tile 0,0,0
            GetTileDataAction(oid(4, 0, (4, 1, 0)), NotFound), // ideal tile
            CreateTileDataAction(oid(4, 0, (4, 1, 0))),
            RetainTileDataAction(oid(4, 0, (4, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(5, 0, (5, 2, 0)), NotFound), // child tiles
            GetTileDataAction(oid(5, 0, (5, 2, 1)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 3, 0)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 3, 1)), NotFound),
        ],
        source.log
    );

    source.log.clear();

    // Set max parent overscale factor to 3.
    // Parent tile 0,0,0 should not be requested / rendered.
    source.update_renderables(Some(3));
    assert_eq!(
        vec![
            GetTileDataAction(oid(4, 0, (4, 0, 0)), Found), // ideal tile
            RetainTileDataAction(oid(4, 0, (4, 0, 0)), TileNecessity::Required),
            GetTileDataAction(oid(5, 0, (5, 0, 0)), NotFound), // child tiles
            GetTileDataAction(oid(5, 0, (5, 0, 1)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 1, 0)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 1, 1)), NotFound),
            GetTileDataAction(oid(3, 0, (3, 0, 0)), NotFound), // ascent
            GetTileDataAction(oid(2, 0, (2, 0, 0)), NotFound),
            GetTileDataAction(oid(1, 0, (1, 0, 0)), NotFound),
            GetTileDataAction(oid(4, 0, (4, 1, 0)), Found), // ideal tile
            RetainTileDataAction(oid(4, 0, (4, 1, 0)), TileNecessity::Required),
            GetTileDataAction(oid(5, 0, (5, 2, 0)), NotFound), // child tiles
            GetTileDataAction(oid(5, 0, (5, 2, 1)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 3, 0)), NotFound),
            GetTileDataAction(oid(5, 0, (5, 3, 1)), NotFound),
        ],
        source.log
    );
}
