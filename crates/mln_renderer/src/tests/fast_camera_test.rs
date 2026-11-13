//! Cameras that move faster than the network answers. Each frame of a fast
//! zoom or pan asks for tiles that the next frame no longer shows; the view
//! the camera stops at must not wait for them.
//!
//! The harness updates a vector source drawn by a fill layer and a raster
//! source, as a map's frames do, over a network that has `SLOTS` requests in
//! flight at once, as the online file source does, and answers each
//! `LATENCY` frames after it got a slot. It counts the frames until the view
//! the camera stops at has all its tiles.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{LocalObject, RunLoop, TaggedScheduler, scheduler};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_storage::{AsyncRequest, Callback, FileSource, Resource, ResourceOptions, Response};
use mln_style::layer::{Layer, downcast_arc};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::fill_layer::Impl as FillLayerImpl;
use mln_style::layers::raster_layer::Impl as RasterLayerImpl;
use mln_style::layers::{FillLayer, FillLayerProperties, RasterLayer, RasterLayerProperties};
use mln_style::source::{self, Source};
use mln_style::sources::tile_source::{RasterSource, UrlOrTileset, VectorSource};
use mln_style_types::SourceType;
use mln_util::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS;
use mln_util::geo::LatLng;
use mln_util::identity::SimpleIdentity;
use mln_util::range::Range;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::{Tileset, VectorEncoding};

use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image_manager::ImageManager;
use crate::render_source::RenderSource;
use crate::sources::render_raster_source::RenderRasterSource;
use crate::sources::render_vector_source::RenderVectorSource;
use crate::text::glyph_manager::GlyphManager;
use crate::tile::tile_worker_transport::InProcessTransport;
use crate::tile_parameters::TileParameters;
use crate::tile_pyramid::TilePyramid;

/// The requests in flight at once.
const SLOTS: usize = DEFAULT_MAXIMUM_CONCURRENT_REQUESTS as usize;
/// The frames a request in flight takes: 100 ms at 60 frames a second.
const LATENCY: usize = 6;

fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// MARK: - The network

/// An open request: the tile's `source/z/x/y`, the callback until the
/// network answers, and the frame it got a slot in.
struct Pending {
    tile: String,
    callback: RefCell<Option<Callback>>,
    cancelled: Cell<bool>,
    in_flight_since: Cell<Option<usize>>,
}

struct PendingRequest(Rc<Pending>);

impl AsyncRequest for PendingRequest {}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.0.cancelled.set(true);
    }
}

/// The open requests, in the order they were made, and the frames ended.
#[derive(Default)]
struct Requests {
    open: VecDeque<Rc<Pending>>,
    frame: usize,
}

/// Answers every tile of the vector source with the same vector tile, and
/// every tile of the raster source with the same image.
struct Network {
    requests: LocalObject<RefCell<Requests>>,
    vector_tile: Arc<Vec<u8>>,
    raster_tile: Arc<Vec<u8>>,
}

impl Network {
    fn new() -> Self {
        Network {
            requests: LocalObject::new(RefCell::default()),
            vector_tile: Arc::new(read_file("test/fixtures/map/issue12432/0-0-0.mvt")),
            raster_tile: Arc::new(read_file("test/fixtures/annotations/emerald.png")),
        }
    }

    fn requests(&self) -> Rc<RefCell<Requests>> {
        self.requests.get().expect("the network is used on its thread")
    }

    /// Ends a frame: the requests that have been open the longest take the
    /// free slots, and those in flight for `LATENCY` frames are answered.
    fn end_frame(&self) {
        let answered: VecDeque<Rc<Pending>> = {
            let requests = self.requests();
            let mut requests = requests.borrow_mut();
            let frame = requests.frame;
            requests.frame += 1;
            requests.open.retain(|pending| !pending.cancelled.get());
            for pending in requests.open.iter().take(SLOTS) {
                if pending.in_flight_since.get().is_none() {
                    pending.in_flight_since.set(Some(frame));
                }
            }
            let (answered, open) = std::mem::take(&mut requests.open)
                .into_iter()
                .partition(|pending| pending.in_flight_since.get().is_some_and(|since| frame + 1 - since >= LATENCY));
            requests.open = open;
            answered
        };
        for pending in answered {
            // An earlier answer in the batch may have cancelled it.
            if pending.cancelled.get() {
                continue;
            }
            let Some(mut callback) = pending.callback.borrow_mut().take() else {
                continue;
            };
            let mut response = Response::new();
            let data = if pending.tile.starts_with("vector") { &self.vector_tile } else { &self.raster_tile };
            response.data = Some(Arc::clone(data));
            callback(response);
        }
    }

    /// The tiles of the open requests, oldest first.
    fn open_requests(&self) -> Vec<String> {
        let requests = self.requests();
        let requests = requests.borrow();
        requests.open.iter().filter(|pending| !pending.cancelled.get()).map(|p| p.tile.clone()).collect()
    }
}

impl FileSource for Network {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let data = resource.tile_data.as_ref().expect("only tiles are requested");
        let source = resource.url.split('/').next().unwrap_or_default();
        let tile = format!("{source}/{}/{}/{}", data.z, data.x, data.y);
        let pending = Rc::new(Pending {
            tile,
            callback: RefCell::new(Some(callback)),
            cancelled: Cell::new(false),
            in_flight_since: Cell::new(None),
        });
        self.requests().borrow_mut().open.push_back(Rc::clone(&pending));
        Box::new(PendingRequest(pending))
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn set_resource_options(&self, _options: ResourceOptions) {}

