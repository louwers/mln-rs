// C++ calls a CustomTileLoader directly and reaches it from the tiles
// through a mailbox on the test's run loop. Here the provider's calls run
// on a pool without threads, so the test's calls run when the test runs the
// pool, and what the provider sends the tiles waits in their store's inbox.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use feature::Feature;
use geo_types::Point;
use mln_style::error::Error;
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::Circle;
use mln_style::tile_id::CanonicalTileID;
use parking_lot::Mutex;

use super::RasterTileTest;
use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::Contents;
use crate::tile_store::custom_tile_loader::{CustomGeometryProvider, CustomProvider, HostFunction};
use crate::tile_store::loading::DataOrigin;
use crate::tile_store::store::StoreEvent;
use crate::tile_store::tile::{TileKind, TileNecessity, TileOperation};
use crate::worker::tile_workers::InlineWorkers;

/// A host's tile function, as the tests share it.
type TileFunction = Arc<dyn Fn(&CanonicalTileID) + Send + Sync>;

/// The provider, whose calls run on `pool`.
fn loader(
    pool: &crate::exec::pool::Pool,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> Arc<CustomGeometryProvider> {
    let host = |function: Option<TileFunction>| -> Option<HostFunction> {
        function.map(|function| -> HostFunction { Box::new(move |tile_id| function(tile_id)) })
    };
    CustomProvider::new(pool, |_| (host(fetch), host(cancel)))
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
fn tiles(test: &RasterTileTest, ids: &[OverscaledTileID], loader: Arc<CustomGeometryProvider>) -> OneTile {
    let store = crate::tests::tile_store(&test.thread_pool, Box::new(InlineWorkers));
    let origin = DataOrigin::Custom { loader, fast_pfor_enabled: false };
    OneTile::with_origin(store, TileKind::Geometry, true, origin, ids, crate::map::MapMode::Static)
}

/// The root tile's id.
fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: CustomGeometryTile.InvokeFetchTile
#[test]
fn invoke_fetch_tile() {
    let test = RasterTileTest::new();
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
    let test = RasterTileTest::new();
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
    let test = RasterTileTest::new();
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
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);

    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.set_tile_error(id, Error::native("failed"));
    test.local_pool.run_all();
    tiles.store.receive();
    assert_eq!(tiles.errors(), ["failed"]);

    loader.invalidate_tile(id);
    test.local_pool.run_all();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// Port-only: a tile whose provider reports an error reports it as a tile action, before the error
// itself, as a tile whose request or layout fails does.
#[test]
fn a_tile_error_is_an_error_action() {
    let test = RasterTileTest::new();
    let loader = loader(&test.local_pool, Some(Arc::new(|_: &CanonicalTileID| {})), None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    tiles.store.take_events();

    loader.set_tile_error(CanonicalTileID::new(0, 0, 0), Error::native("failed"));
    test.local_pool.run_all();
    tiles.store.receive();
    let events = tiles.store.take_events();
    assert!(
        matches!(
            events.as_slice(),
            [StoreEvent::Action { op: TileOperation::Error, .. }, StoreEvent::Error { error, .. }]
                if error.to_string() == "failed"
        ),
        "{events:?}"
    );
    assert!(tiles.tile().is_complete());
}

// gtest: CustomGeometryTile.InvokeTileChanged
#[test]
fn invoke_tile_changed() {
    let test = RasterTileTest::new();
    let loader = loader(&test.local_pool, None, None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));

    let layer = Layer::new(Registry::with_builtins().style(), "circle", "circle", "source").unwrap();
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Circle>::new(LayerStyle::recover(layer.style()).unwrap()));

    tiles.set_style(&crate::tests::style(std::slice::from_ref(&layer_properties)));
    assert!(tiles.tile().contents().is_none());

    tiles.set_data(Some(crate::tile::TileData::Features(Arc::new(vec![Feature {
        geometry: Some(Point::new(0i16, 0).into()),
        ..Feature::default()
    }]))));
    while !tiles.tile().is_complete() {
        test.local_pool.run_all();
        tiles.update();
    }
    // Once present, the bucket should never "disappear", which would cause
    // flickering.
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));

    let Some(Contents::Geometry(contents)) = tiles.tile().contents() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root(), true, &mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result.len(), 1);
}

// gtest: CustomGeometryTile.CancellationDiscardsQueuedInvalidation
#[test]
fn cancellation_discards_queued_invalidation() {
    let test = RasterTileTest::new();
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
    tiles.drop_tile();
    test.local_pool.run_all();

    assert_eq!(tiles.changes(), 0);
}

// gtest: CustomGeometryTile.CancellationDiscardsLateInvalidation
#[test]
fn cancellation_discards_late_invalidation() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);
    let mut tiles = tiles(&test, &[root()], Arc::clone(&loader));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.drop_tile();
    loader.invalidate_tile(root().canonical);
    test.local_pool.run_all();

    assert_eq!(tiles.changes(), 0);
}

// gtest: CustomGeometryTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
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
    tiles.store.receive();
    tiles.set_necessity(&first, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.CancelledTileDropsCachedData
#[test]
fn cancelled_tile_drops_cached_data() {
    let test = RasterTileTest::new();
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
    first.drop_tile();
    test.local_pool.run_all();

    // The data went with the last tile that wanted it.
    let mut next = tiles(&test, &[root()], Arc::clone(&loader));
    next.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
