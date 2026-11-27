//! Cameras that move faster than the network answers. Each frame of a fast
//! zoom or pan asks for tiles that the next frame no longer shows; the view
//! the camera stops at must not wait for them.
//!
//! The harness updates a vector source drawn by a fill layer and a raster
//! source, as a map's frames do, over a network that has `SLOTS` requests in
//! flight at once, as the online file source does, and answers each
//! `LATENCY` frames after it got a slot, fresh for an hour, or, while it
//! fails, with a connection error that leaves the request open for the
//! file source's retry. It counts the frames until the view the camera
//! stops at has all its tiles, or the requests each tile got.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use mln_style::Source;
use mln_style::geo::LatLng;
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::{Fill, Raster};
use mln_style::size::Size;
use mln_style::sources::{raster_source, vector_source};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tileset::PartialTileset;
use parking_lot::Mutex;

use crate::camera::transform::Transform;
use crate::camera::{CameraOptions, ConstrainMode, ViewportMode};
use crate::exec::pool::TaskGroup;
use crate::exec::queue::Queue;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::map::MapMode;
use crate::source::render_source::RenderSource;
use crate::storage::chrono::{Timestamp, now};
use crate::storage::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS;
use crate::storage::response::{Reason, ResourceError};
use crate::storage::{FileSource, Resource, Responder, Response};
use crate::tile_store::TileLodMode;
use crate::tile_store::source_tiles::SourceTiles;
use crate::tile_store::store::TileStore;
use crate::tile_store::tile_parameters::TileParameters;

/// The requests in flight at once.
const SLOTS: usize = DEFAULT_MAXIMUM_CONCURRENT_REQUESTS as usize;
/// The frames a request in flight takes: 100 ms at 60 frames a second.
const LATENCY: usize = 6;

fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// MARK: - The network

/// An open request: the tile's `source/z/x/y`, the callback until the
/// network answers, and the frame it got a slot in.
struct Pending {
    tile: String,
    responder: Responder,
    in_flight_since: Option<usize>,
}

/// The open requests, in the order they were made, the frames ended, how
/// many requests each tile got, and the requests answered with an error,
/// which wait for the file source's retry.
#[derive(Default)]
struct Requests {
    open: VecDeque<Pending>,
    frame: usize,
    made: BTreeMap<String, usize>,
    retrying: Vec<Pending>,
    failed: BTreeSet<String>,
    failing: bool,
}

/// Answers every tile of the vector source with the same vector tile, and
/// every tile of the raster source with the same image.
struct Network {
    requests: Mutex<Requests>,
    vector_tile: Arc<Vec<u8>>,
    raster_tile: Arc<Vec<u8>>,
}

impl Network {
    fn new() -> Self {
        Network {
            requests: Mutex::default(),
            vector_tile: Arc::new(read_file("test/fixtures/map/issue12432/0-0-0.mvt")),
            raster_tile: Arc::new(read_file("test/fixtures/annotations/emerald.png")),
        }
    }

    /// Ends a frame: the requests that have been open the longest take the
    /// free slots, and those in flight for `LATENCY` frames are answered.
    fn end_frame(&self) {
        // Answered with the lock released: an answer's callback may request
        // again.
        let answered: VecDeque<Pending> = {
            let mut requests = self.requests.lock();
            let frame = requests.frame;
            requests.frame += 1;
            requests.open.retain(|pending| !pending.responder.is_cancelled());
            for pending in requests.open.iter_mut().take(SLOTS) {
                pending.in_flight_since.get_or_insert(frame);
            }
            let (answered, open) = std::mem::take(&mut requests.open)
                .into_iter()
                .partition(|pending| pending.in_flight_since.is_some_and(|since| frame + 1 - since >= LATENCY));
            requests.open = open;
            answered
        };
        let failing = self.requests.lock().failing;
        for pending in answered {
            // An earlier answer in the batch may have cancelled it.
            if pending.responder.is_cancelled() {
                continue;
            }
            if failing {
                let mut response = Response::new();
                response.error = Some(Box::new(ResourceError::new(Reason::Connection, "NetworkError")));
                pending.responder.send(response);
                let mut requests = self.requests.lock();
                requests.failed.insert(pending.tile.clone());
                requests.retrying.push(pending);
            } else {
                self.answer(&pending);
            }
        }
    }

