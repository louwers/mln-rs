use std::sync::Arc;

use feature::Feature;
use geo_types::Point;
use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map_types::MapMode;
use mln_style::StyleRegistry;
use mln_style::layer::{Layer, downcast_arc};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::CircleLayerProperties;
use mln_style::layers::circle_layer::Impl as CircleLayerImpl;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style::sources::geojson_source::{Features, GeoJSONData, TileFeatures};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;

use crate::tests::{OneTile, style};
use crate::tile::loading::DataOrigin;
use crate::tile::tile::TileKind;
use crate::tile::tile_workers::TileWorkers;

struct GeoJSONTileTest {
    run_loop: RunLoop,
    thread_pool: TaggedScheduler,
}

impl GeoJSONTileTest {
    fn new() -> Self {
        GeoJSONTileTest {
            run_loop: RunLoop::new(),
            thread_pool: TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new()),
        }
    }

    /// Tiles `ids` of a GeoJSON source of `data`, whose jobs run on
    /// `workers`, or on this thread at once for a source with `synchronous`
    /// updates; the tiles get their data before their style.
    fn tiles(
        &self,
        data: Arc<dyn GeoJSONData>,
        ids: &[OverscaledTileID],
        workers: Box<dyn TileWorkers>,
        synchronous: bool,
    ) -> OneTile {
        let store = crate::tests::tile_store(&self.run_loop, &self.thread_pool, workers);
        let mut tiles =
            OneTile::with_origin(store, TileKind::Geometry, true, DataOrigin::GeoJson(data), ids, MapMode::Continuous);
        let key = tiles.key;
        tiles.store.source_mut(key).expect("the source").0.info.synchronous = synchronous;
        tiles
    }

    fn pool(&self) -> Box<dyn TileWorkers> {
        Box::new(crate::tile::tile_workers::PoolWorkers::new(self.thread_pool.clone(), 8))
    }

    /// Runs the loop and updates `tiles` until the tiles `ids` are complete.
    fn settle(&self, tiles: &mut OneTile, ids: &[OverscaledTileID]) {
        let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ids.iter().all(|id| tiles.tile_at(id).is_complete()) {
            assert!(mln_util::time::Instant::now() < deadline, "the tiles didn't complete");
            self.run_loop.run_once();
            tiles.update();
            std::thread::yield_now();
        }
    }
}

impl Drop for GeoJSONTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait_for_empty();
    }
}

#[derive(Debug)]
struct FakeGeoJSONData {
    features: TileFeatures,
}

impl GeoJSONData for FakeGeoJSONData {
    fn get_tile(
        &self,
        _id: &CanonicalTileID,
        callback: Box<dyn FnOnce(TileFeatures) + Send>,
        _run_synchronously: bool,
    ) {
        callback(self.features.clone());
    }

    fn get_children(&self, _cluster_id: u32) -> Result<Features, Error> {
        Ok(Features::new())
    }

    fn get_leaves(&self, _cluster_id: u32, _limit: u32, _offset: u32) -> Result<Features, Error> {
        Ok(Features::new())
    }

    fn get_cluster_expansion_zoom(&self, _cluster_id: u32) -> Result<u8, Error> {
        Ok(0)
    }
}

fn point_data() -> Arc<dyn GeoJSONData> {
    let features = vec![Feature { geometry: Some(Point::new(0i16, 0).into()), ..Feature::default() }];
    Arc::new(FakeGeoJSONData { features })
}

fn circle_layer_properties() -> Arc<dyn LayerProperties> {
    let layer = Layer::new(&StyleRegistry::with_builtins(), "circle", "circle", "source").unwrap();
    Arc::new(CircleLayerProperties::new(downcast_arc::<CircleLayerImpl>(&layer.base_impl())))
}

fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: GeoJSONTile.SynchronousUpdate
#[test]
fn synchronous_update() {
    let test = GeoJSONTileTest::new();
    let mut tiles = test.tiles(point_data(), &[root()], Box::new(crate::tile::tile_workers::InlineWorkers), true);
    tiles.set_style(&style(&[circle_layer_properties()]));
    assert!(tiles.tile().is_complete());
    assert!(tiles.tile().is_renderable());
}

// gtest: GeoJSONTile.Issue7648
#[test]
fn issue_7648() {
    let test = GeoJSONTileTest::new();
    let data = point_data();
    let mut tiles = test.tiles(Arc::clone(&data), &[root()], test.pool(), false);
    let layer_properties = circle_layer_properties();
    tiles.set_style(&style(&[Arc::clone(&layer_properties)]));
    test.settle(&mut tiles, &[root()]);
    assert!(tiles.changes() >= 1);

    tiles.with(|info, tile, ctx| info.load_geojson(tile, ctx, &data, false));
    // Once present, the bucket should never "disappear", which would cause
    // flickering.
    while !tiles.tile().is_complete() {
        assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));
        test.run_loop.run_once();
        tiles.update();
    }
    assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));
    assert!(tiles.changes() >= 1);
}

// Tests that tiles remain renderable if they have been renderable and then
// had an error sent to them, e.g. when revalidating/refreshing the request.
// gtest: GeoJSONTile.Issue9927
#[test]
fn issue_9927() {
    let test = GeoJSONTileTest::new();
    let mut tiles = test.tiles(point_data(), &[root()], test.pool(), false);

    let layer_properties = circle_layer_properties();
    tiles.set_style(&style(&[Arc::clone(&layer_properties)]));
    test.settle(&mut tiles, &[root()]);

    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));

    // Make sure that once we've had a renderable tile and then receive
    // erroneous data, we retain the previously rendered data and keep the
    // tile renderable.
    tiles.with(|info, tile, ctx| info.load_error(tile, ctx, Error::native("Connection offline")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));

    // Then simulate a parsing failure and make sure that we keep it
    // renderable in this situation as well.
    tiles.with(|info, tile, ctx| info.job_error(tile, ctx, Error::native("Parse error")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|_, tile, _| tile.layer_properties_updated(&layer_properties)));
}

/// A circle layer as its render layer evaluates it, which records what the
/// worker codec sends to evaluate it again.
fn evaluated_circle_layer_properties() -> Arc<dyn LayerProperties> {
    let layer = Layer::new(&StyleRegistry::with_builtins(), "circle", "circle", "source").unwrap();
    let render_layer = crate::layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.borrow().base().evaluated_properties)
}

// Port-only: tiles whose workers are in worker instances on other threads
// get their layouts back through the codec, and a data update replaces them.
#[test]
fn update_through_the_codec() {
    let test = GeoJSONTileTest::new();
    let workers = crate::tests::tile::remote_workers(&test.run_loop, 2);
    let workers = Box::new(crate::tile::worker_codec::RemoteJobs(workers));
    let layer_properties = evaluated_circle_layer_properties();
    // Two tiles, so both instances get one.
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    let mut tiles = test.tiles(point_data(), &ids, workers, false);
    for id in &ids {
        tiles.with_at(id, |info, tile, ctx| info.set_style(tile, ctx, &style(std::slice::from_ref(&layer_properties))));
    }
    test.settle(&mut tiles, &ids);
    for id in &ids {
        assert!(tiles.tile_at(id).is_renderable());
        assert!(tiles.with_at(id, |_, tile, _| tile.layer_properties_updated(&layer_properties)));
    }

    let data = point_data();
    tiles.with_at(&ids[0], |info, tile, ctx| info.load_geojson(tile, ctx, &data, false));
    assert!(!tiles.tile_at(&ids[0]).is_complete());
    test.settle(&mut tiles, &ids[..1]);
    assert!(tiles.tile_at(&ids[0]).is_renderable());
}
