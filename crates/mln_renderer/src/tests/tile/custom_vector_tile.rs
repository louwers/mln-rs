// As in custom_geometry_tile.rs, the loader is an actor on the test's run
// loop, which the test reaches through messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_actor::actor::{Actor, PoolActorRef};
use mln_style::StyleRegistry;
use mln_style::custom_tile_loader::{CustomTileLoader, CustomTileSubscriber, TileFunction};
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Fill;
use mln_style::sources::custom_vector_source::{CustomVectorTileData, CustomVectorTileLoader, TileDataFormat};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::{LatLng, LatLngBounds};
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
) -> Actor<CustomVectorTileLoader> {
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

/// Tiles `ids` of a custom vector source whose loader is `loader`.
fn tiles(test: &RasterTileTest, ids: &[OverscaledTileID], loader: PoolActorRef<CustomVectorTileLoader>) -> OneTile {
    let store = crate::tests::tile_store(&test.run_loop, &test.thread_pool, Box::new(InlineWorkers));
    let origin = DataOrigin::Custom(Arc::new(CustomLoaderOf {
        loader,
        convert: Arc::new(|(data, format): CustomVectorTileData| {
            data.map(|data| match format {
                TileDataFormat::Mvt => mln_tile::TileData::Mvt(mln_tile::MvtTile::new(data)),
                TileDataFormat::Mlt => mln_tile::TileData::Mlt(mln_tile::MltTile::new(data, false)),
            })
        }),
    }));
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
    let loader = loader(&test.run_loop, Some(fetch), None);

    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], loader.self_ref());
    tiles.set_necessity(&root, TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.set_tile_error(&id, &Error::native("failed")));
    test.run_loop.wait_for_empty();
    tiles.store.receive();

    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    tiles.set_necessity(&root, TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomVectorTile.FetchFailedTileAgain
#[test]
fn fetch_failed_tile_again() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(&test.run_loop, Some(fetch), None);

    let id = OverscaledTileID::from_zxy(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.fetch_tile(&id, 0, Box::new(NullSubscriber)));
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    loader.self_ref().invoke(move |loader| loader.set_tile_error(&id.canonical, &Error::native("failed")));

    // A tile that still shows older data doesn't cancel when it leaves the
    // view, so the next request for it has to ask the host again.
    loader.self_ref().invoke(move |loader| loader.fetch_tile(&id, 0, Box::new(NullSubscriber)));
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomVectorTile.CancelOneCopy
#[test]
fn cancel_one_copy() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let (cancels, cancel) = counter();
    let loader = loader(&test.run_loop, Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let second = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles = tiles(&test, &[root, second], loader.self_ref());
    tiles.set_necessity(&root, TileNecessity::Required);
    tiles.set_necessity(&second, TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    // The map moves by a world, and the source changes both copies' wraps.
    let first = root.unwrap_to(-1);
    tiles.rekey(&root, first);
    tiles.rekey(&second, root);

    tiles.set_necessity(&root, TileNecessity::Optional);
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

// gtest: CustomVectorTile.InvalidateRegion
#[test]
fn invalidate_region() {
    let test = RasterTileTest::new();
    let (fetched, fetch) = recorder();
    let (cancelled, cancel) = recorder();
    let loader = loader(&test.run_loop, Some(fetch), Some(cancel));

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
    let mut tiles = tiles(&test, &tile_ids, loader.self_ref());
    for tile_id in &tile_ids {
        tiles.set_necessity(tile_id, TileNecessity::Required);
    }
    test.run_loop.wait_for_empty();
    assert_eq!(fetched.lock().len(), tile_ids.len());

    // Cached data would otherwise answer the refetch.
    for tile_id in &tile_ids {
        let id = tile_id.canonical;
        loader.self_ref().invoke(move |loader| loader.set_tile_data(&id, (None, TileDataFormat::Mvt)));
    }
    test.run_loop.wait_for_empty();
    tiles.store.receive();
    fetched.lock().clear();

    // Northeast of the origin, so it intersects the root tile, 1/1/0, and 2/2/1.
    let bounds = LatLngBounds::hull(LatLng::new(10.0, 10.0).unwrap(), LatLng::new(20.0, 20.0).unwrap());
    loader.self_ref().invoke(move |loader| loader.invalidate_region(&bounds));
    test.run_loop.wait_for_empty();
    tiles.store.receive();

    // The render source updates the necessity of a changed tile.
    for tile_id in &tile_ids {
        tiles.set_necessity(tile_id, TileNecessity::Required);
    }
    test.run_loop.wait_for_empty();
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
    let loader = loader(&test.run_loop, None, None);
    let root = OverscaledTileID::from_zxy(0, 0, 0);
    let mut tiles = tiles(&test, &[root], loader.self_ref());

    let mut layer = Layer::new(&StyleRegistry::with_builtins(), "fill", "water", "source").unwrap();
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn ErasedEvaluatedLayer> =
        Arc::new(EvaluatedLayer::<Fill>::new(LayerStyle::recover(&layer.base_impl()).unwrap()));
    tiles.set_style(&crate::tests::style(&[layer_properties]));
    tiles.set_necessity(&root, TileNecessity::Required);
    test.run_loop.wait_for_empty();

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/test/fixtures/map/issue12432/0-0-0.mlt");
    let data = Arc::new(std::fs::read(path).unwrap());
    loader.self_ref().invoke(move |loader| loader.set_tile_data(&root.canonical, (Some(data), TileDataFormat::Mlt)));
    while !tiles.tile().is_complete() {
        test.run_loop.run_once();
        tiles.update();
    }

    let options = SourceQueryOptions { source_layers: Some(vec!["water".to_owned()]), filter: None };
    let Some(Contents::Geometry(contents)) = tiles.tile().contents.as_deref() else {
        panic!("a layout");
    };
    let mut result = Vec::new();
    contents.query_source_features(&root, false, &mut result, &options, None).unwrap();
    assert_ne!(result, Vec::<mln_util::feature::Feature>::new());
}
