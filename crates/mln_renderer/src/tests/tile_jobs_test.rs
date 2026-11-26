// Port-only: the tile store drives each tile's jobs (tile/job.rs): one job
// in flight and a generation, results applied only when the generation
// matches, a geometry tile's dependencies resolved from the store's glyphs
// and images between jobs, and jobs lost with their lane run again. Raster
// and raster-dem tiles' decodes go the same way.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use feature::{Feature, PropertyMap};
use geo_types::{Geometry, Point};
use mln_map_types::MapMode;
use mln_storage::FileSource;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::image::Impl as ImageImpl;
use mln_style::layer::convert_layer;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::hash::HashSet;
use mln_util::image::PremultipliedImage;
use mln_util::pool::TaskGroup;
use mln_util::queue::Queue;
use mln_util::size::Size;
use parking_lot::Mutex;
use serde_json::json;

use crate::Registry;
use crate::gfx::dynamic_texture::DynamicTextureBinId;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layout::tile_layout::LayoutParams;
use crate::renderer_event::RendererEvent;
use crate::tests::tile::png;
use crate::tests::{OneTile, style, tile_store};
use crate::tile::contents::Contents;
use crate::tile::inbox::{Inbox, InboxSender, TileEvent};
use crate::tile::job::{DataInput, Job, JobId, JobResult, JobTimes, LayoutJob, Reply, Run};
use crate::tile::tile::TileKind;
use crate::tile::tile_workers::{InlineWorkers, Lane, PoolWorkers, ReplyBody, TileWorkers};
use crate::tile::worker_codec::{Encoded, RemoteWorkers, ReplySink, TileWorkerHost, WorkerInstances};

/// Workers whose jobs run when the test says so, or get the replies the
/// test makes up.
#[derive(Clone, Default)]
pub(super) struct Manual(Rc<RefCell<ManualState>>);

#[derive(Default)]
struct ManualState {
    runs: VecDeque<Run>,
    inbox: Option<InboxSender>,
    caches_data: bool,
    submitted: usize,
}

impl Manual {
    /// The oldest job submitted and not yet answered.
    pub(super) fn take(&self) -> Run {
        self.0.borrow_mut().runs.pop_front().expect("a job in flight")
    }

    pub(super) fn reply(&self, Reply { id, result }: Reply) {
        let inbox = self.0.borrow().inbox.clone().expect("a job submitted");
        inbox.send(TileEvent::Reply { lane: Lane(0), id, body: ReplyBody::Result(result), times: JobTimes::default() });
    }

    /// Runs the oldest job in flight; returns its id.
    pub(super) fn run(&self) -> JobId {
        let reply = InlineWorkers::run(self.take());
        let id = reply.id;
        self.reply(reply);
        id
    }

    pub(super) fn submitted(&self) -> usize {
        self.0.borrow().submitted
    }
}

impl TileWorkers for Manual {
    fn lanes(&self) -> usize {
        1
    }

    fn capacity(&self, _lane: Lane) -> usize {
        2
    }

    fn caches_data(&self) -> bool {
        self.0.borrow().caches_data
    }

    fn submit(&mut self, _lane: Lane, run: Run, inbox: &InboxSender) {
        let mut state = self.0.borrow_mut();
        state.inbox = Some(inbox.clone());
        state.submitted += 1;
        state.runs.push_back(run);
    }
}

