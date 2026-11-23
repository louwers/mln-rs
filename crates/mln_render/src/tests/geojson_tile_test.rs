// The tiles of a GeoJSON source get features that the source cut from its
// data; here the test gives them the features.

use std::sync::Arc;

use feature::Feature;
use geo_types::Point;
use mln_map_types::MapMode;
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::pool::TaskGroup;

use crate::Registry;
use crate::erased_layer::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::tests::fixture_layer::Dot;
use crate::tests::scripted_origin::Script;
use crate::tests::{OneTile, style};
use crate::tile::tile::{TileInput, TileKind};
use crate::tile::tile_workers::TileWorkers;

struct GeoJSONTileTest {
    thread_pool: TaskGroup,
}

impl GeoJSONTileTest {
    fn new() -> Self {
        GeoJSONTileTest { thread_pool: TaskGroup::new(&crate::tests::pool()) }
    }

    /// Tiles `ids` of a source of one layer of features, whose jobs run on
    /// `workers`, or on this thread at once for a source with `synchronous`
    /// updates; the tiles get their features before their style.
    fn tiles(&self, ids: &[OverscaledTileID], workers: Box<dyn TileWorkers>, synchronous: bool) -> OneTile {
        let store = crate::tests::tile_store(&self.thread_pool, workers);
        let mut tiles =
            OneTile::with_script(store, TileKind::Geometry, true, &Script::default(), ids, MapMode::Continuous);
        let key = tiles.key;
        tiles.store.source_mut(key).expect("the source").0.info.synchronous = synchronous;
        for id in ids {
            tiles.with_at(id, |tile| tile.take_input(point_features()));
        }
        tiles
    }

    fn pool(&self) -> Box<dyn TileWorkers> {
        Box::new(crate::tile::tile_workers::PoolWorkers::new(self.thread_pool.clone(), 8))
    }
}

impl Drop for GeoJSONTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

/// A point's features, as a load delivers them.
fn point_features() -> TileInput {
    let features = vec![Feature { geometry: Some(Point::new(0i16, 0).into()), ..Feature::default() }];
    TileInput::Geometry { data: Some(TileData::Features(Arc::new(features))), missing: false }
}

fn dot_layer_properties() -> Arc<dyn ErasedEvaluatedLayer> {
    let layer = Layer::new(Registry::with_builtins().style(), "dot", "circle", "source").unwrap();
    Arc::new(EvaluatedLayer::<Dot>::new(LayerStyle::recover(layer.style()).unwrap()))
}

fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

// gtest: GeoJSONTile.SynchronousUpdate
#[test]
fn synchronous_update() {
    let test = GeoJSONTileTest::new();
    let mut tiles = test.tiles(&[root()], Box::new(crate::tile::tile_workers::InlineWorkers), true);
    tiles.set_style(&style(&[dot_layer_properties()]));
    assert!(tiles.tile().is_complete());
    assert!(tiles.tile().is_renderable());
}

// gtest: GeoJSONTile.Issue7648
#[test]
fn issue_7648() {
    let test = GeoJSONTileTest::new();
    let mut tiles = test.tiles(&[root()], test.pool(), false);
    let layer_properties = dot_layer_properties();
    tiles.set_style(&style(&[Arc::clone(&layer_properties)]));
    tiles.settle(&[root()]);
    assert!(tiles.changes() >= 1);

    tiles.with(|tile| tile.take_input(point_features()));
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
    let mut tiles = test.tiles(&[root()], test.pool(), false);

    let layer_properties = dot_layer_properties();
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

/// A dot layer as its render layer evaluates it, which records what the
/// worker codec sends to evaluate it again.
fn evaluated_dot_layer_properties() -> Arc<dyn ErasedEvaluatedLayer> {
    let layer = Layer::new(Registry::with_builtins().style(), "dot", "circle", "source").unwrap();
    let mut render_layer = crate::tests::render_layer(layer.style());
    render_layer.evaluate(&PropertyEvaluationParameters::new(0.0));
    render_layer.evaluated()
}

// Port-only: tiles whose workers are in worker instances on other threads
// get their layouts back through the codec, and a data update replaces them.
#[test]
fn update_through_the_codec() {
    let test = GeoJSONTileTest::new();
    let workers = Box::new(crate::tests::tile::remote_workers(2));
    let layer_properties = evaluated_dot_layer_properties();
    // Two tiles, so both instances get one.
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    let mut tiles = test.tiles(&ids, workers, false);
    let (style, params) = (style(std::slice::from_ref(&layer_properties)), tiles.params());
    for id in &ids {
        tiles.with_at(id, |tile| tile.set_style(&style, params.pixel_ratio, params.subdivision_granularity));
    }
    tiles.settle(&ids);
    for id in &ids {
        assert!(tiles.tile_at(id).is_renderable());
        assert!(tiles.with_at(id, |tile| tile.parts().1.layer_properties_updated(&layer_properties)));
    }

    tiles.with_at(&ids[0], |tile| tile.take_input(point_features()));
    assert!(!tiles.tile_at(&ids[0]).is_complete());
    tiles.settle(&ids[..1]);
    assert!(tiles.tile_at(&ids[0]).is_renderable());
}
