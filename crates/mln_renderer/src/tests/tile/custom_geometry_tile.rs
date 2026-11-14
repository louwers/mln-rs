// C++ calls a CustomTileLoader directly and reaches it from the tiles
// through a mailbox on the test's run loop. Here the loader is an actor on
// the run loop, so the test reaches it through messages too.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use feature::Feature;
use geo_types::Point;
use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::scheduler;
use mln_style::custom_tile_loader::{CustomTileLoader, TileFunction};
use mln_style::layer::{Layer, downcast_arc};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::circle_layer::Impl as CircleLayerImpl;
use mln_style::layers::{CircleLayer, CircleLayerProperties};
use mln_style::sources::custom_geometry_source::CustomGeometryTileLoader;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use parking_lot::Mutex;

use super::RasterTileTest;
use crate::query::SourceQueryOptions;
use crate::tile::custom_tile::CustomGeometryTile;
use crate::tile::tile::{Tile, TileBase, TileNecessity};
use crate::tile::tile_observer::TileObserver;

/// The loader actor, on the test's run loop.
fn loader(fetch: Option<TileFunction>, cancel: Option<TileFunction>) -> Actor<CustomGeometryTileLoader> {
    Actor::new(scheduler::get_current(), |_| CustomTileLoader::new(fetch, cancel))
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

fn tile(
    test: &RasterTileTest,
    id: OverscaledTileID,
    loader: PoolActorRef<CustomGeometryTileLoader>,
    observer: Option<Rc<dyn TileObserver>>,
) -> CustomGeometryTile {
    CustomGeometryTile::new(id, "source".to_owned(), &test.tile_parameters(), loader, observer)
}

type OnTileChanged = Box<dyn Fn(&TileBase)>;
type OnTileError = Box<dyn Fn(&TileBase, Error)>;

#[derive(Default)]
struct StubTileObserver {
    tile_changed: RefCell<Option<OnTileChanged>>,
    tile_error: RefCell<Option<OnTileError>>,
}

impl TileObserver for StubTileObserver {
    fn on_tile_changed(&self, tile: &TileBase) {
        if let Some(f) = &*self.tile_changed.borrow() {
            f(tile);
        }
    }

    fn on_tile_error(&self, tile: &TileBase, error: Error) {
        if let Some(f) = &*self.tile_error.borrow() {
            f(tile, error);
        }
    }
}

/// An observer that counts the tile's changes.
fn change_counter() -> (Rc<Cell<u32>>, Rc<StubTileObserver>) {
    let changes = Rc::new(Cell::new(0));
    let observer = Rc::new(StubTileObserver::default());
    let counted = Rc::clone(&changes);
    *observer.tile_changed.borrow_mut() = Some(Box::new(move |_| counted.set(counted.get() + 1)));
    (changes, observer)
}

// gtest: CustomGeometryTile.InvokeFetchTile
#[test]
fn invoke_fetch_tile() {
    let test = RasterTileTest::new();
    let (fetched, fetch) = recorder();
    let loader = loader(Some(fetch), Some(Arc::new(|_: &CanonicalTileID| {})));

    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), None);
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();

    assert_eq!(*fetched.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.InvokeCancelTile
#[test]
fn invoke_cancel_tile() {
    let test = RasterTileTest::new();
    let (cancelled, cancel) = recorder();
    let loader = loader(Some(Arc::new(|_: &CanonicalTileID| {})), Some(cancel));

    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), None);
    tile.set_necessity(TileNecessity::Required);
    tile.set_necessity(TileNecessity::Optional);
    test.run_loop.wait_for_empty();

    assert_eq!(*cancelled.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.FetchCancelledTileAgain
#[test]
fn fetch_cancelled_tile_again() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(Some(fetch), None);

    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), None);
    tile.set_necessity(TileNecessity::Required);
    tile.set_necessity(TileNecessity::Optional);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.InvalidateFailedTile
