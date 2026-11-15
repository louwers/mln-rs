// Port-only: a tile drives its jobs (tile/job.rs): one job in flight and a
// generation, results applied only when the generation matches, a geometry
// tile's dependencies resolved from the renderer's stores between jobs, and
// jobs lost with their lane run again. Raster and raster-dem tiles' decodes
// go the same way.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use feature::{Feature, PropertyMap};
use geo_types::{Geometry, Point};
use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer_properties::{LayerProperties, LayerPropertiesBase};
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::TileData;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::hash::HashSet;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use parking_lot::Mutex;
use serde_json::json;

use crate::buckets::raster_bucket::RasterBucket;
use crate::gfx::dynamic_texture::DynamicTextureBinId;
use crate::image_store::ImageStoreObserver;
use crate::layermanager;
use crate::layout::tile_layout::LayoutParams;
use crate::tests::tile::{RasterTileTest, png};
use crate::tests::{TileDriver, style, test_source};
use crate::tile::geometry_tile::GeometryTile;
use crate::tile::job::{DataInput, Job, JobId, JobResult, JobTimes, LayoutJob, Reply, Run};
use crate::tile::raster_tile::{RasterKind, RasterTile};
use crate::tile::tile::Tile;
use crate::tile::tile_loader::{LoaderTile, VectorTileEncoding};
use crate::tile::tile_workers::{InlineWorkers, Lane, PoolWorkers, ReplyBody, TileWorkers, WorkerEvent};
use crate::tile::worker_codec::{Encoded, RemoteJobs, RemoteWorkers, ReplySink, TileWorkerHost, WorkerInstances};
use crate::tile_parameters::TileParameters;

/// Workers whose jobs run when the test says so, or get the replies the
/// test makes up.
#[derive(Clone, Default)]
struct Manual(Rc<RefCell<ManualState>>);

#[derive(Default)]
struct ManualState {
    runs: VecDeque<Run>,
    replies: Vec<Reply>,
    in_flight: usize,
    caches_data: bool,
    submitted: usize,
}

impl Manual {
    /// The oldest job submitted and not yet answered.
    fn take(&self) -> Run {
        self.0.borrow_mut().runs.pop_front().expect("a job in flight")
    }

    fn reply(&self, reply: Reply) {
        self.0.borrow_mut().replies.push(reply);
    }

    /// Runs the oldest job in flight; returns its id.
    fn run(&self) -> JobId {
        let reply = InlineWorkers::run(self.take());
        let id = reply.id;
        self.reply(reply);
        id
    }

    fn submitted(&self) -> usize {
        self.0.borrow().submitted
    }
}

impl TileWorkers for Manual {
    fn lanes(&self) -> usize {
        1
    }

    fn in_flight(&self, _lane: Lane) -> usize {
        self.0.borrow().in_flight
    }

    fn capacity(&self, _lane: Lane) -> usize {
        2
    }

    fn caches_data(&self) -> bool {
        self.0.borrow().caches_data
    }

    fn submit(&mut self, _lane: Lane, run: Run) {
        let mut state = self.0.borrow_mut();
        state.in_flight += 1;
        state.submitted += 1;
        state.runs.push_back(run);
    }

    fn receive(&mut self, events: &mut Vec<WorkerEvent>) {
        let mut state = self.0.borrow_mut();
        for Reply { id, result } in std::mem::take(&mut state.replies) {
            state.in_flight -= 1;
            events.push(WorkerEvent::Reply {
                lane: Lane(0),
                id,
                body: ReplyBody::Result(result),
                times: JobTimes::default(),
            });
        }
    }
}

/// Counts the images the store asks the app for.
#[derive(Default)]
struct Asked(RefCell<Vec<String>>);

impl ImageStoreObserver for Asked {
    fn on_style_image_missing(&self, id: &str) {
        self.0.borrow_mut().push(id.to_owned());
    }
}

fn image(id: &str) -> Arc<ImageImpl> {
    Arc::new(
        ImageImpl::new(
            id.to_owned(),
            PremultipliedImage::new(Size::new(4, 4)),
            1.0,
            false,
            vec![],
            vec![],
            None,
            None,
            None,
        )
        .expect("image"),
    )
}