pub(super) fn image(id: &str) -> Arc<ImageImpl> {
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
pub(super) fn layer(mut json: serde_json::Value) -> Arc<dyn ErasedEvaluatedLayer> {
    json["source"] = json!("s");
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let mut render_layer = crate::tests::render_layer(layer.style());
    render_layer.evaluate(&PropertyEvaluationParameters::new(0.0));
    Arc::clone(&render_layer.base().evaluated_properties)
}

/// Icons of `icon-image`, at size `size`.
pub(super) fn icons(icon_image: &serde_json::Value, size: f32) -> Arc<dyn ErasedEvaluatedLayer> {
    layer(json!({"id": "icons", "type": "symbol", "layout": {"icon-image": icon_image, "icon-size": size}}))
}

pub(super) fn points() -> TileData {
    let point = Feature {
        geometry: Some(Geometry::Point(Point::new(2048i16, 2048))),
        properties: Arc::new(PropertyMap::new()),
        id: feature::Identifier::Null,
    };
    TileData::Features(Arc::new(vec![point]))
}

struct Test {
    thread_pool: TaskGroup,
    tile: OneTile,
    /// The images the store asked the app for.
    asked: Vec<String>,
    /// What the glyph and image stores reported, in order.
    resource_events: Vec<RendererEvent>,
}

impl Test {
    /// A tile whose jobs run on `workers`, with a loaded sprite of `images`.
    fn new(workers: Box<dyn TileWorkers>, images: &[&str]) -> Self {
        let thread_pool = TaskGroup::new(&crate::tests::pool());
        let mut store = tile_store(&thread_pool, workers);
        for id in images {
            store.images_mut().add_image(image(id));
        }
        store.images_mut().set_loaded(true);
        let id = OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0));
        let tile = OneTile::new(store, TileKind::Geometry, id, MapMode::Static);
        Test { thread_pool, tile, asked: Vec::new(), resource_events: Vec::new() }
    }

    /// The renderer's update.
    fn update(&mut self) -> bool {
        let updated = self.tile.update();
        for event in self.tile.store.take_resource_events() {
            if let RendererEvent::StyleImageMissing(id) = &event {
                self.asked.push(id.clone());
            }
            self.resource_events.push(event);
        }
        updated
    }

    /// A tile with `layers`, whose data arrived.
    fn load(&mut self, layers: &[Arc<dyn ErasedEvaluatedLayer>]) {
        self.tile.set_style(&style(layers));
        self.tile.set_data(Some(points()));
    }

    fn is_complete(&self) -> bool {
        self.tile.tile().is_complete()
    }

    fn shows_layout(&self) -> bool {
        self.tile.shows_layout()
    }

    /// Adds image `id` to the store, and tells the tiles.
    fn add_image(&mut self, id: &str) {
        self.tile.store.images_mut().add_image(image(id));
        self.tile.store.images_changed(&HashSet::from_iter([id.to_owned()]));
    }

    /// The id of the bin the atlas packs `id` into now.
    fn bin_of(&mut self, id: &str) -> DynamicTextureBinId {
        let image = self.tile.store.images().image(id).cloned().expect("image");
        let icons = [(id.to_owned(), image)].into_iter().collect();
        let page = self.tile.store.atlas().pack_images(&icons, &crate::image_store::ImageMap::default());
        page.lease.texture_handles()[0].id()
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

/// A cold tile with icons takes two jobs: the first finds what it needs,
/// the second lays out with it. A relayout carries what the layout used,
/// so a style edit that changes no icons takes one.
#[test]
fn a_cold_labelled_tile_takes_two_jobs_and_a_relayout_one() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &["marker"]);
    test.tile.set_style(&style(&[icons(&json!("marker"), 1.0)]));
    test.tile.set_data(Some(points()));
    test.update();
    let first = manual.run();
    test.update();
    assert!(!test.shows_layout());
    let second = manual.run();
    assert_eq!(first, second, "the resume keeps the generation");
    test.update();
    assert!(test.shows_layout() && test.is_complete());
    assert_eq!(manual.submitted(), 2);

    test.tile.set_style(&style(&[icons(&json!("marker"), 2.0)]));
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
    assert!(test.is_complete());
    assert_eq!(manual.submitted(), 3);
}

/// A reply to a job whose tile changed since is stale: the tile keeps
/// showing what it showed, and lays its newest generation out.
#[test]
fn a_stale_reply_after_a_relayout_is_dropped() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    let line = layer(json!({"id": "line", "type": "circle"}));
    test.tile.set_style(&style(std::slice::from_ref(&line)));
    test.tile.set_data(Some(points()));
    test.update();
    let stale = manual.take();

    test.tile.set_style(&style(&[line]));
    let reply = InlineWorkers::run(stale);
    assert!(matches!(reply.result, JobResult::Laid(_)));
    manual.reply(reply);
    test.update();
    assert!(!test.shows_layout(), "a stale layout isn't installed");
    assert_eq!(manual.submitted(), 2, "the newest generation went once the stale job came back");
    manual.run();
    test.update();
    assert!(test.shows_layout() && test.is_complete());
}

