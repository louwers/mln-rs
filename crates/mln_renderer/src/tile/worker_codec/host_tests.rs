// Port-only: a geometry tile whose worker is behind a `TileWorkerHost`
// recovers from a `setLayers` or `setData` the host can't decode, or tile
// data its worker can't parse.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer_properties::LayerProperties;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::exception::ExceptionPtr;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;

use super::envelope::{self, Action, MessageKind, Request};
use super::*;
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image_manager::ImageManager;
use crate::layermanager;
use crate::text::glyph_manager::GlyphManager;
use crate::tile::geometry_tile::GeometryTile;
use crate::tile::geometry_tile_worker::SharedTileData;
use crate::tile::tile::{Tile, TileBase};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;

/// Worker instances on threads whose requests a test can corrupt on the way.
struct Corrupting {
    instances: ThreadWorkerInstances,
    /// The kind of message whose next request becomes undecodable.
    next: Rc<Cell<Option<MessageKind>>>,
}

impl WorkerInstances for Corrupting {
    fn count(&self) -> usize {
        self.instances.count()
    }

    fn post(&self, instance: usize, request: Vec<u8>) {
        let Some(corruption) = self.next.get() else {
            return self.instances.post(instance, request);
        };
        let (envelope, message) = envelope::take::<Request>(&request).unwrap();
        if !matches!(envelope.action, Action::Message { about, .. } if about.kind == corruption) {
            return self.instances.post(instance, request);
        }
        self.next.set(None);
        // The envelope, then a message that doesn't decode.
        let mut corrupted = request[..request.len() - message.len()].to_vec();
        corrupted.extend_from_slice(&[0xff; 8]);
        self.instances.post(instance, corrupted);
    }
}

/// Counts the errors its tile reports.
#[derive(Default)]
struct Errors(Cell<usize>);

impl TileObserver for Errors {
    fn on_tile_error(&self, _tile: &TileBase, _error: ExceptionPtr) {
        self.0.set(self.0.get() + 1);
    }
}

struct Test {
    run_loop: RunLoop,
    tile: GeometryTile,
    errors: Rc<Errors>,
    corrupt: Rc<Cell<Option<MessageKind>>>,
}

impl Test {
    /// A tile at 0/0/0 of source "s", with its worker in a worker instance.
    fn new(mode: MapMode) -> Self {
        let run_loop = RunLoop::new();
        let corrupt = Rc::new(Cell::new(None));
        let next = corrupt.clone();
        let transport = SerializingTransport::new(move |replies| {
            Box::new(Corrupting { instances: ThreadWorkerInstances::new(1, replies), next })
        });
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        let transform_state = TransformState::default();
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: &transform_state,
            file_source: None,
            mode,
            prefetch_zoom_delta: 0,
            thread_pool,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            global_state: None,
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: ImageManager::create(),
            glyph_manager: GlyphManager::new(None),
            dynamic_texture_atlas: Arc::new(DynamicTextureAtlas::new()),
            tile_worker_transport: transport,
        };
        let errors = Rc::new(Errors::default());
        let id = OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0));
        let tile = GeometryTile::new(id, "s".to_owned(), &parameters, Some(errors.clone()));
        Test { run_loop, tile, errors, corrupt }
    }

    /// Runs the loop until the tile is complete.
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.tile.base().is_complete() {
            assert!(Instant::now() < deadline, "the tile stays pending");
            self.run_loop.run_once();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn set_data(&self) {
        self.tile.set_data(Some(tile_data()));
        self.wait();
    }

    fn set_layers(&self, layer: &Immutable<dyn LayerProperties>) {
        self.tile.set_layers(std::slice::from_ref(layer), None);
        self.wait();
    }

    fn errors(&self) -> usize {
        self.errors.0.get()
    }

    /// Whether the tile's layout has `layer`.
    fn laid_out(&self, layer: &Immutable<dyn LayerProperties>) -> bool {
        self.tile.create_render_data().get_layer_render_data(&*layer.base_impl).is_some()
    }
}

fn tile_data() -> SharedTileData {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test/fixtures/map/issue12432/0-0-0.mvt");
    Arc::new(mln_tile::VectorMVTTileData::new(Arc::new(std::fs::read(path).unwrap())))
}

/// Tile data that isn't a vector tile.
fn garbage() -> SharedTileData {
    Arc::new(mln_tile::VectorMVTTileData::new(Arc::new(vec![0xff; 16])))
}