    /// Answers `pending` with its tile, fresh for an hour.
    fn answer(&self, pending: &Pending) {
        let mut response = Response::new();
        let data = if pending.tile.starts_with("vector") { &self.vector_tile } else { &self.raster_tile };
        response.data = Some(Arc::clone(data));
        response.caching.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 3600));
        pending.responder.send(response);
    }

    /// Answers the requests in flight with a connection error from now on,
    /// or with their tiles again.
    fn set_failing(&self, failing: bool) {
        self.requests.lock().failing = failing;
    }

    /// The file source's retry: answers each request that an error left
    /// open, and that the tile still holds, with its tile.
    fn retry_failed(&self) {
        let retrying = std::mem::take(&mut self.requests.lock().retrying);
        for pending in retrying.iter().filter(|pending| !pending.responder.is_cancelled()) {
            self.answer(pending);
        }
    }

    /// The tiles whose requests the network answered with an error.
    fn failed(&self) -> BTreeSet<String> {
        self.requests.lock().failed.clone()
    }

    /// The tiles of the requests an error left open that their tiles still
    /// hold.
    fn retrying(&self) -> BTreeSet<String> {
        let requests = self.requests.lock();
        requests.retrying.iter().filter(|p| !p.responder.is_cancelled()).map(|p| p.tile.clone()).collect()
    }

    /// The tiles of the open requests, oldest first.
    fn open_requests(&self) -> Vec<String> {
        let requests = self.requests.lock();
        requests.open.iter().filter(|pending| !pending.responder.is_cancelled()).map(|p| p.tile.clone()).collect()
    }

    /// The tiles requested more than once, and how many times.
    fn requested_again(&self) -> BTreeMap<String, usize> {
        let requests = self.requests.lock();
        requests.made.iter().filter(|(_, count)| **count > 1).map(|(tile, count)| (tile.clone(), *count)).collect()
    }
}