/// A job whose data the worker evicted answers `NeedsData`: the tile sends
/// the same job again, with its data.
#[test]
fn a_tile_sends_its_data_again_after_an_eviction() {
    let manual = Manual::default();
    manual.0.borrow_mut().caches_data = true;
    let mut test = Test::new(Box::new(manual.clone()), &["marker"]);
    test.tile.set_style(&style(&[icons(&json!("marker"), 1.0)]));
    test.tile.set_data(Some(points()));
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

/// A first try whose worker lacked the data runs again with the data, and
/// with the images as they are then: an image added since is there.
#[test]
fn a_first_try_sent_again_gets_the_current_images() {
    let manual = Manual::default();
    manual.0.borrow_mut().caches_data = true;
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.load(&[layer(json!({"id": "dots", "type": "circle"}))]);
    test.update();
    manual.run();
    test.update();
    assert!(test.shows_layout());

    // The relayout's first try names the data its lane got.
    test.tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    test.update();
    let first = manual.take();
    assert!(matches!(&first.job, Job::Layout(job) if matches!(job.data, DataInput::Cached(_))));
    test.tile.store.images_mut().add_image(image("late"));
    test.tile.store.images_changed(&HashSet::from_iter(["late".to_owned()]));
    manual.reply(Reply { id: first.id, result: JobResult::NeedsData });
    test.update();
    let again = manual.take();
    let Job::Layout(job) = &again.job else {
        panic!("not a layout job");
    };
    assert!(matches!(job.data, DataInput::Full { .. }), "with its data");
    assert!(job.available_images.contains("late"), "with the images as they are");
    manual.reply(InlineWorkers::run(again));
    test.update();
    assert!(test.is_complete());
}

/// A relayout for a changed image starts without the answer the tile shows,
/// and so does every first try until a layout shows: one superseded by a
/// style edit before it replied too.
#[test]
fn a_relayout_for_an_image_never_starts_with_the_outdated_answer() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &["a"]);
    test.load(&[icons(&json!("a"), 1.0)]);
    for _ in 0..2 {
        test.update();
        manual.run();
    }
    test.update();
    assert!(test.shows_layout());

    let resolved = |run: &Run| match &run.job {
        Job::Layout(job) => Arc::clone(&job.resolved),
        Job::Raster { .. } | Job::Dem { .. } => panic!("not a layout job"),
    };
    test.tile.store.images_mut().update_image(image("a"));
    test.tile.store.images_changed(&HashSet::from_iter(["a".to_owned()]));
    test.update();
    let first = manual.take();
    assert!(resolved(&first).icons.is_empty(), "the relayout starts without the icon's position");

    test.tile.set_style(&style(&[icons(&json!("a"), 2.0)]));
    manual.reply(InlineWorkers::run(first));
    test.update();
    let again = manual.take();
    assert!(resolved(&again).icons.is_empty(), "so does the first try after it");
}

/// An image added while a tile's first job runs may change what that job
/// discovers (`coalesce(image(a), image(b))`): its reply is stale, and the
/// tile lays out again with the images as they are.
#[test]
fn an_image_added_while_a_first_job_runs_lays_out_again() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.tile.set_style(&style(&[icons(&json!(["coalesce", ["image", "late"], ["image", "other"]]), 1.0)]));
    test.tile.set_data(Some(points()));
    test.update();
    let first = manual.take();

    test.add_image("late");
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
    test.tile.set_style(&style(&[icons(&json!("a"), 1.0)]));
    test.tile.set_data(Some(points()));
    test.update();
    manual.run();
    test.update();
    let bin = test.bin_of("a");
    test.tile.store.atlas().free_released();
    assert_eq!(test.bin_of("a"), bin, "the running job holds the bin");

    manual.run();
    test.update();
    assert!(test.shows_layout());
    test.tile.store.atlas().free_released();
    assert_eq!(test.bin_of("a"), bin, "the layout holds the bin");

    test.tile.drop_tile();
    test.tile.store.atlas().free_released();
    assert_eq!(test.tile.store.atlas().page_count(), 0, "nothing holds the bin");
}

