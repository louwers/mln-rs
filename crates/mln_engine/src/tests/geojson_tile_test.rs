use std::sync::Arc;

use feature::Feature;
use geo_types::Point;
use mln_style::error::Error;
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::Circle;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::sources::geojson::{Features, GeoJSONData, TileFeatures};
use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::OverscaledTileID;
use mln_util::pool::TaskGroup;

use crate::Registry;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::map::MapMode;
use crate::tests::{OneTile, style};
use crate::tile_store::loading::DataOrigin;
use crate::tile_store::tile::TileKind;
use crate::worker::tile_workers::TileWorkers;

struct GeoJSONTileTest {
    thread_pool: TaskGroup,
}

impl GeoJSONTileTest {
    fn new() -> Self {
        GeoJSONTileTest { thread_pool: TaskGroup::new(&crate::tests::pool()) }
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
        let store = crate::tests::tile_store(&self.thread_pool, workers);
        let origin = DataOrigin::GeoJson { data, cuts: TaskGroup::new(self.thread_pool.pool()) };
        let mut tiles = OneTile::with_origin(store, TileKind::Geometry, true, origin, ids, MapMode::Continuous);
        let key = tiles.key;
        tiles.store.source_mut(key).expect("the source").0.info.synchronous = synchronous;
        tiles
    }

    fn pool(&self) -> Box<dyn TileWorkers> {
        Box::new(crate::worker::tile_workers::PoolWorkers::new(self.thread_pool.clone(), 8))
    }
}

impl Drop for GeoJSONTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

#[derive(Debug)]
struct FakeGeoJSONData {
    features: Arc<TileFeatures>,
}

impl GeoJSONData for FakeGeoJSONData {
    fn get_tile(&self, _id: &CanonicalTileID) -> Arc<TileFeatures> {
        Arc::clone(&self.features)
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
    Arc::new(FakeGeoJSONData { features: Arc::new(features) })
}

fn circle_layer_properties() -> Arc<dyn ErasedEvaluatedLayer> {
    let layer = Layer::new(Registry::with_builtins().style(), "circle", "circle", "source").unwrap();
    Arc::new(EvaluatedLayer::<Circle>::new(LayerStyle::recover(layer.style()).unwrap()))
}

fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: GeoJSONTile.SynchronousUpdate
#[test]
fn synchronous_update() {
    let test = GeoJSONTileTest::new();
    let mut tiles = test.tiles(point_data(), &[root()], Box::new(crate::worker::tile_workers::InlineWorkers), true);
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
    tiles.settle(&[root()]);
    assert!(tiles.changes() >= 1);

    tiles.with(|tile| tile.load_geojson(false));
    // Once present, the bucket should never "disappear", which would cause
    // flickering.
    while !tiles.tile().is_complete() {
        assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));
        tiles.update();
    }
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));
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
    tiles.settle(&[root()]);

    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));

    // Make sure that once we've had a renderable tile and then receive
    // erroneous data, we retain the previously rendered data and keep the
    // tile renderable.
    tiles.with(|tile| tile.fail(Error::native("Connection offline")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));

    // Then simulate a parsing failure and make sure that we keep it
    // renderable in this situation as well.
    tiles.with(|tile| tile.fail(Error::native("Parse error")));
    assert!(tiles.tile().is_renderable());
    assert!(tiles.with(|tile| tile.parts().1.layer_properties_updated(&layer_properties)));
}

/// A circle layer as its render layer evaluates it, which records what the
/// worker codec sends to evaluate it again.
fn evaluated_circle_layer_properties() -> Arc<dyn ErasedEvaluatedLayer> {
    let layer = Layer::new(Registry::with_builtins().style(), "circle", "circle", "source").unwrap();
    let mut render_layer = crate::tests::render_layer(layer.style());
    render_layer.evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.base().evaluated_properties)
}

// Port-only: tiles whose workers are in worker instances on other threads
// get their layouts back through the codec, and a data update replaces them.
#[test]
fn update_through_the_codec() {
    let test = GeoJSONTileTest::new();
    let workers = Box::new(crate::tests::tile::remote_workers(2));
    let layer_properties = evaluated_circle_layer_properties();
    // Two tiles, so both instances get one.
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    let mut tiles = test.tiles(point_data(), &ids, workers, false);
    let (style, params) = (style(std::slice::from_ref(&layer_properties)), tiles.params());
    for id in &ids {
        tiles.with_at(id, |tile| tile.set_style(&style, params.pixel_ratio, params.subdivision_granularity));
    }
    tiles.settle(&ids);
    for id in &ids {
        assert!(tiles.tile_at(id).is_renderable());
        assert!(tiles.with_at(id, |tile| tile.parts().1.layer_properties_updated(&layer_properties)));
    }

    let origin = DataOrigin::GeoJson { data: point_data(), cuts: TaskGroup::new(test.thread_pool.pool()) };
    tiles.with_at(&ids[0], |tile| {
        tile.parts().0.origin = Some(origin);
        tile.load_geojson(false);
    });
    assert!(!tiles.tile_at(&ids[0]).is_complete());
    tiles.settle(&ids[..1]);
    assert!(tiles.tile_at(&ids[0]).is_renderable());
}