impl FileSource for Network {
    fn load(&self, resource: &Resource, responder: Responder) {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let source = resource.url.split('/').next().unwrap_or_default();
        let tile = format!("{source}/{}/{}/{}", data.z, data.x, data.y);
        // A tile that is still fresh, which the online file source holds
        // until it expires: never answered.
        if resource.prior_caching.expires.is_some_and(|expires| expires > now()) {
            return;
        }
        let mut requests = self.requests.lock();
        *requests.made.entry(tile.clone()).or_default() += 1;
        requests.open.push_back(Pending { tile, responder, in_flight_since: None });
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

// MARK: - The view

/// A vector source of 512-pixel tiles up to zoom 14 under a fill layer, and
/// a raster source of 256-pixel tiles up to zoom 6, as the Liberty style
/// draws its land and water at low zoom levels.
struct Scene {
    run_loop: Queue<()>,
    thread_pool: TaskGroup,
    network: Arc<Network>,
    transform: Transform,
    vector: Source,
    raster: Source,
    fill_layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    raster_layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    vector_render_source: Box<dyn RenderSource>,
    raster_render_source: Box<dyn RenderSource>,
    store: TileStore,
    laid_out: bool,
}

impl Scene {
    fn new(size: Size) -> Self {
        let run_loop = Queue::new();
        let thread_pool = TaskGroup::new(&crate::tests::pool());
        let network = Arc::new(Network::new());

        let tileset = |url: &str, max_zoom| PartialTileset {
            tiles: Some(vec![url.to_owned()]),
            max_zoom: Some(max_zoom),
            ..Default::default()
        };
        let vector = crate::tests::ready(vector_source("vector", None, tileset("vector/{z}/{x}/{y}", 14)));
        let raster = crate::tests::ready(raster_source("raster", None, tileset("raster/{z}/{x}/{y}", 6), 256));

        let mut fill = Layer::new(crate::tests::registry().style(), "fill", "water", "vector").unwrap();
        fill.set_source_layer("water");
        let fill_layers: Vec<Arc<dyn ErasedEvaluatedLayer>> =
            vec![Arc::new(EvaluatedLayer::<Fill>::builtin(LayerStyle::recover(fill.style()).unwrap()))];
        let raster_layer = Layer::new(crate::tests::registry().style(), "raster", "raster", "raster").unwrap();
        let raster_layers: Vec<Arc<dyn ErasedEvaluatedLayer>> =
            vec![Arc::new(EvaluatedLayer::<Raster>::builtin(LayerStyle::recover(raster_layer.style()).unwrap()))];

        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(size).unwrap();

        let mut store = crate::tests::pool_store(&thread_pool);
        Scene {
            vector_render_source: crate::tests::create_render_source(&vector, &mut store),
            raster_render_source: crate::tests::create_render_source(&raster, &mut store),
            store,
            run_loop,
            thread_pool,
            network,
            transform,
            vector,
            raster,
            fill_layers,
            raster_layers,
            laid_out: false,
        }
    }

    fn pyramids(&self) -> [&SourceTiles; 2] {
        [&self.vector_render_source, &self.raster_render_source]
            .map(|source| source.tiles().and_then(|key| self.store.source(key)).expect("the source's tiles"))
    }

    /// A frame at `zoom` over `center`, north up.
    fn frame(&mut self, zoom: f64, center: (f64, f64)) {
        self.frame_rotated(zoom, center, 0.0);
    }

    /// A frame at `zoom` over `center`, turned by `bearing` degrees: the
    /// sources update for the camera, then the network answers what is due,
    /// and the tile workers lay out what it answered.
    fn frame_rotated(&mut self, zoom: f64, center: (f64, f64), bearing: f64) {
        self.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(center.0, center.1).unwrap()),
            zoom: Some(zoom),
            bearing: Some(bearing),
            ..Default::default()
        });
        let file_source: Arc<dyn FileSource> = Arc::<Network>::clone(&self.network);
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            view: self.transform.view(),
            min_zoom: self.transform.state().min_zoom(),
            max_zoom: self.transform.state().limits().max_zoom,
            file_source: Some(file_source),
            mode: MapMode::Continuous,
            // The map's default.
            prefetch_zoom_delta: 4,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            font_faces: None,
            glyph_pbfs: true,
            placement_commits: 0,
            terrain: None,
        };
        // The first frame lays the tiles out.
        let relayout = !self.laid_out;
        self.laid_out = true;
        let store = &mut self.store;
        self.vector_render_source.update(store, self.vector.clone(), &self.fill_layers, true, relayout, &parameters);
        self.raster_render_source.update(store, self.raster.clone(), &self.raster_layers, true, relayout, &parameters);

        self.network.end_frame();
        // Until neither the tile workers nor the replies they sent have
        // anything left to do.
        loop {
            self.thread_pool.wait();
            let ran = self.run_loop.run_once(&mut ());
            crate::tests::update(&mut self.store);
            let work = self.pyramids().iter().any(|source| source.slots().any(|slot| slot.tile.has_work()));
            if !ran && !work && self.store.is_idle() {
                break;
            }
        }
    }

    /// Whether every tile the view wants has loaded.
    fn is_complete(&self) -> bool {
        self.pyramids().iter().all(|pyramid| pyramid.is_loaded())
    }

    /// Frames at `zoom` over `center` until the view is complete; the frames
    /// it took.
    fn frames_until_complete(&mut self, zoom: f64, center: (f64, f64)) -> usize {
        for frames in 1..=500 {
            self.frame(zoom, center);
            if self.is_complete() {
                return frames;
            }
        }
        panic!("the view didn't complete in 500 frames; open requests: {:?}", self.network.open_requests());
    }

    /// The frames that the requests for the view's tiles take at most, in
    /// batches of `SLOTS`, with a batch to spare for the requests in flight
    /// when the camera stopped.
    fn frames_needed(&self) -> usize {
        let tiles: usize = self.pyramids().iter().map(|source| source.cover().count()).sum();
        (1 + tiles.div_ceil(SLOTS)) * LATENCY
    }

    /// The vector source's tiles at zoom 14, the ideal zoom of a view at 14,
    /// that can't draw.
    fn z14_holes(&self) -> usize {
        self.pyramids()[0].cover().filter(|tile| tile.id.overscaled_z == 14 && !tile.is_renderable()).count()
    }

    /// The world copies that the vector source draws.
    fn vector_wraps(&self) -> BTreeSet<i16> {
        self.pyramids()[0].rendered().map(|rendered| rendered.id.wrap).collect()
    }
}

// MARK: - Tests

/// A pinch out from a loaded view at zoom 5.5 over Canada to the whole world,
/// a frame at each of a few zoom levels, faster than the network answers.
/// Each world copy of the vector tiles, and of the raster tiles, shows as
/// soon as the network can answer their requests: the tiles of the zoom
/// levels that the pinch passed stop loading rather than go first.
#[test]
fn fast_zoom_out_loads_the_world_copies_first() {
    let mut scene = Scene::new(Size::new(1600, 900));
    let canada = (58.3, -110.5);
    scene.frames_until_complete(5.5, canada);

    for (zoom, center) in [(4.5, (50.0, -111.0)), (3.5, (40.0, -112.0)), (2.5, (25.0, -113.0)), (1.5, (10.0, -114.0))] {
        scene.frame(zoom, center);
    }
    let open = scene.network.open_requests();
    let frames = scene.frames_until_complete(0.0, (0.0, -114.4));

    let needed = scene.frames_needed();
    assert!(
        frames <= needed,
        "the world took {frames} frames to load, its tiles' requests {needed}; requests open after the pinch: \
         {open:?}"
    );
    assert_eq!(BTreeSet::from([-1, 0, 1]), scene.vector_wraps(), "the view shows three world copies");
}