/// A layer from its style JSON on source `s`, evaluated at zoom 0 as the
/// `RenderOrchestrator` evaluates it.
fn layer(mut json: serde_json::Value) -> Arc<dyn LayerProperties> {
    json["source"] = json!("s");
    let mut error = ConversionError::default();
    let layer = convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message));
    let render_layer = layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.borrow().base().evaluated_properties)
}

/// Icons of `icon-image`, at size `size`.
fn icons(icon_image: &serde_json::Value, size: f32) -> Arc<dyn LayerProperties> {
    layer(json!({"id": "icons", "type": "symbol", "layout": {"icon-image": icon_image, "icon-size": size}}))
}

fn points() -> TileData {
    let point = Feature {
        geometry: Some(Geometry::Point(Point::new(2048i16, 2048))),
        properties: Arc::new(PropertyMap::new()),
        id: feature::Identifier::Null,
    };
    TileData::Features(Arc::new(vec![point]))
}

struct Test {
    run_loop: RunLoop,
    thread_pool: TaggedScheduler,
    driver: TileDriver,
    asked: Rc<Asked>,
    tile: Option<GeometryTile>,
}

impl Test {
    /// A tile whose jobs run on `workers`, with a loaded sprite of `images`.
    fn new(workers: Box<dyn TileWorkers>, images: &[&str]) -> Self {
        Self::on(|_| workers, images)
    }

    /// A tile whose jobs run on the workers `workers` makes on the test's
    /// loop.
    fn on(workers: impl FnOnce(&RunLoop) -> Box<dyn TileWorkers>, images: &[&str]) -> Self {
        let run_loop = RunLoop::new();
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        let mut driver = TileDriver::new(&run_loop, workers(&run_loop));
        let asked = Rc::new(Asked::default());
        driver.images.set_observer(Some(Rc::<Asked>::clone(&asked)));
        for id in images {
            driver.images.add_image(image(id));
        }
        driver.images.set_loaded(true);
        let transform_state = TransformState::default();
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: &transform_state,
            file_source: None,
            scheduler: run_loop.scheduler(),
            mode: MapMode::Static,
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
            source: test_source(),
            jobs: None,
            available_images: Arc::clone(driver.images.available_images()),
            font_faces: None,
            thread_pool: thread_pool.clone(),
        };
        let id = OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0));
        let tile = GeometryTile::new(id, "s".to_owned(), &parameters, None);
        Test { run_loop, thread_pool, driver, asked, tile: Some(tile) }
    }

    fn tile(&self) -> &GeometryTile {
        self.tile.as_ref().expect("the tile")
    }

    /// The renderer's update.
    fn update(&mut self) -> bool {
        let tile = self.tile.as_ref().expect("the tile");
        self.driver.update(&[tile])
    }

    /// A tile with `layers`, whose data arrived.
    fn load(&self, layers: &[Arc<dyn LayerProperties>]) {
        self.tile().set_style(&style(layers));
        self.tile().set_data(Some(points()));
    }

    fn shows_layout(&self) -> bool {
        self.tile().create_render_data().get_feature_index().is_some()
    }

    /// The id of the bin the atlas packs `id` into now.
    fn bin_of(&mut self, id: &str) -> DynamicTextureBinId {
        let icons = [(id.to_owned(), self.driver.images.get_image(id).cloned().expect("image"))].into_iter().collect();
        let page = self.driver.atlas.pack_images(&icons, &mln_style_types::image::ImageMap::default());
        page.lease.texture_handles()[0].get_id()
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        self.run_loop.run_once();
        self.thread_pool.wait_for_empty();
    }
}

