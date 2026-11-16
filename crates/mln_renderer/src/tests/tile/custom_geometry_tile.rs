// C++ calls a CustomTileLoader directly and reaches it from the tiles
// through a mailbox on the test's run loop. Here the loader is an actor on
// the run loop, so the test reaches it through messages too, and what the
// loader sends the tiles waits in their store's inbox.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use feature::Feature;
use geo_types::Point;
use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::scheduler;
use mln_style::StyleRegistry;
use mln_style::custom_tile_loader::{CustomTileLoader, TileFunction};
use mln_style::layer::{Layer, downcast_arc};
use mln_style::layers::Circle;
use mln_style::layers::circle_layer::Impl as CircleLayerImpl;
use mln_style::sources::custom_geometry_source::CustomGeometryTileLoader;
use mln_style::sources::geojson_source::TileFeatures;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use parking_lot::Mutex;

use super::RasterTileTest;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile::contents::Contents;
use crate::tile::loading::{CustomLoaderOf, DataOrigin};
use crate::tile::tile::{TileKind, TileNecessity};
use crate::tile::tile_workers::InlineWorkers;

/// The loader actor, on the test's run loop.
fn loader(
    run_loop: &mln_actor::RunLoop,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> Actor<CustomGeometryTileLoader> {
    Actor::new(run_loop.scheduler(), |_| CustomTileLoader::new(fetch, cancel))
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

/// Tiles `ids` of a custom geometry source whose loader is `loader`.
fn tiles(test: &RasterTileTest, ids: &[OverscaledTileID], loader: PoolActorRef<CustomGeometryTileLoader>) -> OneTile {
    let store = crate::tests::tile_store(&test.run_loop, &test.thread_pool, Box::new(InlineWorkers));
    let origin = DataOrigin::Custom(Arc::new(CustomLoaderOf {
        loader,
        convert: Arc::new(|features: Arc<TileFeatures>| Some(mln_tile::TileData::Features(features))),
    }));
    OneTile::with_origin(store, TileKind::Geometry, true, origin, ids, mln_map_types::MapMode::Static)
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
    let loader = loader(&test.run_loop, Some(fetch), Some(Arc::new(|_: &CanonicalTileID| {})));

    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();

    assert_eq!(*fetched.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.InvokeCancelTile
#[test]
fn invoke_cancel_tile() {
    let test = RasterTileTest::new();
    let (cancelled, cancel) = recorder();
    let loader = loader(&test.run_loop, Some(Arc::new(|_: &CanonicalTileID| {})), Some(cancel));

    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.run_loop.wait_for_empty();

    assert_eq!(*cancelled.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.FetchCancelledTileAgain
#[test]
fn fetch_cancelled_tile_again() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.run_loop, Some(fetch), None);

    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.InvalidateFailedTile
#[test]
fn invalidate_failed_tile() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.run_loop, Some(fetch), None);

    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.set_tile_error(&id, &Error::native("failed")));
    test.run_loop.wait_for_empty();
    tiles.store.receive();
    assert_eq!(tiles.errors(), ["failed"]);

    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.InvokeTileChanged
#[test]
fn invoke_tile_changed() {
    let test = RasterTileTest::new();
    let loader = loader(&test.run_loop, None, None);
    let mut tiles = tiles(&test, &[root()], loader.self_ref());

    let layer = Layer::new(&StyleRegistry::with_builtins(), "circle", "circle", "source").unwrap();
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Circle>::new(downcast_arc::<CircleLayerImpl>(&layer.base_impl())));

    tiles.set_style(&crate::tests::style(std::slice::from_ref(&layer_properties)));
    assert!(tiles.tile().contents.is_none());

    tiles.set_data(Some(mln_tile::TileData::Features(Arc::new(vec![Feature {
        geometry: Some(Point::new(0i16, 0).into()),
        ..Feature::default()
    }]))));
    while !tiles.tile().is_complete() {
        test.run_loop.run_once();
        tiles.update();
    }
    // Once present, the bucket should never "disappear", which would cause
    // flickering.
    assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));

    let Some(Contents::Geometry(contents)) = tiles.tile().contents.as_deref() else {
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
    // On the background scheduler, so the loader's message to the tile waits
    // in the store's inbox until the test takes it in.
    let loader: Actor<CustomGeometryTileLoader> =
        Actor::new(scheduler::get_background(), |_| CustomTileLoader::new(Some(fetch), None));
    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    loader.self_ref().ask(|_| ()).recv().unwrap();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    loader.self_ref().ask(move |loader| loader.invalidate_tile(&root().canonical)).recv().unwrap();
    tiles.drop_tile();
    test.run_loop.wait_for_empty();

    assert_eq!(tiles.changes(), 0);
}

// gtest: CustomGeometryTile.CancellationDiscardsLateInvalidation
#[test]
fn cancellation_discards_late_invalidation() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.run_loop, Some(fetch), None);
    let mut tiles = tiles(&test, &[root()], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.drop_tile();
    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&root().canonical));
    test.run_loop.wait_for_empty();

    assert_eq!(tiles.changes(), 0);
}

// gtest: CustomGeometryTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let loader = loader(&test.run_loop, Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root(), second], loader.self_ref());
    tiles.set_necessity(&root(), TileNecessity::Required);
    tiles.set_necessity(&second, TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The map moves by a world, and the source changes both copies' wraps.
    let first = root().unwrap_to(-1);
    tiles.rekey(&root(), first);
    tiles.rekey(&second, root());

    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.run_loop.wait_for_empty();
    assert_eq!(cancels.load(Ordering::SeqCst), 0);

    // The copy that is still needed is still invalidated, and fetches again.
    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();
    tiles.store.receive();
    tiles.set_necessity(&first, TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.CancelledTileDropsCachedData
#[test]
fn cancelled_tile_drops_cached_data() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.run_loop, Some(fetch), None);

    let mut first = tiles(&test, &[root()], loader.self_ref());
    first.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The data arrives, but the tile leaves the view before it is parsed.
    loader.self_ref().invoke(move |loader| loader.set_tile_data(&root().canonical, Arc::new(Vec::new())));
    first.set_necessity(&root(), TileNecessity::Optional);
    test.run_loop.wait_for_empty();
    first.drop_tile();
    test.run_loop.wait_for_empty();

    // The data went with the last tile that wanted it.
    let mut next = tiles(&test, &[root()], loader.self_ref());
    next.set_necessity(&root(), TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
