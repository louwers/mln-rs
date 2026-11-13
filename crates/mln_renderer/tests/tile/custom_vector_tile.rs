// As in custom_geometry_tile.rs, the loader is an actor on the test's run
// loop, which the test reaches through messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::scheduler;
use mln_renderer::query::SourceQueryOptions;
use mln_renderer::tile::custom_tile::CustomVectorTile;
use mln_renderer::tile::tile::{Tile, TileNecessity};
use mln_style::custom_tile_loader::{CustomTileLoader, CustomTileSubscriber, TileFunction};
use mln_style::layer::{Layer, downcast_arc};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::fill_layer::Impl as FillLayerImpl;
use mln_style::layers::{FillLayer, FillLayerProperties};
use mln_style::sources::custom_vector_source::{CustomVectorTileData, CustomVectorTileLoader, TileDataFormat};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::{LatLng, LatLngBounds};
use parking_lot::Mutex;

use super::RasterTileTest;

/// The loader actor, on the test's run loop.
fn loader(fetch: Option<TileFunction>, cancel: Option<TileFunction>) -> Actor<CustomVectorTileLoader> {
    Actor::new(scheduler::get_current(), |_| CustomTileLoader::new(fetch, cancel))
}

/// A tile function that counts its calls.
fn counter() -> (Arc<AtomicUsize>, TileFunction) {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
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
    let recorded = ids.clone();
    (ids, Arc::new(move |id: &CanonicalTileID| recorded.lock().push(*id)))
}

fn tile(test: &RasterTileTest, id: OverscaledTileID, loader: PoolActorRef<CustomVectorTileLoader>) -> CustomVectorTile {
    CustomVectorTile::new(id, "source".to_owned(), &test.tile_parameters(), loader, None, false)
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
    let loader = loader(Some(fetch), None);

    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref());
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let id = CanonicalTileID::new(0, 0, 0);
    loader.self_ref().invoke(move |loader| loader.set_tile_error(&id, &Error::native("failed")));
    test.run_loop.wait_for_empty();

    loader.self_ref().invoke(move |loader| loader.invalidate_tile(&id));
    test.run_loop.wait_for_empty();

    // The render source updates the necessity of a changed tile.
    tile.set_necessity(TileNecessity::Required);
    test.run_loop.wait_for_empty();
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// gtest: CustomVectorTile.FetchFailedTileAgain
#[test]
fn fetch_failed_tile_again() {
    let test = RasterTileTest::new();
    let (fetches, fetch) = counter();
    let loader = loader(Some(fetch), None);

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
    let loader = loader(Some(fetch), Some(cancel));

    // Two copies of one tile in neighboring worlds share its data.
    let first = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref());
    let second = tile(&test, OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0)), loader.self_ref());
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

// gtest: CustomVectorTile.InvalidateRegion
#[test]
fn invalidate_region() {
    let test = RasterTileTest::new();
    let (fetched, fetch) = recorder();
    let (cancelled, cancel) = recorder();
    let loader = loader(Some(fetch), Some(cancel));

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
    let tiles: Vec<CustomVectorTile> = tile_ids
        .iter()
        .map(|tile_id| {
            let tile = tile(&test, *tile_id, loader.self_ref());
            tile.set_necessity(TileNecessity::Required);
            tile
        })
        .collect();
    test.run_loop.wait_for_empty();
    assert_eq!(fetched.lock().len(), tile_ids.len());

    // Cached data would otherwise answer the refetch.
    for tile in &tiles {
        let id = tile.id().canonical;
        loader.self_ref().invoke(move |loader| loader.set_tile_data(&id, (None, TileDataFormat::Mvt)));
    }
    test.run_loop.wait_for_empty();
    fetched.lock().clear();

    // Northeast of the origin, so it intersects the root tile, 1/1/0, and 2/2/1.
    let bounds = LatLngBounds::hull(LatLng::new(10.0, 10.0).unwrap(), LatLng::new(20.0, 20.0).unwrap());
    loader.self_ref().invoke(move |loader| loader.invalidate_region(&bounds));
    test.run_loop.wait_for_empty();

    // The render source updates the necessity of a changed tile.
    for tile in &tiles {
        tile.set_necessity(TileNecessity::Required);
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
    let loader = loader(None, None);
    let tile = tile(&test, OverscaledTileID::from_zxy(0, 0, 0), loader.self_ref());

    let mut layer = FillLayer::new("water", "source");
    layer.set_source_layer("water");
    let layer_properties: Arc<dyn LayerProperties> =
        Arc::new(FillLayerProperties::new(downcast_arc::<FillLayerImpl>(&layer.base_impl())));
    tile.set_layers(&[layer_properties], None);

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/test/fixtures/map/issue12432/0-0-0.mlt");
    let data = Arc::new(std::fs::read(path).unwrap());
    tile.set_tile_data((Some(data), TileDataFormat::Mlt));
    while !tile.is_complete() {
        test.run_loop.run_once();
    }

    let options = SourceQueryOptions { source_layers: Some(vec!["water".to_owned()]), filter: None };
    let mut result = Vec::new();
    tile.query_source_features(&mut result, &options, None).unwrap();
    assert_ne!(result, [] as [mln_util::feature::Feature; 0]);
}