/// A cold tile with icons takes two jobs: the first finds what it needs,
/// the second lays out with it. A relayout carries what the layout used,
/// so a style edit that changes no icons takes one.
#[test]
fn a_cold_labelled_tile_takes_two_jobs_and_a_relayout_one() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &["marker"]);
    test.tile().set_style(&style(&[icons(&json!("marker"), 1.0)]));
    test.tile().set_data(Some(points()));
    test.update();
    let first = manual.run();
    test.update();
    assert!(!test.shows_layout());
    let second = manual.run();
    assert_eq!(first, second, "the resume keeps the generation");
    test.update();
    assert!(test.shows_layout() && test.tile().is_complete());
    assert_eq!(manual.submitted(), 2);

    test.tile().set_style(&style(&[icons(&json!("marker"), 2.0)]));
    test.update();
    {
        let state = manual.0.borrow();
        let Job::Layout(job) = &state.runs[0].job else {
            panic!("not a layout job");
        };
        assert!(job.resolved.icons.contains_key("marker"), "the relayout carries the icon's position");
    }
    manual.run();
    test.update();
    assert!(test.tile().is_complete());
    assert_eq!(manual.submitted(), 3);
}

/// A reply to a job whose tile changed since is stale: the tile keeps
/// showing what it showed, and lays its newest generation out.
#[test]
fn a_stale_reply_after_a_relayout_is_dropped() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    let line = layer(json!({"id": "line", "type": "circle"}));
    test.tile().set_style(&style(std::slice::from_ref(&line)));
    test.tile().set_data(Some(points()));
    test.update();
    let stale = manual.take();

    test.tile().set_style(&style(&[line]));
    let reply = InlineWorkers::run(stale);
    assert!(matches!(reply.result, JobResult::Laid(_)));
    manual.reply(reply);
    test.update();
    assert!(!test.shows_layout(), "a stale layout isn't installed");
    assert_eq!(manual.submitted(), 2, "the newest generation went once the stale job came back");
    manual.run();
    test.update();
    assert!(test.shows_layout() && test.tile().is_complete());
}

/// A job whose data the worker evicted answers `NeedsData`: the tile sends
/// the same job again, with its data.
#[test]
fn a_tile_sends_its_data_again_after_an_eviction() {
    let manual = Manual::default();
    manual.0.borrow_mut().caches_data = true;
    let mut test = Test::new(Box::new(manual.clone()), &["marker"]);
    test.tile().set_style(&style(&[icons(&json!("marker"), 1.0)]));
    test.tile().set_data(Some(points()));
    test.update();
    let first = manual.take();
    assert!(matches!(&first.job, Job::Layout(job) if matches!(job.data, DataInput::Full { .. })));
    manual.reply(InlineWorkers::run(first));
    test.update();
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(matches!(job.data, DataInput::Cached(_)), "the lane that had the data gets its key");
    manual.reply(Reply { id: resume.id, result: JobResult::NeedsData });
    test.update();
    let again = manual.take();
    assert_eq!(again.id, resume.id);
    let Job::Layout(job) = &again.job else {
        panic!("not a layout job");
    };
    assert!(matches!(job.data, DataInput::Full { .. }));
    assert!(job.resolved.icons.contains_key("marker"), "with the same answer");
    manual.reply(InlineWorkers::run(again));
    test.update();
    assert!(test.shows_layout());
}

/// An image added while a tile's first job runs may change what that job
/// discovers (`coalesce(image(a), image(b))`): its reply is stale, and the
/// tile lays out again with the images as they are.
#[test]
fn an_image_added_while_a_first_job_runs_lays_out_again() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.tile().set_style(&style(&[icons(&json!(["coalesce", ["image", "late"], ["image", "other"]]), 1.0)]));
    test.tile().set_data(Some(points()));
    test.update();
    let first = manual.take();

    test.driver.images.add_image(image("late"));
    let added = HashSet::from_iter(["late".to_owned()]);
    let available = Arc::clone(test.driver.images.available_images());
    test.tile().inner().images_changed(&added, &available);
    manual.reply(InlineWorkers::run(first));
    test.update();
    let again = manual.take();
    let Job::Layout(job) = &again.job else {
        panic!("not a layout job");
    };
    assert!(job.available_images.contains("late"));
    manual.reply(InlineWorkers::run(again));
    test.update();
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.icons.contains_key("late"));
}