/// A layer of the fixture's "water" source layer, evaluated as the
/// RenderOrchestrator evaluates it.
fn layer(id: &str, type_: &str) -> Immutable<dyn LayerProperties> {
    let json = serde_json::json!({"id": id, "type": type_, "source": "s", "source-layer": "water"});
    let mut error = ConversionError::default();
    let layer = convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message));
    let render_layer = layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    render_layer.borrow().base().evaluated_properties.clone()
}

/// A tile laid out with the fixture and the layer "fill".
fn loaded(mode: MapMode) -> (Test, Immutable<dyn LayerProperties>) {
    let test = Test::new(mode);
    let fill = layer("fill", "fill");
    test.tile.set_data(Some(tile_data()));
    test.set_layers(&fill);
    assert_eq!(test.errors(), 0);
    assert!(test.tile.is_renderable());
    assert!(test.laid_out(&fill));
    (test, fill)
}

/// A `setLayers` the host can't decode is an error for the tile, which keeps
/// its layout. The worker keeps the data but none of the earlier layers, so
/// new data gets the same error rather than a layout of the earlier layers,
/// and the next `setLayers` it can decode lays the tile out again. (The
/// instance never got what the failed one defined, so that one's layers
/// can't be decoded again.)
fn failed_set_layers_in(mode: MapMode) {
    let (test, fill) = loaded(mode);
    let line = layer("line", "line");

    test.corrupt.set(Some(MessageKind::SetLayers));
    test.set_layers(&line);
    assert_eq!(test.errors(), 1);
    assert!(test.laid_out(&fill));
    assert!(!test.laid_out(&line));

    test.set_data();
    assert_eq!(test.errors(), 2);
    assert!(test.laid_out(&fill));

    let other = layer("other", "line");
    test.set_layers(&other);
    assert_eq!(test.errors(), 2);
    assert!(test.laid_out(&other));
    assert!(!test.laid_out(&fill));
}

#[test]
fn failed_set_layers() {
    for mode in [MapMode::Continuous, MapMode::Static] {
        failed_set_layers_in(mode);
    }
}

/// A `setData` the host can't decode is an error for the tile. The worker
/// keeps the layers but not the earlier data, so new layers get the same
/// error, and the next `setData` it can decode lays the tile out again.
fn failed_set_data_in(mode: MapMode) {
    let (test, fill) = loaded(mode);
    let line = layer("line", "line");

    test.corrupt.set(Some(MessageKind::SetData));
    test.set_data();
    assert_eq!(test.errors(), 1);
    assert!(test.laid_out(&fill));

    test.set_layers(&line);
    assert_eq!(test.errors(), 2);
    assert!(test.laid_out(&fill));
    assert!(!test.laid_out(&line));

    test.set_data();
    assert_eq!(test.errors(), 2);
    assert!(test.laid_out(&line));
}

#[test]
fn failed_set_data() {
    for mode in [MapMode::Continuous, MapMode::Static] {
        failed_set_data_in(mode);
    }
}

/// Tile data the worker can't parse is an error for the tile, as in C++:
/// the worker keeps it, so new layers get an error too, until new data
/// lays the tile out again.
fn unparsable_tile_data_in(mode: MapMode) {
    let (test, fill) = loaded(mode);
    let line = layer("line", "line");

    test.tile.set_data(Some(garbage()));
    test.wait();
    assert_eq!(test.errors(), 1);
    assert!(test.laid_out(&fill));

    test.set_layers(&line);
    assert_eq!(test.errors(), 2);

    test.set_data();
    assert_eq!(test.errors(), 2);
    assert!(test.laid_out(&line));
}

#[test]
fn unparsable_tile_data() {
    for mode in [MapMode::Continuous, MapMode::Static] {
        unparsable_tile_data_in(mode);
    }
}

/// After a failed `setLayers`, a `reset` from the tile (which sends both
/// again) clears what the host remembers of the failure.
#[test]
fn reset_after_a_failed_set_layers() {
    let (test, _) = loaded(MapMode::Continuous);
    let line = layer("line", "line");

    test.corrupt.set(Some(MessageKind::SetLayers));
    test.set_layers(&line);
    assert_eq!(test.errors(), 1);

    test.tile.reset();
    test.tile.set_data(Some(tile_data()));
    let other = layer("other", "line");
    test.set_layers(&other);
    assert_eq!(test.errors(), 1);
    assert!(test.laid_out(&other));
}
