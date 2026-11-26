// As in custom_geometry_tile.rs, the provider's calls run on the test's
// local pool, and the test answers through the handle the provider calls
// the host's tile functions with.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mln_style::Source;
use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::Fill;
use mln_style::sources::custom_vector_source::{CustomVectorTileData, Options, TileDataFormat, create};
use mln_style::tile_id::CanonicalTileID;

use super::{Host, RasterTileTest, TileFunction, counter, custom_tiles, recorder};
use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::Contents;
use crate::tile_store::custom_tile_loader::{CustomProvider, CustomTileSubscriber, CustomVectorProvider, HostFunction};
use crate::tile_store::tile::TileNecessity;

/// The provider alone, whose calls run on `pool`.
fn loader(
    pool: &crate::exec::pool::Pool,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> Arc<CustomVectorProvider> {
    let host = |function: Option<TileFunction>| -> Option<HostFunction> {
        function.map(|function| -> HostFunction { Box::new(move |tile_id| function(tile_id)) })
    };
    CustomProvider::new(pool, |_| (host(fetch), host(cancel)))
}

/// A custom vector source whose host, `host`, calls `fetch` and `cancel`
/// from its tile functions.
fn source(host: &Host<CustomVectorTileData>, fetch: Option<TileFunction>, cancel: Option<TileFunction>) -> Source {
    let options = Options {
        fetch_tile_function: Some(host.function(fetch)),
        cancel_tile_function: cancel.map(|cancel| host.function(Some(cancel))),
        ..Options::default()
    };
    create("source", options)
}

/// Tiles `ids` of a custom vector source whose host, `host`, calls `fetch`
/// and `cancel` from its tile functions.
fn tiles(
    test: &RasterTileTest,
    ids: &[OverscaledTileID],
    host: &Host<CustomVectorTileData>,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> OneTile {
    custom_tiles(test, &source(host, fetch, cancel), ids)
}

/// A subscriber that ignores what the loader sends it.
struct NullSubscriber;

impl CustomTileSubscriber<CustomVectorTileData> for NullSubscriber {
    fn set_tile_data(&self, _data: CustomVectorTileData) {}
    fn set_tile_error(&self, _error: Error) {}
    fn invalidate_tile_data(&self) {}
}

// gtest: CustomVectorTile.InvalidateFailedTile
#[test]
fn invalidate_failed_tile() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let host = Host::new();

    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], &host, Some(fetch), None);
    tiles.set_necessity(&root, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    host.handle().set_tile_error(id, Error::native("failed"));
    test.local_pool.run_all();
    tiles.store.receive();

    host.handle().invalidate_tile(id);
    test.local_pool.run_all();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    tiles.set_necessity(&root, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomVectorTile.FetchFailedTileAgain
#[test]
fn fetch_failed_tile_again() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.local_pool, Some(fetch), None);

    let id = OverscaledTileID::from_zxy(0, 0, 0);
    loader.fetch_tile(id, 0, Box::new(NullSubscriber));
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    loader.set_tile_error(id.canonical, Error::native("failed"));

    // A tile that still shows older data doesn't cancel when it leaves the
    // view, so the next request for it has to ask the host again.
    loader.fetch_tile(id, 0, Box::new(NullSubscriber));
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomVectorTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let host = Host::new();

    // Two copies of one tile in neighboring worlds share its data.
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root, second], &host, Some(fetch), Some(cancel));
    tiles.set_necessity(&root, TileNecessity::Required);
    tiles.set_necessity(&second, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The map moves by a world, and the source changes both copies' wraps.
    let first = root.unwrap_to(-1);
    tiles.rekey(&root, first);
    tiles.rekey(&second, root);

    tiles.set_necessity(&root, TileNecessity::Optional);
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

// gtest: CustomVectorTile.InvalidateRegion
#[test]
fn invalidate_region() {
    let test = RasterTileTest::new();
    let (fetched, fetch) = recorder();
    let (cancelled, cancel) = recorder();
    let host = Host::new();

    // The root tile, every tile at zoom 1, and two tiles at zoom 2.
    let mut tile_ids = vec![
        OverscaledTileID::from_zxy(0, 0, 0),
        OverscaledTileID::from_zxy(2, 2, 1),
        OverscaledTileID::from_zxy(2, 1, 1),
    ];
    for y in 0..2 {
        for x in 0..2 {
            tile_ids.push(OverscaledTileID::from_zxy(1, x, y));
        }
    }
    let mut tiles = tiles(&test, &tile_ids, &host, Some(fetch), Some(cancel));
    for tile_id in &tile_ids {
        tiles.set_necessity(tile_id, TileNecessity::Required);
    }
    test.local_pool.run_all();
    assert_eq!(fetched.lock().len(), tile_ids.len());

    // Cached data would otherwise answer the refetch.
    for tile_id in &tile_ids {
        let id = tile_id.canonical;
        host.handle().set_tile_data(id, (None, TileDataFormat::Mvt));
    }
    test.local_pool.run_all();
    tiles.store.receive();
    fetched.lock().clear();

    // Northeast of the origin, so it intersects the root tile, 1/1/0, and 2/2/1.
    let bounds = LatLngBounds::hull(LatLng::new(10.0, 10.0).unwrap(), LatLng::new(20.0, 20.0).unwrap());
    host.handle().invalidate_region(bounds);
    test.local_pool.run_all();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    for tile_id in &tile_ids {
        tiles.set_necessity(tile_id, TileNecessity::Required);
    }
    test.local_pool.run_all();
    let intersecting =
        vec![CanonicalTileID::new(0, 0, 0), CanonicalTileID::new(1, 1, 0), CanonicalTileID::new(2, 2, 1)];
    let mut cancelled = cancelled.lock().clone();
    let mut fetched = fetched.lock().clone();
    cancelled.sort();
    fetched.sort();
    assert_eq!(cancelled, intersecting);
    assert_eq!(fetched, intersecting);
}

// Port-only: a tile decodes MLT data as MLT (ffi patch 0052).
#[test]
fn set_mlt_tile_data() {
    let test = RasterTileTest::new();
    let host = Host::new();
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], &host, None, None);

    let mut layer = Layer::new(Registry::with_builtins().style(), "fill", "water", "source").unwrap();
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Fill>::new(LayerStyle::recover(layer.style()).unwrap()));
    tiles.set_style(&crate::tests::style(&[layer_properties]));
    tiles.set_necessity(&root, TileNecessity::Required);
    test.local_pool.run_all();

    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/test/fixtures/map/issue12432/0-0-0.mlt");
    let data = Arc::new(std::fs::read(path).unwrap());
    host.handle().set_tile_data(root.canonical, (Some(data), TileDataFormat::Mlt));
    while !tiles.tile().is_complete() {
        test.local_pool.run_all();
        tiles.update();
    }

    let options = SourceQueryOptions { source_layers: Some(vec!["water".to_owned()]), filter: None };
    let Some(Contents::Geometry(contents)) = tiles.tile().contents() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root, &mut result, &options, None).unwrap();
    assert_ne!(result, Vec::<crate::tile::feature::Feature>::new());
}