/// An answer's bins stay leased while its job runs and while its layout
/// shows: a bin a worker was told about can't go to another image before.
#[test]
fn an_answer_holds_its_bins_while_its_layout_runs_and_shows() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &["a"]);
    test.tile().set_style(&style(&[icons(&json!("a"), 1.0)]));
    test.tile().set_data(Some(points()));
    test.update();
    manual.run();
    test.update();
    let bin = test.bin_of("a");
    test.driver.atlas.free_released();
    assert_eq!(test.bin_of("a"), bin, "the running job holds the bin");

    manual.run();
    test.update();
    assert!(test.tile().create_render_data().get_atlas_textures().is_some());
    test.driver.atlas.free_released();
    assert_eq!(test.bin_of("a"), bin, "the layout holds the bin");

    test.tile = None;
    test.driver.atlas.free_released();
    assert_eq!(test.driver.atlas.page_count(), 0, "nothing holds the bin");
}

/// The stores resolve only when they have something new: a tile that waits
/// for a missing image asks the app for it once, and resumes knowing it
/// missing in the next update.
#[test]
fn a_missing_image_resolves_as_missing_in_the_next_update() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.load(&[icons(&json!("missing"), 1.0)]);
    test.update();
    manual.run();
    test.update();
    assert_eq!(*test.asked.0.borrow(), ["missing"]);
    assert_eq!(manual.submitted(), 1, "the app has an update to add it");

    test.update();
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.missing_images.contains("missing"));
    assert_eq!(test.asked.0.borrow().len(), 1);
    manual.reply(InlineWorkers::run(resume));
    test.update();
    assert!(test.shows_layout());
}

/// Removing an image the shown layout used lays the tile out again.
#[test]
fn removing_a_used_image_lays_out_again() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &["a"]);
    test.load(&[icons(&json!("a"), 1.0)]);
    for _ in 0..2 {
        test.update();
        manual.run();
    }
    test.update();
    assert!(test.tile().is_complete());

    test.driver.images.remove_image("a");
    let available = Arc::clone(test.driver.images.available_images());
    test.tile().inner().images_changed(&HashSet::from_iter(["b".to_owned()]), &available);
    assert!(test.tile().is_complete(), "an image it didn't name changes nothing");
    test.tile().inner().images_changed(&HashSet::from_iter(["a".to_owned()]), &available);
    assert!(!test.tile().is_complete());
    test.update();
    assert_eq!(manual.submitted(), 3);
}

/// A scheduler whose tasks each run on a thread of their own, which catches
/// their panics as the thread pool does.
struct Catching;

impl scheduler::Scheduler for Catching {
    fn schedule(&self, task: scheduler::Task) {
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
        });
    }

    fn wait_for_empty(&self) {}
}

/// Layer properties of no layer type's, which a layout of their layer's
/// type can't read: a bug that the layout panics at.
struct Mistyped(LayerPropertiesBase);

impl LayerProperties for Mistyped {
    fn base(&self) -> &LayerPropertiesBase {
        &self.0
    }
    fn base_mut(&mut self) -> &mut LayerPropertiesBase {
        &mut self.0
    }
    fn get_dependencies(&self) -> mln_style::expression::Dependency {
        mln_style::expression::Dependency::NONE
    }
}

/// A job that panics on the thread pool still replies, `Failed`, so it
/// doesn't hold a place in flight for good.
#[test]
fn a_panicking_native_job_fails() {
    let pool = TaggedScheduler::new(Arc::new(Catching), SimpleIdentity::new());
    let mut workers = PoolWorkers::new(pool, 1, Arc::new(|| {}));
    let circles = layer(json!({"id": "dots", "type": "circle"}));
    let mistyped: Arc<dyn LayerProperties> =
        Arc::new(Mistyped(LayerPropertiesBase::new(Arc::clone(&circles.base_impl))));
    let id = JobId {
        tile: crate::tile::job::TileKey {
            source: crate::tile::job::SourceKey(1),
            id: OverscaledTileID::from_zxy(0, 0, 0),
        },
        generation: 1,
    };
    let key = crate::tile::job::DataKey { source: id.tile.source, tile: id.tile.id.canonical, load: 1 };
    let job = LayoutJob {
        style: style(&[mistyped]),
        available_images: Arc::default(),
        data: DataInput::Full { key, data: Some(points()) },
        params: LayoutParams {
            mode: MapMode::Static,
            pixel_ratio: 1.0,
            show_collision_boxes: false,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
            capture_rendered_features: false,
        },
        resolved: Arc::default(),
    };
    workers.submit(Lane(0), Run { id, job: Job::Layout(job), superseded: Arc::default() });
    assert_eq!(workers.in_flight(Lane(0)), 1);
    let mut events = Vec::new();
    let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(10);
    while events.is_empty() {
        assert!(mln_util::time::Instant::now() < deadline, "no reply");
        workers.receive(&mut events);
        std::thread::yield_now();
    }
    let [WorkerEvent::Reply { id: replied, body: ReplyBody::Result(JobResult::Failed(error)), .. }] = events.as_slice()
    else {
        panic!("expected a failure");
    };
    assert_eq!(*replied, id);
    assert_eq!(error.to_string(), "the tile's job panicked");
    assert_eq!(workers.in_flight(Lane(0)), 0);
}