/// A tile that waits for both glyphs and a missing image reports the glyph
/// range it requests, then the image it asks the app for: the order the
/// stores were asked in.
#[test]
fn glyph_and_image_events_keep_their_order() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    let run_loop: Queue<()> = Queue::new();
    let file_source: Arc<dyn FileSource> = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    test.tile.store.set_file_source(Some(file_source));
    test.tile.store.glyphs().set_url("test://{fontstack}/{range}.pbf");
    test.load(&[layer(json!({
        "id": "labels",
        "type": "symbol",
        "layout": {"text-field": "a", "icon-image": "missing"}
    }))]);
    test.update();
    manual.run();
    test.update();

    let kinds: Vec<&str> = test
        .resource_events
        .iter()
        .filter_map(|event| match event {
            RendererEvent::GlyphsRequested { .. } => Some("glyphs"),
            RendererEvent::StyleImageMissing(_) => Some("image"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["glyphs", "image"]);
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
    assert_eq!(test.asked, ["missing"]);
    assert_eq!(manual.submitted(), 1, "the app has an update to add it");

    test.update();
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.missing_images.contains("missing"));
    assert_eq!(test.asked.len(), 1);
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
    assert!(test.is_complete());

    test.tile.store.images_mut().remove_image("a");
    test.tile.store.images_changed(&HashSet::from_iter(["b".to_owned()]));
    assert!(test.is_complete(), "an image it didn't name changes nothing");
    test.tile.store.images_changed(&HashSet::from_iter(["a".to_owned()]));
    assert!(!test.is_complete());
    test.update();
    assert_eq!(manual.submitted(), 3);
}

/// A relayout that no longer uses a missing image drops it from the answer
/// it carries: the image, added while unused, is there when a later layout
/// uses it again.
#[test]
fn a_missing_image_a_relayout_drops_resolves_when_used_again() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.load(&[icons(&json!("a"), 1.0)]);
    for _ in 0..2 {
        test.update();
        test.update();
        manual.run();
    }
    test.update();
    assert!(test.shows_layout());

    test.load(&[layer(json!({"id": "dots", "type": "circle"}))]);
    test.update();
    manual.run();
    test.update();
    test.add_image("a");

    test.load(&[icons(&json!("a"), 1.0)]);
    test.update();
    let again = manual.take();
    let Job::Layout(job) = &again.job else {
        panic!("not a layout job");
    };
    assert!(!job.resolved.missing_images.contains("a"), "the answer it carries forgets the missing image");
}

/// Evaluated values of no layer type: a bug that panics when the layout
/// asks for their type.
struct Mistyped(Arc<dyn mln_style::layer::ErasedLayerStyle>);

impl ErasedEvaluatedLayer for Mistyped {
    fn entry(&self) -> &'static dyn crate::layer_type::ErasedLayerType {
        panic!("evaluated values of no layer type")
    }
    fn style(&self) -> &dyn mln_style::layer::ErasedLayerStyle {
        &*self.0
    }
    fn shared_style(&self) -> Arc<dyn mln_style::layer::ErasedLayerStyle> {
        Arc::clone(&self.0)
    }
    fn constants_mask(&self) -> u64 {
        0
    }
    fn evaluated_dependencies(&self) -> mln_style::expression::Dependency {
        mln_style::expression::Dependency::NONE
    }
    fn collect_evaluated_global_state_refs(&self, _refs: &mut mln_vec_map::VecSet<String>) {}
    fn write(&self, w: &mut mln_wire::Writer) {
        w.fail("no wire form".to_owned());
    }
}

