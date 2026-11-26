// C++ calls a CustomTileLoader directly and reaches it from the tiles
// through a mailbox on the test's run loop. Here the provider's calls run
// on the store's pool, a pool without threads, so the host's calls run when
// the test runs the pool, and what the provider sends the tiles waits in
// their store's inbox. The test answers as the host does, through the
// handle the provider calls the host's tile functions with.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use feature::Feature;
use geo_types::Point;
use geojson::GeoJson;
use mln_style::Source;
use mln_style::error::Error;
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::Circle;
use mln_style::sources::custom_geometry_source::{Options, create};
use mln_style::tile_id::CanonicalTileID;

use super::{Host, RasterTileTest, TileFunction, counter, custom_tiles, recorder};
use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::Contents;
use crate::tile_store::store::StoreEvent;
use crate::tile_store::tile::{TileNecessity, TileOperation};

/// A custom geometry source whose host, `host`, calls `fetch` and `cancel`
/// from its tile functions.
fn source(host: &Host<GeoJson>, fetch: Option<TileFunction>, cancel: Option<TileFunction>) -> Source {
    let options = Options {
        fetch_tile_function: Some(host.function(fetch)),
        cancel_tile_function: cancel.map(|cancel| host.function(Some(cancel))),
        ..Options::default()
    };
    create("source", options).expect("a custom geometry source")
}

/// Tiles `ids` of a custom geometry source whose host, `host`, calls `fetch`
/// and `cancel` from its tile functions.
fn tiles(
    test: &RasterTileTest,
    ids: &[OverscaledTileID],
    host: &Host<GeoJson>,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> OneTile {
    custom_tiles(test, &source(host, fetch, cancel), ids)
}

/// A tile function that does nothing.
fn ignore() -> TileFunction {
    Arc::new(|_: &CanonicalTileID| {})
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
    let host = Host::new();

    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), Some(ignore()));
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();

    assert_eq!(*fetched.lock(), vec![CanonicalTileID::new(0, 0, 0)]);
}

// gtest: CustomGeometryTile.InvokeCancelTile
#[test]
fn invoke_cancel_tile() {
    let test = RasterTileTest::new();
    let (cancelled, cancel) = recorder();
    let host = Host::new();

    let mut tiles = tiles(&test, &[root()], &host, Some(ignore()), Some(cancel));
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
    let host = Host::new();

    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), None);
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
    let host = Host::new();

    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), None);
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    host.handle().set_tile_error(id, Error::native("failed"));
    test.local_pool.run_all();
    tiles.store.receive();
    assert_eq!(tiles.errors(), ["failed"]);

    host.handle().invalidate_tile(id);
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
    let host = Host::new();
    let mut tiles = tiles(&test, &[root()], &host, Some(ignore()), None);
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    tiles.store.take_events();

    host.handle().set_tile_error(CanonicalTileID::new(0, 0, 0), Error::native("failed"));
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
    let host = Host::new();
    let mut tiles = tiles(&test, &[root()], &host, None, None);

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
    contents.query_source_features(&root(), &mut result, &SourceQueryOptions::default(), None).unwrap();
    assert_eq!(result.len(), 1);
}

// gtest: CustomGeometryTile.CancellationDiscardsQueuedInvalidation
#[test]
fn cancellation_discards_queued_invalidation() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    // The provider's message to the tile waits in the store's inbox until
    // the test takes it in.
    let host = Host::new();
    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), None);
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    host.handle().invalidate_tile(root().canonical);
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
    let host = Host::new();
    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), None);
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    tiles.drop_tile();
    host.handle().invalidate_tile(root().canonical);
    test.local_pool.run_all();

    assert_eq!(tiles.changes(), 0);
}

// gtest: CustomGeometryTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let host = Host::new();

    // Two copies of one tile in neighboring worlds share its data.
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root(), second], &host, Some(fetch), Some(cancel));
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
    host.handle().invalidate_tile(id);
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
    let host = Host::new();

    let mut tiles = tiles(&test, &[root()], &host, Some(fetch), None);
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The data arrives, but the tile leaves the view before it is parsed.
    let empty = geojson::FeatureCollection { bbox: None, features: Vec::new(), foreign_members: None };
    host.handle().set_tile_data(root().canonical, GeoJson::FeatureCollection(empty));
    tiles.set_necessity(&root(), TileNecessity::Optional);
    test.local_pool.run_all();
    tiles.drop_tile();
    test.local_pool.run_all();

    // The data went with the last tile that wanted it.
    tiles.start(root());
    tiles.set_necessity(&root(), TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