/// Worker instances on the test's thread, whose frames wait until the test
/// passes them on, and which the test can kill.
#[derive(Clone)]
struct Instances(Rc<RefCell<InstancesState>>);

struct InstancesState {
    sink: Option<ReplySink>,
    hosts: Vec<Option<TileWorkerHost>>,
    queued: Vec<VecDeque<Vec<u8>>>,
    replies: Arc<Mutex<Vec<(usize, Encoded)>>>,
    started: usize,
}

impl Instances {
    fn new(count: usize) -> Self {
        let replies = Arc::new(Mutex::new(Vec::new()));
        let hosts = (0..count).map(|instance| Some(Self::host(instance, &replies))).collect();
        Instances(Rc::new(RefCell::new(InstancesState {
            sink: None,
            hosts,
            queued: (0..count).map(|_| VecDeque::new()).collect(),
            replies,
            started: count,
        })))
    }

    fn host(instance: usize, replies: &Arc<Mutex<Vec<(usize, Encoded)>>>) -> TileWorkerHost {
        let replies = Arc::clone(replies);
        TileWorkerHost::new(move |reply| replies.lock().push((instance, reply)))
    }

    /// `RemoteWorkers` over these instances.
    fn workers(&self, run_loop: &RunLoop) -> Box<dyn TileWorkers> {
        let workers = RemoteWorkers::new(&run_loop.scheduler(), |sink| {
            self.0.borrow_mut().sink = Some(sink);
            Box::new(self.clone())
        });
        Box::new(RemoteJobs(workers))
    }

    /// Runs the frames that wait for `instance`, and sends their replies.
    fn step(&self, instance: usize) {
        let state = self.0.borrow();
        let frames: Vec<Vec<u8>> = state.queued[instance].iter().cloned().collect();
        drop(state);
        self.0.borrow_mut().queued[instance].clear();
        for frame in frames {
            self.0.borrow().hosts[instance].as_ref().expect("a live instance").handle(&frame);
        }
        let state = self.0.borrow();
        let sink = state.sink.as_ref().expect("a sink");
        for (instance, reply) in state.replies.lock().drain(..) {
            sink.send(instance, reply);
        }
    }

    /// Ends `instance` with what waits for it, as a trap ends a Web
    /// Worker, which reports it.
    fn kill(&self, instance: usize) {
        let mut state = self.0.borrow_mut();
        state.hosts[instance] = None;
        state.queued[instance].clear();
        state.sink.as_ref().expect("a sink").lost(instance);
    }

    fn queued(&self, instance: usize) -> usize {
        self.0.borrow().queued[instance].len()
    }
}

impl WorkerInstances for Instances {
    fn count(&self) -> usize {
        self.0.borrow().hosts.len()
    }

    fn post(&mut self, instance: usize, request: Vec<u8>) {
        self.0.borrow_mut().queued[instance].push_back(request);
    }

    fn replace(&mut self, instance: usize) -> bool {
        let mut state = self.0.borrow_mut();
        let host = Self::host(instance, &state.replies);
        state.hosts[instance] = Some(host);
        state.queued[instance].clear();
        state.started += 1;
        true
    }
}

impl Test {
    /// The renderer's update, after the replies and losses the loop got.
    fn update_after(&mut self) -> bool {
        while self.run_loop.run_once() {}
        self.update()
    }
}