#[test]
fn invalidate_failed_tile() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(Some(fetch), None);

    let observer = Rc::new(StubTileObserver::default());
    let failed = Rc::new(Cell::new(false));
    let failed_set = Rc::clone(&failed);
    *observer.tile_error.borrow_mut() = Some(Box::new(move |_, _| failed_set.set(true)));

    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), Some(observer));
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.set_tile_error(&id, &Error::native("failed")));
    test.run_loop.wait_for_empty();
    assert!(failed.get());

    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();

    // The render source updates the necessity of a changed tile.
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.InvokeTileChanged
#[test]
fn invoke_tile_changed() {
    let test = RasterTileTest::new();
    let loader = loader(None, None);
    let tile = Rc::new(tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), None));

    let layer = CircleLayer::new("circle", "source");
    let layer_properties: Arc<dyn LayerProperties> =
        Arc::new(CircleLayerProperties::new(downcast_arc::<CircleLayerImpl>(&layer.base_impl())));

    let observer = Rc::new(StubTileObserver::default());
    let weak_tile = Rc::downgrade(&tile);
    let properties = Arc::clone(&layer_properties);
    *observer.tile_changed.borrow_mut() = Some(Box::new(move |_| {
        // Once present, the bucket should never "disappear", which would
        // cause flickering.
        let tile = weak_tile.upgrade().unwrap();
        assert!(tile.layer_properties_updated(&properties));
    }));

    tile.set_layers(&[layer_properties], None);
    tile.set_observer(observer);
    let mut result = Vec::new();
    tile.query_source_features(&mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result, Vec::<mln_util::feature::Feature>::new());

    tile.set_tile_data(Arc::new(vec![Feature { geometry: Some(Point::new(0i16, 0).into()), ..Feature::default() }]));

    while !tile.is_complete() {
        test.run_loop.run_once();
    }

    tile.query_source_features(&mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result.len(), 1);
}

// gtest: CustomGeometryTile.CancellationDiscardsQueuedInvalidation
#[test]
fn cancellation_discards_queued_invalidation() {
    let test = RasterTileTest::new();
    let (changes, observer) = change_counter();
    let (fetches, fetch) = counter();
    // On the background scheduler, so the loader's message to the tile waits
    // in the run loop until the test runs it.
    let loader: Actor<CustomGeometryTileLoader> =
        Actor::new(scheduler::get_background(), |_| CustomTileLoader::new(Some(fetch), None));
    let id = OverscaledTileID::from_zxy(0, 0, 0);
    let tile = tile(&test, id, loader.self_ref(), Some(observer));
    tile.set_necessity(TileNecessity::Required);
    loader.self_ref().ask(|_| ()).recv().unwrap();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    loader.self_ref().ask(move |loader| loader.invalidate_tile(&id.canonical)).recv().unwrap();
    tile.cancel();
    test.run_loop.wait_for_empty();

    assert_eq!(changes.get(), 0);
}

// gtest: CustomGeometryTile.CancellationDiscardsLateInvalidation
#[test]
fn cancellation_discards_late_invalidation() {
    let test = RasterTileTest::new();
    let (changes, observer) = change_counter();
    let (fetches, fetch) = counter();
    let loader = loader(Some(fetch), None);
    let id = OverscaledTileID::from_zxy(0, 0, 0);
    let tile = tile(&test, id, loader.self_ref(), Some(observer));
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tile.cancel();
    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id.canonical));
    test.run_loop.wait_for_empty();

    assert_eq!(changes.get(), 0);
}

// gtest: CustomGeometryTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let loader = loader(Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let first = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref(), None);
    let second = tile(&test, OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0)), loader.self_ref(), None);
    first.set_necessity(TileNecessity::Required);
    second.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The map moves by a world, and TilePyramid changes both copies' wraps.
    first.base().set_id(first.id().unwrap_to(-1));
    second.base().set_id(second.id().unwrap_to(0));

    second.set_necessity(TileNecessity::Optional);
    test.run_loop.wait_for_empty();
    assert_eq!(cancels.load(Ordering::SeqCst), 0);

    // The copy that is still needed is still invalidated, and fetches again.
    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();
    first.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomGeometryTile.CancelledTileDropsCachedData
#[test]
fn cancelled_tile_drops_cached_data() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(Some(fetch), None);

    let id = OverscaledTileID::from_zxy(0, 0, 0);
    {
        let tile = tile(&test, id, loader.self_ref(), None);
        tile.set_necessity(TileNecessity::Required);
        test.run_loop.wait_for_empty();
        assert_eq!(fetches.load(Ordering::SeqCst), 1);

        // The data arrives, but the tile leaves the view before it is parsed.
        loader.self_ref().invoke(move |loader| loader.set_tile_data(&id.canonical, Arc::new(Vec::new())));
        tile.set_necessity(TileNecessity::Optional);
        test.run_loop.wait_for_empty();
    }
    test.run_loop.wait_for_empty();

    // The data went with the last tile that wanted it.
    let next = tile(&test, id, loader.self_ref(), None);
    next.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
