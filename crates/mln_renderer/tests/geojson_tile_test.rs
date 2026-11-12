// Ports test/tile/geojson_tile.test.cpp.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use feature::Feature;
use geometry::{Geometry, Point};
use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_renderer::tile::geojson_tile::GeoJSONTile;
use mln_renderer::tile::tile::{Tile, TileBase};
use mln_renderer::tile::tile_observer::TileObserver;
use mln_renderer::tile::worker_codec::{SerializingTransport, ThreadWorkerInstances};
use mln_renderer::tile_parameters::TileParameters;
use mln_style::layer::{Layer, static_immutable_cast};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::circle_layer::Impl as CircleLayerImpl;
use mln_style::layers::{CircleLayer, CircleLayerProperties};
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style::sources::geojson_source::{Features, GeoJSONData, TileFeatures};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

/// `GeoJSONTileTest`.
struct GeoJSONTileTest {
    run_loop: RunLoop,
    transform: Transform,
    thread_pool: TaggedScheduler,
}

impl GeoJSONTileTest {
    fn new() -> Self {
        GeoJSONTileTest {
            run_loop: RunLoop::new(),
            transform: Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default),
            thread_pool: TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new()),
        }
    }

    fn tile_parameters(&self) -> TileParameters<'_> {
        TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: self.transform.get_state(),
            file_source: None,
            mode: MapMode::Continuous,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: mln_renderer::image_manager::ImageManager::create(),
            glyph_manager: mln_renderer::text::glyph_manager::GlyphManager::new(None),
            dynamic_texture_atlas: std::sync::Arc::new(
                mln_renderer::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new(),
            ),
            tile_worker_transport: std::rc::Rc::new(
                mln_renderer::tile::tile_worker_transport::InProcessTransport::new(self.thread_pool.clone()),
            ),
        }
    }
}

/// `FakeGeoJSONData`.
#[derive(Debug)]
struct FakeGeoJSONData {
    features: TileFeatures,
}

impl GeoJSONData for FakeGeoJSONData {
    fn get_tile(&self, _id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures)>, _run_synchronously: bool) {
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
    let features = vec![Feature::new(Geometry::Point(Point::new(0i16, 0)))];
    Arc::new(FakeGeoJSONData { features })
}

fn circle_layer_properties() -> Immutable<dyn LayerProperties> {
    let layer = CircleLayer::new("circle", "source");
    Arc::new(CircleLayerProperties::new(static_immutable_cast::<CircleLayerImpl>(&layer.base_impl())))
}

type OnTileChanged = Box<dyn Fn(&TileBase)>;

/// `StubTileObserver` (`test/src/mln/test/stub_tile_observer.hpp`).
#[derive(Default)]
struct StubTileObserver {
    tile_changed: RefCell<Option<OnTileChanged>>,
}

impl TileObserver for StubTileObserver {
    fn on_tile_changed(&self, tile: &TileBase) {
        if let Some(f) = &*self.tile_changed.borrow() {
            f(tile);
        }
    }
}

// gtest: GeoJSONTile.SynchronousUpdate
#[test]
fn synchronous_update() {
    let test = GeoJSONTileTest::new();

    let data = point_data();
    let mut tile_parameters = test.tile_parameters();
    tile_parameters.is_update_synchronous = true;
    let tile = GeoJSONTile::new(OverscaledTileID::from_zxy(0, 0, 0), "source".to_owned(), &tile_parameters, data, None);
    let layers = vec![circle_layer_properties()];
    tile.set_layers(&layers, tile_parameters.global_state.clone());
    assert!(tile.is_complete());
    assert!(tile.is_renderable());
}

// gtest: GeoJSONTile.Issue7648
#[test]
fn issue_7648() {
    let test = GeoJSONTileTest::new();

    let data = point_data();
    let tile_parameters = test.tile_parameters();
    let tile = Rc::new(GeoJSONTile::new(
        OverscaledTileID::from_zxy(0, 0, 0),
        "source".to_owned(),
        &tile_parameters,
        data.clone(),
        None,
    ));
    let layer_properties = circle_layer_properties();
    let observer = Rc::new(StubTileObserver::default());
    let changed = Rc::new(RefCell::new(0));
    {
        let weak: Weak<GeoJSONTile> = Rc::downgrade(&tile);
        let layer_properties = layer_properties.clone();
        let changed = changed.clone();
        *observer.tile_changed.borrow_mut() = Some(Box::new(move |_tile| {
            // Once present, the bucket should never "disappear", which would
            // cause flickering.
            let tile = weak.upgrade().unwrap();
            assert!(tile.layer_properties_updated(&layer_properties));
            *changed.borrow_mut() += 1;
        }));
    }

    let layers = vec![layer_properties.clone()];
    tile.set_layers(&layers, tile_parameters.global_state.clone());
    tile.set_observer(observer.clone());

    while !tile.is_complete() {
        test.run_loop.run_once();
    }

    tile.update_data(data, false, tile_parameters.is_update_synchronous);
    while !tile.is_complete() {
        test.run_loop.run_once();
    }
    assert!(*changed.borrow() >= 1);
}