/// A Web Worker that dies while it discovers a tile's dependencies, and
/// another while it lays the tile out with them: each time the tile's job
/// runs again, on a new worker with new tables, and the tile lays out.
#[test]
fn a_worker_lost_during_discovery_and_during_a_resume() {
    let instances = Instances::new(1);
    let mut test = Test::on(|run_loop| instances.workers(run_loop), &["marker"]);
    test.load(&[icons(&json!("marker"), 1.0)]);
    test.update();
    assert_eq!(instances.queued(0), 1);

    // During discovery.
    instances.kill(0);
    test.update_after();
    assert_eq!(instances.0.borrow().started, 2);
    assert_eq!(instances.queued(0), 1, "the first try runs again, on the new worker");
    instances.step(0);
    test.update_after();
    assert_eq!(instances.queued(0), 1, "the resume");

    // During the resume.
    instances.kill(0);
    test.update_after();
    assert_eq!(instances.queued(0), 1, "the first try runs again");
    assert!(!test.shows_layout());
    for _ in 0..2 {
        instances.step(0);
        test.update_after();
    }
    assert!(test.shows_layout() && test.tile().is_complete());
}

/// A reply whose header the map side can't read loses its worker too.
#[test]
fn an_unreadable_reply_loses_its_worker() {
    let instances = Instances::new(2);
    let mut test = Test::on(|run_loop| instances.workers(run_loop), &[]);
    test.load(&[layer(json!({"id": "dots", "type": "circle"}))]);
    test.update();
    let lane = (0..2).find(|lane| instances.queued(*lane) == 1).expect("a lane with the job");
    instances.0.borrow().sink.as_ref().unwrap().send(lane, Encoded::from(vec![0, 1, 2]));
    test.update_after();
    assert_eq!(instances.0.borrow().started, 3);
    let queued: usize = (0..2).map(|lane| instances.queued(lane)).sum();
    assert_eq!(queued, 1, "the job runs again");
    for lane in 0..2 {
        instances.step(lane);
    }
    test.update_after();
    assert!(test.shows_layout());
}

/// An image added while a tile's first try runs, which the try names but
/// didn't branch on, resolves as it is when the try replies: the tile
/// resumes without discovering again.
#[test]
fn an_image_a_first_try_only_names_resolves_without_a_relayout() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.load(&[icons(&json!("late"), 1.0)]);
    test.update();
    let first = manual.take();

    test.driver.images.add_image(image("late"));
    let available = Arc::clone(test.driver.images.available_images());
    test.tile().inner().images_changed(&HashSet::from_iter(["late".to_owned()]), &available);
    manual.reply(InlineWorkers::run(first));
    test.update();
    assert_eq!(manual.submitted(), 2);
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.icons.contains_key("late"));
}

/// A tile with the renderer's jobs at hand submits as soon as its data
/// arrives, without waiting for an update.
#[test]
fn a_tile_submits_as_its_data_arrives() {
    let manual = Manual::default();
    let jobs = Rc::new(RefCell::new(crate::tile::tile_workers::TileJobs::new(Box::new(manual.clone()))));
    let run_loop = RunLoop::new();
    let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
    let transform_state = TransformState::default();
    let parameters = TileParameters {
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        transform_state: &transform_state,
        file_source: None,
        scheduler: run_loop.scheduler(),
        mode: MapMode::Static,
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
        source: test_source(),
        jobs: Some(Rc::clone(&jobs)),
        available_images: Arc::default(),
        font_faces: None,
        thread_pool: thread_pool.clone(),
    };
    let tile = GeometryTile::new(OverscaledTileID::from_zxy(0, 0, 0), "s".to_owned(), &parameters, None);
    tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    assert_eq!(manual.submitted(), 0, "no data yet");
    tile.set_data(Some(points()));
    assert_eq!(manual.submitted(), 1);

    // A newer generation supersedes the job in flight, which a worker that
    // hasn't started it may skip.
    let first = manual.take();
    tile.set_data(Some(points()));
    assert!(first.superseded.load(std::sync::atomic::Ordering::Relaxed));
    drop(tile);
    thread_pool.wait_for_empty();
}

