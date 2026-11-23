// C++ calls a CustomTileLoader directly and reaches it from the tiles
// through a mailbox on the test's run loop. Here the provider's calls run
// on a pool without threads, so the test's calls run when the test runs the
// pool, and what the provider sends the tiles waits in their store's inbox.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_map_types::MapMode;
use mln_render::source::{TileKind, TileNecessity, TileOperation};
use mln_render::testing::{Event, OriginTiles};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use parking_lot::Mutex;

use super::{TileTest, changes, errors};
use crate::CustomGeometrySourceType;
use crate::custom::CustomGeometryProvider;
use crate::custom::origin::CustomOrigin;
use crate::custom::provider::{CustomProvider, TileFunction};

/// The provider, whose calls run on `pool`.
fn loader(
    pool: &mln_util::pool::Pool,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> Arc<CustomGeometryProvider> {
    CustomProvider::new(fetch, cancel, pool)
}

/// A tile function that counts its calls.
fn counter() -> (Arc<AtomicUsize>, TileFunction) {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    (
        count,
        Arc::new(move |_: &CanonicalTileID| {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

/// A tile function that records the tiles it's called with.
fn recorder() -> (Arc<Mutex<Vec<CanonicalTileID>>>, TileFunction) {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&ids);
    (ids, Arc::new(move |id: &CanonicalTileID| recorded.lock().push(*id)))
}

/// Tiles `ids` of a custom geometry source whose provider is `loader`.
fn tiles(test: &TileTest, ids: &[OverscaledTileID], loader: Arc<CustomGeometryProvider>) -> OriginTiles {
    let origin = CustomOrigin::<CustomGeometrySourceType>::with_loader(loader);
    OriginTiles::new(&test.thread_pool, TileKind::Geometry, origin, ids, MapMode::Static)
}

/// The root tile's id.
fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: CustomGeometryTile.InvokeFetchTile
#[test]
fn invoke_fetch_tile() {
    let test = TileTest::new();
    let (fetched, fetch) = recorder();
    let loader = loader(&test.local_pool, Some(fetch), Some(Arc::new(|_: &CanonicalTileID| {})));

    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();

    assert_eq!(*fetched.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.InvokeCancelTile
#[test]
fn invoke_cancel_tile() {
    let test = TileTest::new();
    let (cancelled, cancel) = recorder();
    let loader = loader(&test.local_pool, Some(Arc::new(|_: &CanonicalTileID| {})), Some(cancel));

    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.local_pool.run_all();

    assert_eq!(*cancelled.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.FetchCancelledTileAgain
#[test]
fn fetch_cancelled_tile_again() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);

    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.InvalidateFailedTile
#[test]
fn invalidate_failed_tile() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);

    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.set_tile_error(id, Error::native("failed"));
    test.local_pool.run_all();
    tiles.receive();
    assert_eq!(errors(&mut tiles), ["failed"]);

    loader.invalidate_tile(id);
    test.local_pool.run_all();
    tiles.receive();

    // The render source updates the necessity of a changed tile.
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// Port-only: a tile whose provider reports an error reports it as a tile action, before the error
// itself, as a tile whose request or layout fails does.
#[test]
fn a_tile_error_is_an_error_action() {
    let test = TileTest::new();
    let loader = loader(&test.local_pool, Some(Arc::new(|_: &CanonicalTileID| {})), None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    tiles.take_events();

    loader.set_tile_error(CanonicalTileID::new(0, 0, 0), Error::native("failed"));
    test.local_pool.run_all();
    tiles.receive();
    let events = tiles.take_events();
    assert!(
        matches!(
            events.as_slice(),
            [Event::Action(TileOperation::Error), Event::Error { error, .. }] if error.to_string() == "failed"
        ),
        "{events:?}"
    );
    assert!(tiles.is_complete(&root()));
}

// gtest: CustomGeometryTile.CancellationDiscardsQueuedInvalidation
#[test]
fn cancellation_discards_queued_invalidation() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    // The provider's message to the tile waits in the store's inbox until
    // the test takes it in.
    let loader = loader(&test.local_pool, Some(fetch), None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    loader.invalidate_tile(root().canonical);
    test.local_pool.run_all();
    tiles.drop_tile(&root());
    test.local_pool.run_all();

    assert_eq!(changes(&mut tiles), 0);
}

// gtest: CustomGeometryTile.CancellationDiscardsLateInvalidation
#[test]
fn cancellation_discards_late_invalidation() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.drop_tile(&root());
    loader.invalidate_tile(root().canonical);
    test.local_pool.run_all();

    assert_eq!(changes(&mut tiles), 0);
}

// gtest: CustomGeometryTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let loader = loader(&test.local_pool, Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root(), second], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&second, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The map moves by a world, and the source changes both copies' wraps.
    let first = root().unwrap_to(-1);
    tiles.rekey(&root(), first);
    tiles.rekey(&second, root());

    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.local_pool.run_all();
    assert_eq!(cancels.load(Ordering::SeqCst), 0);

    // The copy that is still needed is still invalidated, and fetches again.
    let id = CanonicalTileID::new(0, 0, 0);
    loader.invalidate_tile(id);
    test.local_pool.run_all();
    tiles.receive();
    tiles.set_necessity(&first, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.CancelledTileDropsCachedData
#[test]
fn cancelled_tile_drops_cached_data() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);

    let mut first = tiles(&test, &[root()], Arc::clone(&loader));
    first.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The data arrives, but the tile leaves the view before it is parsed.
    loader.set_tile_data(root().canonical, Arc::new(Vec::new()));
    first.set_necessity(&root(), TileNecessity::Optional);
    test.local_pool.run_all();
    first.drop_tile(&root());
    test.local_pool.run_all();

    // The data went with the last tile that wanted it.
    let mut next = tiles(&test, &[root()], Arc::clone(&loader));
    next.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// Port-only: a tile leaves its provider however it goes, here with the
// store that holds it: the host stops fetching what no tile needs, and the
// next tile to need it asks again.
#[test]
fn dropping_the_tiles_removes_their_subscriptions() {
    let test = TileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let loader = loader(&test.local_pool, Some(fetch), Some(cancel));
    let mut first = tiles(&test, &[root()], Arc::clone(&loader));
    first.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    drop(first);
    test.local_pool.run_all();
    assert_eq!(cancels.load(Ordering::SeqCst), 1, "the host stops fetching for the tile that went");

    let mut next = tiles(&test, &[root()], loader);
    next.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