// Tests that tiles remain renderable if they have been renderable and then
// had an error sent to them, e.g. when revalidating/refreshing the request.
// gtest: GeoJSONTile.Issue9927
#[test]
fn issue_9927() {
    let test = GeoJSONTileTest::new();

    let data = point_data();
    let tile_parameters = test.tile_parameters();
    let tile = GeoJSONTile::new(OverscaledTileID::from_zxy(0, 0, 0), "source".to_owned(), &tile_parameters, data, None);

    let layer_properties = circle_layer_properties();
    let layers = vec![layer_properties.clone()];
    tile.set_layers(&layers, tile_parameters.global_state.clone());

    while !tile.is_complete() {
        test.run_loop.run_once();
    }

    assert!(tile.is_renderable());
    assert!(tile.layer_properties_updated(&layer_properties));

    // Make sure that once we've had a renderable tile and then receive
    // erroneous data, we retain the previously rendered data and keep the
    // tile renderable.
    tile.geometry_tile().set_error(Error::native("Connection offline"));
    assert!(tile.is_renderable());
    assert!(tile.layer_properties_updated(&layer_properties));

    // Then simulate a parsing failure and make sure that we keep it
    // renderable in this situation as well. We're using 3 as a correlationID
    // since we've done two three calls that increment this counter (as part
    // of the GeoJSONTile constructor, setLayers, and setPlacementConfig).
    tile.geometry_tile().inner().on_error(Error::native("Parse error"), 3);
    assert!(tile.is_renderable());
    assert!(tile.layer_properties_updated(&layer_properties));
}

/// A circle layer as its render layer evaluates it, which records what the
/// worker codec sends to evaluate it again.
fn evaluated_circle_layer_properties() -> Immutable<dyn LayerProperties> {
    let layer = CircleLayer::new("circle", "source");
    let render_layer = mln_renderer::layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    render_layer.borrow().base().evaluated_properties.clone()
}

/// A transport whose workers are in `count` worker instances on threads.
fn serializing_transport(count: usize) -> Rc<SerializingTransport> {
    SerializingTransport::new(|replies| Box::new(ThreadWorkerInstances::new(count, &replies)))
}

// Port-only: GeoJSON.SynchronousUpdate with a `SerializingTransport`: the
// worker of a synchronous update stays on this thread, so the tile still
// completes before `set_layers` returns.
#[test]
fn synchronous_update_with_the_serializing_transport() {
    let test = GeoJSONTileTest::new();

    let mut tile_parameters = test.tile_parameters();
    tile_parameters.is_update_synchronous = true;
    tile_parameters.tile_worker_transport = serializing_transport(1);
    let tile = GeoJSONTile::new(
        OverscaledTileID::from_zxy(0, 0, 0),
        "source".to_owned(),
        &tile_parameters,
        point_data(),
        None,
    );
    let layer_properties = evaluated_circle_layer_properties();
    tile.set_layers(std::slice::from_ref(&layer_properties), tile_parameters.global_state.clone());
    assert!(tile.is_complete());
    assert!(tile.is_renderable());
    assert!(tile.layer_properties_updated(&layer_properties));
}

// Port-only: tiles whose workers are in worker instances on other threads
// get their layouts back through the codec, and a data update replaces them.
#[test]
fn update_through_the_codec() {
    let test = GeoJSONTileTest::new();

    let mut tile_parameters = test.tile_parameters();
    tile_parameters.tile_worker_transport = serializing_transport(2);
    let layer_properties = evaluated_circle_layer_properties();
    // Two tiles, so both instances get one.
    let tiles: Vec<GeoJSONTile> = (0..2)
        .map(|x| {
            let id = OverscaledTileID::from_zxy(1, x, 0);
            let tile = GeoJSONTile::new(id, "source".to_owned(), &tile_parameters, point_data(), None);
            tile.set_layers(std::slice::from_ref(&layer_properties), tile_parameters.global_state.clone());
            tile
        })
        .collect();
    while !tiles.iter().all(mln_renderer::tile::Tile::is_complete) {
        test.run_loop.run_once();
    }
    for tile in &tiles {
        assert!(tile.is_renderable());
        assert!(tile.layer_properties_updated(&layer_properties));
    }

    tiles[0].update_data(point_data(), false, false);
    assert!(!tiles[0].is_complete());
    while !tiles[0].is_complete() {
        test.run_loop.run_once();
    }
    assert!(tiles[0].is_renderable());
}