/// A raster tile of `kind` at `x`/0 of zoom 1, whose jobs `driver` runs.
fn raster_tile(test: &RasterTileTest, x: u32, kind: RasterKind) -> RasterTile {
    test.tile(OverscaledTileID::from_zxy(1, x, 0), kind, None)
}

/// The size of the image a raster tile shows.
fn raster_size(tile: &RasterTile) -> Option<Size> {
    let layer = mln_style::layer::Layer::base_impl(&mln_style::layers::RasterLayer::new("r", "s"));
    let render_data = tile.create_render_data();
    let bucket = render_data.get_bucket(&*layer)?;
    bucket.downcast_ref::<RasterBucket>()?.image.as_ref().map(|image| image.size)
}

/// Decodes come back in any order: each tile takes only the reply of its
/// current generation, and one whose data changed while it decoded decodes
/// its newest data next and never shows the stale image.
#[test]
fn raster_decodes_apply_in_any_order() {
    let test = RasterTileTest::new();
    let manual = Manual::default();
    let mut driver = TileDriver::new(&test.run_loop, Box::new(manual.clone()));
    let tiles = [raster_tile(&test, 0, RasterKind::Image), raster_tile(&test, 1, RasterKind::Image)];
    let all: Vec<&dyn Tile> = tiles.iter().map(|tile| -> &dyn Tile { tile }).collect();
    for (tile, size) in tiles.iter().zip([16, 32]) {
        LoaderTile::set_data(&**tile.inner(), Some(png(size, size)), VectorTileEncoding::Mvt);
    }
    driver.update(&all);
    let (first, second) = (manual.take(), manual.take());
    manual.reply(InlineWorkers::run(second));
    manual.reply(InlineWorkers::run(first));
    driver.update(&all);
    assert_eq!(raster_size(&tiles[0]), Some(Size::new(16, 16)));
    assert_eq!(raster_size(&tiles[1]), Some(Size::new(32, 32)));

    // New data while the decode of the last runs: the last's reply is stale.
    LoaderTile::set_data(&**tiles[0].inner(), Some(png(8, 8)), VectorTileEncoding::Mvt);
    driver.update(&all);
    let stale = manual.take();
    LoaderTile::set_data(&**tiles[0].inner(), Some(png(4, 4)), VectorTileEncoding::Mvt);
    assert!(stale.superseded.load(std::sync::atomic::Ordering::Relaxed));
    driver.update(&all);
    assert_eq!(manual.submitted(), 3, "one decode in flight per tile");
    manual.reply(InlineWorkers::run(stale));
    driver.update(&all);
    assert_eq!(raster_size(&tiles[0]), Some(Size::new(16, 16)), "the stale image isn't shown");
    assert!(!tiles[0].is_complete());
    manual.run();
    driver.update(&all);
    assert_eq!(raster_size(&tiles[0]), Some(Size::new(4, 4)));
    assert!(tiles[0].is_complete());
}

/// A Web Worker that dies while it decodes a raster-dem tile: the tile kept
/// its data, which it sends again to the worker in its place.
#[test]
fn a_worker_lost_during_a_decode() {
    let test = RasterTileTest::new();
    let instances = Instances::new(1);
    let mut driver = TileDriver::new(&test.run_loop, instances.workers(&test.run_loop));
    let tile = raster_tile(&test, 0, RasterKind::Dem(mln_util::tileset::RasterEncoding::Mapbox));
    let update = |driver: &mut TileDriver| {
        while test.run_loop.run_once() {}
        driver.update(&[&tile]);
    };
    LoaderTile::set_data(&**tile.inner(), Some(png(16, 16)), VectorTileEncoding::Mvt);
    update(&mut driver);
    assert_eq!(instances.queued(0), 1);

    instances.kill(0);
    update(&mut driver);
    assert_eq!(instances.0.borrow().started, 2);
    assert_eq!(instances.queued(0), 1, "the decode runs again, on the new worker");
    assert!(!tile.is_complete());
    instances.step(0);
    update(&mut driver);
    assert!(tile.is_complete() && tile.is_renderable());
    assert_eq!(tile.get_bucket().expect("a DEM").get_dem_data().dim, 16);
}