/// A job that panics on the thread pool still replies, `Failed`, so it
/// doesn't hold a place in flight for good.
#[test]
fn a_panicking_native_job_fails() {
    let pool = TaskGroup::new(&crate::tests::pool());
    let mut workers = PoolWorkers::new(pool, 1);
    let inbox = Inbox::new(Arc::new(|| {}));
    let circles = layer(json!({"id": "dots", "type": "circle"}));
    let mistyped: Arc<dyn ErasedEvaluatedLayer> = Arc::new(Mistyped(circles.shared_style()));
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
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        },
        resolved: Arc::default(),
    };
    workers.submit(Lane(0), Run { id, job: Job::Layout(job), superseded: Arc::default() }, inbox.sender());
    let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(10);
    let event = loop {
        if let Some(event) = inbox.next() {
            break event;
        }
        assert!(mln_util::time::Instant::now() < deadline, "no reply");
        std::thread::yield_now();
    };
    let TileEvent::Reply { id: replied, body: ReplyBody::Result(JobResult::Failed(error)), .. } = event else {
        panic!("expected a failure");
    };
    assert_eq!(replied, id);
    assert_eq!(error.to_string(), "the tile's job panicked");
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
        TileWorkerHost::new(Registry::with_builtins(), move |reply| replies.lock().push((instance, reply)))
    }

    /// `RemoteWorkers` over these instances.
    fn workers(&self) -> Box<dyn TileWorkers> {
        let instances = self.clone();
        Box::new(RemoteWorkers::new(Registry::with_builtins(), move |sink| {
            instances.0.borrow_mut().sink = Some(sink);
            Box::new(instances)
        }))
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

/// A Web Worker that dies while it discovers a tile's dependencies, and
/// another while it lays the tile out with them: each time the tile's job
/// runs again, on a new worker with new tables, and the tile lays out.
#[test]
fn a_worker_lost_during_discovery_and_during_a_resume() {
    let instances = Instances::new(1);
    let mut test = Test::new(instances.workers(), &["marker"]);
    test.load(&[icons(&json!("marker"), 1.0)]);
    test.update();
    assert_eq!(instances.queued(0), 1);

    // During discovery.
    instances.kill(0);
    test.update();
    assert_eq!(instances.0.borrow().started, 2);
    assert_eq!(instances.queued(0), 1, "the first try runs again, on the new worker");
    instances.step(0);
    test.update();
    assert_eq!(instances.queued(0), 1, "the resume");

    // During the resume.
    instances.kill(0);
    test.update();
    assert_eq!(instances.queued(0), 1, "the first try runs again");
    assert!(!test.shows_layout());
    for _ in 0..2 {
        instances.step(0);
        test.update();
    }
    assert!(test.shows_layout() && test.is_complete());
}

/// A reply whose header the map side can't read loses its worker too.
#[test]
fn an_unreadable_reply_loses_its_worker() {
    let instances = Instances::new(2);
    let mut test = Test::new(instances.workers(), &[]);
    test.load(&[layer(json!({"id": "dots", "type": "circle"}))]);
    test.update();
    let lane = (0..2).find(|lane| instances.queued(*lane) == 1).expect("a lane with the job");
    instances.0.borrow().sink.as_ref().unwrap().send(lane, Encoded::from(vec![0, 1, 2]));
    test.update();
    assert_eq!(instances.0.borrow().started, 3);
    let queued: usize = (0..2).map(|lane| instances.queued(lane)).sum();
    assert_eq!(queued, 1, "the job runs again");
    for lane in 0..2 {
        instances.step(lane);
    }
    test.update();
    assert!(test.shows_layout());
}

/// What the instances of replaced workers send, a reply or their loss,
/// reaches only those workers, not the ones in their place.
#[test]
fn replaced_workers_reach_no_other_workers() {
    let (old, new) = (Instances::new(1), Instances::new(1));
    let mut test = Test::new(old.workers(), &[]);
    test.load(&[layer(json!({"id": "dots", "type": "circle"}))]);
    test.update();
    assert_eq!(old.queued(0), 1);

    test.tile.store.set_workers(new.workers());
    test.update();
    assert_eq!(new.queued(0), 1, "the job runs again on the new workers");
    old.step(0);
    old.kill(0);
    test.update();
    assert_eq!(new.0.borrow().started, 1, "the old instance's loss isn't the new one's");
    assert!(!test.shows_layout(), "nor is its reply");

    new.step(0);
    test.update();
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

    test.add_image("late");
    manual.reply(InlineWorkers::run(first));
    test.update();
    assert_eq!(manual.submitted(), 2);
    let resume = manual.take();
    let Job::Layout(job) = &resume.job else {
        panic!("not a layout job");
    };
    assert!(job.resolved.icons.contains_key("late"));
}

/// A tile's job starts in the update that has its data, and a newer
/// generation supersedes the job in flight, which a worker that hasn't
/// started it may skip.
#[test]
fn a_newer_generation_supersedes_the_job_in_flight() {
    let manual = Manual::default();
    let mut test = Test::new(Box::new(manual.clone()), &[]);
    test.tile.set_style(&style(&[layer(json!({"id": "dots", "type": "circle"}))]));
    test.update();
    assert_eq!(manual.submitted(), 0, "no data yet");
    test.tile.set_data(Some(points()));
    test.update();
    assert_eq!(manual.submitted(), 1);

    let first = manual.take();
    test.tile.set_data(Some(points()));
    assert!(first.superseded.load(std::sync::atomic::Ordering::Relaxed));
}

/// A raster tile of `kind`, `x`/0 of zoom 1, and a second one at `x + 1`,
/// whose jobs run on `workers`.
fn raster_tiles(pool: &TaskGroup, workers: Box<dyn TileWorkers>, kind: TileKind) -> OneTile {
    let store = tile_store(pool, workers);
    let mut tiles = OneTile::new(store, kind, OverscaledTileID::from_zxy(1, 0, 0), MapMode::Static);
    tiles.add(OverscaledTileID::from_zxy(1, 1, 0));
    tiles
}

/// The size of the image a raster tile shows.
fn raster_size(tiles: &OneTile, x: u32) -> Option<Size> {
    let Some(Contents::Raster(raster)) = tiles.tile_at(&OverscaledTileID::from_zxy(1, x, 0)).contents() else {
        return None;
    };
    Some(raster.image.size)
}

/// Decodes come back in any order: each tile takes only the reply of its
/// current generation, and one whose data changed while it decoded decodes
/// its newest data next and never shows the stale image.
#[test]
fn raster_decodes_apply_in_any_order() {
    let pool = TaskGroup::new(&crate::tests::pool());
    let manual = Manual::default();
    let mut tiles = raster_tiles(&pool, Box::new(manual.clone()), TileKind::Raster);
    let ids = [OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0)];
    for (id, size) in ids.iter().zip([16, 32]) {
        tiles.set_image(id, Some(png(size, size)));
    }
    tiles.update();
    let (first, second) = (manual.take(), manual.take());
    manual.reply(InlineWorkers::run(second));
    manual.reply(InlineWorkers::run(first));
    tiles.update();
    assert_eq!(raster_size(&tiles, 0), Some(Size::new(16, 16)));
    assert_eq!(raster_size(&tiles, 1), Some(Size::new(32, 32)));

    // New data while the decode of the last runs: the last's reply is stale.
    tiles.set_image(&ids[0], Some(png(8, 8)));
    tiles.update();
    let stale = manual.take();
    tiles.set_image(&ids[0], Some(png(4, 4)));
    assert!(stale.superseded.load(std::sync::atomic::Ordering::Relaxed));
    tiles.update();
    assert_eq!(manual.submitted(), 3, "one decode in flight per tile");
    manual.reply(InlineWorkers::run(stale));
    tiles.update();
    assert_eq!(raster_size(&tiles, 0), Some(Size::new(16, 16)), "the stale image isn't shown");
    assert!(!tiles.tile().is_complete());
    manual.run();
    tiles.update();
    assert_eq!(raster_size(&tiles, 0), Some(Size::new(4, 4)));
    assert!(tiles.tile().is_complete());
    drop(tiles);
    pool.wait();
}