/// A fling at zoom 6 from Paris to Beijing, a tile a frame, faster than the
/// network answers. The tiles the view passes stop loading once it has left
/// them behind, and Beijing's load first.
#[test]
fn fast_pan_loads_the_final_view_first() {
    let mut scene = Scene::new(Size::new(1600, 900));
    let (paris, beijing) = ((48.86, 2.35), (39.90, 116.40));
    scene.frames_until_complete(6.0, paris);

    let steps = 20;
    for step in 1..steps {
        let t = f64::from(step) / f64::from(steps);
        scene.frame(6.0, (paris.0 + (beijing.0 - paris.0) * t, paris.1 + (beijing.1 - paris.1) * t));
    }
    let open = scene.network.open_requests();
    let frames = scene.frames_until_complete(6.0, beijing);

    let needed = scene.frames_needed();
    assert!(
        frames <= needed,
        "Beijing took {frames} frames to load, its tiles' requests {needed}; requests open after the fling: {open:?}"
    );
}

/// From 0 at `seconds`: to 1, then between -1 and 1, each swing half a
/// second, back to 0 after each second.
fn swing(seconds: f64) -> f64 {
    4.0 * ((seconds - 0.25).rem_euclid(1.0) - 0.5).abs() - 1.0
}

/// A point of zoom 14's tiles, in tile coordinates.
fn z14_point(x: f64, y: f64) -> (f64, f64) {
    let tiles = 16384.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / tiles)).sinh().atan().to_degrees();
    (latitude, x / tiles * 360.0 - 180.0)
}

/// A camera's zoom, center and bearing.
type Camera = (f64, (f64, f64), f64);

/// Three seconds of the camera swinging back and forth along `path`, its
/// camera at a time in seconds, a frame at a time at 60 Hz,
/// from a loaded view. A tile that the view needs only for moments, too
/// short to load it, goes on loading after it leaves the view, so that each
/// swing back finds it rather than fetching it again.
fn assert_swing_fetches_each_tile_once(path: fn(f64) -> Camera) {
    let mut scene = Scene::new(Size::new(1024, 768));
    let (zoom, center, bearing) = path(0.0);
    for _ in 0..500 {
        scene.frame_rotated(zoom, center, bearing);
        if scene.is_complete() {
            break;
        }
    }
    assert!(scene.is_complete(), "the first view didn't complete");
    for frame in 1..=180 {
        let (zoom, center, bearing) = path(f64::from(frame) / 60.0);
        scene.frame_rotated(zoom, center, bearing);
    }
    assert_eq!(BTreeMap::new(), scene.network.requested_again(), "the tiles requested again during the swings");
}

/// A turn between -90° and 90° around a point 0.23 of a tile south-east of a
/// tile corner: the view reaches the column and the row of tiles two west
/// and two north of that corner only near its widest, for a sixth of each
/// swing.
#[test]
fn turning_back_and_forth_fetches_each_tile_once() {
    assert_swing_fetches_each_tile_once(|seconds| (14.0, z14_point(2620.23, 6332.23), 90.0 * swing(seconds)));
}

/// A pan 0.65 of a tile east and west of a tile's center that zooms out
/// from 14.9 to 14 at each end: the view's edges reach the next tiles only
/// in the last eighth of each swing, as a pinch back and forth does.
#[test]
fn panning_and_zooming_back_and_forth_fetches_each_tile_once() {
    assert_swing_fetches_each_tile_once(|seconds| {
        let swing = swing(seconds);
        (14.0 + 0.9 * (1.0 - swing.abs()), z14_point(2620.5 + 0.65 * swing, 6332.0), 0.0)
    });
}

/// A pan at zoom 0 from a loaded view of one world to the next world copy
/// east, then west. The frame that first shows a world copy draws it, from
/// the tiles of the world that has loaded, and the network isn't asked for
/// them again.
#[test]
fn pan_to_a_new_world_copy_draws_it_at_once() {
    let mut scene = Scene::new(Size::new(512, 512));
    scene.frames_until_complete(0.0, (0.0, 0.0));
    let raster_wraps =
        |scene: &Scene| -> BTreeSet<i16> { scene.pyramids()[1].rendered().map(|rendered| rendered.id.wrap).collect() };

    for (longitude, wraps) in [(90.0, [0, 1]), (-90.0, [-1, 0])] {
        scene.frame(0.0, (0.0, longitude));
        assert_eq!(BTreeSet::from(wraps), scene.vector_wraps(), "the vector tiles' world copies at {longitude}");
        assert_eq!(BTreeSet::from(wraps), raster_wraps(&scene), "the raster tiles' world copies at {longitude}");
        assert!(scene.is_complete(), "the copies' tiles laid out the data of the loaded ones");
    }
    assert_eq!(BTreeMap::new(), scene.network.requested_again());
}

