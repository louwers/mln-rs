// As in custom_geometry_tile.rs, the provider's calls run on the test's
// local pool.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::Fill;
use mln_style::sources::custom_vector_source::{CustomVectorTileData, TileDataFormat};
use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::OverscaledTileID;
use parking_lot::Mutex;

use super::RasterTileTest;
use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::query::SourceQueryOptions;
use crate::tests::OneTile;
use crate::tile_store::contents::Contents;
use crate::tile_store::custom_tile_loader::{CustomProvider, CustomTileSubscriber, CustomVectorProvider, HostFunction};
use crate::tile_store::loading::DataOrigin;
use crate::tile_store::tile::{TileKind, TileNecessity};
use crate::worker::tile_workers::InlineWorkers;

/// A host's tile function, as the tests share it.
type TileFunction = Arc<dyn Fn(&CanonicalTileID) + Send + Sync>;

/// The provider, whose calls run on `pool`.
fn loader(
    pool: &mln_util::pool::Pool,
    fetch: Option<TileFunction>,
    cancel: Option<TileFunction>,
) -> Arc<CustomVectorProvider> {
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

/// Tiles `ids` of a custom vector source whose provider is `loader`.
fn tiles(test: &RasterTileTest, ids: &[OverscaledTileID], loader: Arc<CustomVectorProvider>) -> OneTile {
    let store = crate::tests::tile_store(&test.thread_pool, Box::new(InlineWorkers));
    let origin = DataOrigin::Custom { loader, fast_pfor_enabled: false };
    OneTile::with_origin(store, TileKind::Geometry, false, origin, ids, mln_map_types::MapMode::Static)
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
    let loader = loader(&test.local_pool, Some(fetch), None);

    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], Arc::clone(&loader));
    tiles.set_necessity(&root, TileNecessity::Required);
    test.local_pool.run_all();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.set_tile_error(id, Error::native("failed"));
    test.local_pool.run_all();
    tiles.store.receive();

    loader.invalidate_tile(id);
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
    let loader = loader(&test.local_pool, Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root, second], Arc::clone(&loader));
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
    loader.invalidate_tile(id);
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
    let loader = loader(&test.local_pool, Some(fetch), Some(cancel));

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
    let mut tiles = tiles(&test, &tile_ids, Arc::clone(&loader));
    for tile_id in &tile_ids {
        tiles.set_necessity(tile_id, TileNecessity::Required);
    }
    test.local_pool.run_all();
    assert_eq!(fetched.lock().len(), tile_ids.len());

    // Cached data would otherwise answer the refetch.
    for tile_id in &tile_ids {
        let id = tile_id.canonical;
        loader.set_tile_data(id, (None, TileDataFormat::Mvt));
    }
    test.local_pool.run_all();
    tiles.store.receive();
    fetched.lock().clear();

    // Northeast of the origin, so it intersects the root tile, 1/1/0, and 2/2/1.
    let bounds = LatLngBounds::hull(LatLng::new(10.0, 10.0).unwrap(), LatLng::new(20.0, 20.0).unwrap());
    loader.invalidate_region(bounds);
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
    let loader = loader(&test.local_pool, None, None);
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], Arc::clone(&loader));

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
    loader.set_tile_data(root.canonical, (Some(data), TileDataFormat::Mlt));
    while !tiles.tile().is_complete() {
        test.local_pool.run_all();
        tiles.update();
    }

    let options = SourceQueryOptions { source_layers: Some(vec!["water".to_owned()]), filter: None };
    let Some(Contents::Geometry(contents)) = tiles.tile().contents() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root, false, &mut result, &options, None).unwrap();
    assert_ne!(result, Vec::<mln_util::feature::Feature>::new());
}