/// A Web Worker that dies while it decodes a raster-dem tile: the tile kept
/// its data, which it sends again to the worker in its place.
#[test]
fn a_worker_lost_during_a_decode() {
    let pool = TaskGroup::new(&crate::tests::pool());
    let instances = Instances::new(1);
    let kind = TileKind::Dem(mln_util::tileset::RasterEncoding::Mapbox);
    let mut tiles = raster_tiles(&pool, instances.workers(), kind);
    let id = tiles.id;
    tiles.set_image(&id, Some(png(16, 16)));
    tiles.update();
    assert_eq!(instances.queued(0), 1);

    instances.kill(0);
    tiles.update();
    assert_eq!(instances.0.borrow().started, 2);
    assert_eq!(instances.queued(0), 1, "the decode runs again, on the new worker");
    assert!(!tiles.tile().is_complete());
    instances.step(0);
    tiles.update();
    assert!(tiles.tile().is_complete() && tiles.tile().is_renderable());
    let Some(Contents::Dem(dem)) = tiles.tile().contents() else {
        panic!("a DEM");
    };
    assert_eq!(dem.dem().dim, 16);
    drop(tiles);
    pool.wait();
}

/// A world copy made of a tile that hasn't uploaded copies the data of its
/// buckets. The tile's upload moves it into the context's stores once, and
/// the copy, laid out from the same data, takes what the upload moved and
/// frees its own.
#[test]
fn a_tiles_upload_frees_the_data_its_world_copy_copied() {
    use crate::bucket_geometry::bucket_vertices;
    use crate::buckets::circle_bucket::CircleBucket;
    use crate::tile::source_tiles::SourceTiles;

    let thread_pool = TaskGroup::new(&crate::tests::pool());
    let id = OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0));
    let copy_id = OverscaledTileID::new(0, 1, CanonicalTileID::new(0, 0, 0));
    let mut tiles =
        OneTile::new(tile_store(&thread_pool, Box::new(InlineWorkers)), TileKind::Geometry, id, MapMode::Continuous);
    tiles.set_style(&style(&[layer(json!({"id": "circles", "type": "circle"}))]));
    tiles.set_data(Some(points()));
    tiles.settle(&[id]);
    tiles.add(copy_id);
    let (source, ctx) = tiles.store.source_mut(tiles.key).expect("the source");
    let copy = source.world_copy(&copy_id).expect("a copy of the tile");
    source.update(&copy_id, ctx, |tile| tile.start_as_copy(copy));
    // Whether the circles' vertices of tile `id` moved out of its bucket and
    // its data is gone, and the stored vertices they moved into.
    let circles = |source: &SourceTiles, id: &OverscaledTileID| {
        let contents = source.tile(id)?.contents()?.geometry()?;
        let mut buckets = contents.result.layer_render_data.buckets();
        let vertices = &buckets.find_map(|bucket| bucket.downcast_ref::<CircleBucket>())?.vertices;
        let stored = vertices.is_moved().then(|| bucket_vertices(&contents.bucket_geometry, vertices)).flatten();
        Some((vertices.is_moved(), vertices.data().is_empty(), stored))
    };
    assert!(matches!(circles(source, &copy_id), Some((false, false, None))), "the copy holds a copy of the data");

    crate::tests::with_context(|_, context| {
        let needs = crate::layers::upload_needs(context);
        source.upload_at(&id, None, &ctx.atlas, context, needs);
        let (tile, copy) = (circles(source, &id), circles(source, &copy_id));
        assert!(matches!(&tile, Some((true, true, Some(_)))), "the tile's data moved");
        assert!(matches!(&copy, Some((true, true, Some(_)))), "the copy freed its data");
        assert!(tile.and_then(|tile| tile.2) == copy.and_then(|copy| copy.2), "both draw the same vertices");
    });
}