/// A fly from far away into a view at zoom 14 whose tile requests fail, as
/// a dropped connection fails every request in flight on it.
fn fly_into_failing_view(scene: &mut Scene, view: (f64, f64)) {
    scene.frames_until_complete(6.0, (40.0, 60.0));
    scene.network.set_failing(true);
    for _ in 0..12 {
        scene.frame(14.0, view);
    }
    scene.network.set_failing(false);
    assert!(scene.z14_holes() > 0, "the failed requests left holes");
}

/// A tile whose request failed waits, in the view, for the file source to
/// ask again, and loads from that answer: its request stays open, and the
/// network gets no new one.
#[test]
fn a_failed_tile_in_view_loads_when_the_file_source_retries() {
    let mut scene = Scene::new(Size::new(1024, 768));
    let view = z14_point(8418.5, 5384.5);
    fly_into_failing_view(&mut scene, view);
    for _ in 0..30 {
        scene.frame(14.0, view);
    }
    let holes = scene.z14_holes();
    assert!(holes > 0, "nothing loads a failed tile but the file source's retry");
    assert_eq!(scene.network.failed(), scene.network.retrying(), "every failed request stays open for the retry");

    scene.network.retry_failed();
    for _ in 0..30 {
        scene.frame(14.0, view);
    }
    assert_eq!(0, scene.z14_holes(), "the retry filled the {holes} holes");
    assert_eq!(BTreeMap::new(), scene.network.requested_again(), "the retry made no new requests");
}

/// A tile whose request failed and that left the view goes, rather than
/// wait in the cache as if it were loading, whether or not it is next to
/// the view: back in the view, it is requested anew, and loads without the
/// file source's retry.
#[test]
fn a_failed_tile_that_leaves_the_view_is_requested_again_on_its_return() {
    let mut scene = Scene::new(Size::new(1024, 768));
    let view = z14_point(8418.5, 5384.5);
    fly_into_failing_view(&mut scene, view);

    // Two and a half tiles east: the column the view leaves last is next to
    // the new view, the others further.
    let east = z14_point(8421.0, 5384.5);
    for _ in 0..30 {
        scene.frame(14.0, east);
    }
    let left_behind: BTreeSet<String> =
        scene.network.failed().into_iter().filter(|tile| tile.starts_with("vector/14/")).collect();
    let retrying = scene.network.retrying();
    assert!(left_behind.is_disjoint(&retrying), "the tiles that left the view stopped their retries: {retrying:?}");

    for _ in 0..60 {
        scene.frame(14.0, view);
    }
    assert_eq!(0, scene.z14_holes(), "the failed tiles loaded again");
    let again = scene.network.requested_again();
    assert!(
        again.iter().all(|(tile, count)| *count == 2 && scene.network.failed().contains(tile)),
        "only the failed tiles were requested again, once each: {again:?}"
    );
}

/// A tile that leaves the view while it loads, and fails in the cache,
/// is requested anew when the view comes back on the next frame, rather
/// than wait for a retry that may never come.
#[test]
fn a_tile_that_fails_in_the_cache_is_requested_again_on_its_return() {
    let mut scene = Scene::new(Size::new(1024, 768));
    let view = z14_point(8418.5, 5384.5);
    scene.frames_until_complete(6.0, (40.0, 60.0));
    for _ in 0..LATENCY - 1 {
        scene.frame(14.0, view);
    }
    // The centre column, at x 8419 and loading first, leaves the view still
    // loading, next to the new one, and its answers fail as the frame ends.
    scene.network.set_failing(true);
    scene.frame(14.0, z14_point(8417.4, 5384.5));
    scene.network.set_failing(false);
    let parked: BTreeSet<String> =
        scene.network.failed().into_iter().filter(|tile| tile.starts_with("vector/14/8419/")).collect();
    assert!(!parked.is_empty(), "the column that left failed in the cache");

    scene.frame(14.0, view);
    let again = scene.network.requested_again();
    assert!(parked.iter().all(|tile| again.contains_key(tile)), "requested anew: {parked:?}, {again:?}");
}