    fn get_resource_options(&self) -> ResourceOptions {
        ResourceOptions::default()
    }
}

// MARK: - The view

/// A vector source of 512-pixel tiles up to zoom 14 under a fill layer, and
/// a raster source of 256-pixel tiles up to zoom 6, as the Liberty style
/// draws its land and water at low zoom levels.
struct Scene {
    run_loop: RunLoop,
    thread_pool: TaggedScheduler,
    network: Arc<Network>,
    transform: Transform,
    vector: VectorSource,
    raster: RasterSource,
    fill_layers: Vec<Arc<dyn LayerProperties>>,
    raster_layers: Vec<Arc<dyn LayerProperties>>,
    vector_render_source: RenderVectorSource,
    raster_render_source: RenderRasterSource,
    transport: Rc<InProcessTransport>,
    image_manager: Rc<ImageManager>,
    glyph_manager: Rc<GlyphManager>,
    texture_atlas: Arc<DynamicTextureAtlas>,
    laid_out: bool,
}

impl Scene {
    fn new(size: Size) -> Self {
        let run_loop = RunLoop::new();
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        let network = Arc::new(Network::new());
        let file_source: Arc<dyn FileSource> = Arc::<Network>::clone(&network);

        let tileset = |url: &str, max_zoom| {
            UrlOrTileset::Tileset(Tileset {
                tiles: vec![url.to_owned()],
                zoom_range: Range::new(0, max_zoom),
                ..Default::default()
            })
        };
        let vector = VectorSource::new("vector", tileset("vector/{z}/{x}/{y}", 14), None, None, VectorEncoding::Mapbox);
        let raster = RasterSource::new("raster", tileset("raster/{z}/{x}/{y}", 6), 256, SourceType::Raster, None, None);
        vector.load_description(&*file_source);
        raster.load_description(&*file_source);

        let mut fill = FillLayer::new("water", "vector");
        fill.set_source_layer("water");
        let fill_layers: Vec<Arc<dyn LayerProperties>> =
            vec![Arc::new(FillLayerProperties::new(downcast_arc::<FillLayerImpl>(&fill.base_impl())))];
        let raster_layer = RasterLayer::new("raster", "raster");
        let raster_layers: Vec<Arc<dyn LayerProperties>> =
            vec![Arc::new(RasterLayerProperties::new(downcast_arc::<RasterLayerImpl>(&raster_layer.base_impl())))];

        let mut transform =
            Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(size).unwrap();

        Scene {
            vector_render_source: RenderVectorSource::new(source::downcast_arc(&vector.base_impl())),
            raster_render_source: RenderRasterSource::new(source::downcast_arc(&raster.base_impl())),
            transport: Rc::new(InProcessTransport::new(thread_pool.clone())),
            run_loop,
            thread_pool,
            network,
            transform,
            vector,
            raster,
            fill_layers,
            raster_layers,
            image_manager: ImageManager::create(),
            glyph_manager: GlyphManager::new(None),
            texture_atlas: Arc::new(DynamicTextureAtlas::new()),
            laid_out: false,
        }
    }

    fn pyramids(&self) -> [&TilePyramid; 2] {
        [&self.vector_render_source.tile_source().tile_pyramid, &self.raster_render_source.tile_source().tile_pyramid]
    }

    /// A frame at `zoom` over `center`: the sources update for the camera,
    /// then the network answers what is due, and the tile workers lay out
    /// what it answered.
    fn frame(&mut self, zoom: f64, center: (f64, f64)) {
        self.transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(center.0, center.1).unwrap()),
            zoom: Some(zoom),
            ..Default::default()
        });
        let file_source: Arc<dyn FileSource> = Arc::<Network>::clone(&self.network);
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: self.transform.get_state(),
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
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: Rc::clone(&self.image_manager),
            glyph_manager: Rc::clone(&self.glyph_manager),
            dynamic_texture_atlas: Arc::clone(&self.texture_atlas),
            tile_worker_transport: Rc::<InProcessTransport>::clone(&self.transport),
        };
        // The first frame lays the tiles out.
        let relayout = !self.laid_out;
        self.laid_out = true;
        self.vector_render_source.update(self.vector.base_impl(), &self.fill_layers, true, relayout, &parameters);
        self.raster_render_source.update(self.raster.base_impl(), &self.raster_layers, true, relayout, &parameters);

        self.network.end_frame();
        // Until neither the tile workers nor the replies they sent have
        // anything left to do.
        loop {
            self.thread_pool.wait_for_empty();
            if !self.run_loop.run_once() {
                self.thread_pool.wait_for_empty();
                if !self.run_loop.run_once() {
                    break;
                }
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
        let tiles: usize = self.pyramids().iter().map(|pyramid| pyramid.get_tiles().len()).sum();
        (1 + tiles.div_ceil(SLOTS)) * LATENCY
    }

    /// The world copies that the vector source draws.
    fn vector_wraps(&self) -> BTreeSet<i16> {
        self.pyramids()[0].get_rendered_tiles().map(|(id, _)| id.wrap).collect()
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
