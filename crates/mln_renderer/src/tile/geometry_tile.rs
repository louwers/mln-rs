// A geometry tile drives one layout job at a time (tile/job.rs). Each change
// of an input (its data, its source's style, its layout parameters, an image
// its layout used or asked for, the lane running its job) takes a new
// generation from its source's counters, and the tile applies a reply only
// if it names the current generation: a stale result is dropped, and the
// tile keeps showing its current layout while it lays out again.
//
// The renderer's update drives the tiles (`take_reply` before the sources
// update, `drive` after): a job that answers `NeedsDeps` waits for the
// update to resolve its glyphs and images through the stores, which pack
// them into one glyph page and one image page of the atlas, and the job runs
// again with their positions. The tile holds leases on those bins until no
// layout it shows or runs uses them. A tile of a source with synchronous
// updates runs its jobs on the render thread as soon as its inputs change.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_actor::TaggedScheduler;
use mln_map_types::{MapDebugOptions, MapMode};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_style::layer;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::image::{ImagePosition, ImagePositions};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::logging::{Event, Log};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::bucket::BucketPtr;
use crate::bucket_geometry::BucketGeometry;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::context::Context;
use crate::gfx::dynamic_texture_atlas::{AtlasLease, DynamicTextureAtlas};
use crate::image_store::{ImageStore, ImageWait};
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::render_layer::LayerRenderData;
use crate::render_orchestrator::drop_on_workers;
use crate::text::glyph_store::{GlyphStore, GlyphWait};
use crate::tile::job::{DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceStyle, TileKey};
use crate::tile::tile::{Kind, Tile, TileBase, TileOperation};
use crate::tile::tile_observer::TileObserver;
use crate::tile::tile_workers::{InlineWorkers, Lane, SourceCounters, TileJobs};
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::{TileAtlasTextures, TileRenderData};

/// A layout: each layer's bucket, and the feature index.
#[derive(Clone)]
pub(crate) struct LayoutResult {
    pub(crate) layer_render_data: HashMap<String, LayerRenderData>,
    pub(crate) feature_index: Arc<FeatureIndex>,
}

impl LayoutResult {
    #[must_use]
    pub(crate) fn new(layer_render_data: HashMap<String, LayerRenderData>, feature_index: Arc<FeatureIndex>) -> Self {
        LayoutResult { layer_render_data, feature_index }
    }

    pub(crate) fn get_layer_render_data(&self, layer_impl: &dyn layer::Impl) -> Option<&LayerRenderData> {
        let result = self.layer_render_data.get(&layer_impl.id)?;
        if !std::ptr::eq(result.layer_properties.base_impl.get_type_info(), layer_impl.get_type_info()) {
            // Layer data might be outdated, see issue #12432.
            return None;
        }
        Some(result)
    }
}

/// The atlas pages a layout draws its glyphs and images from. Dropping it
/// gives their bins back.
pub(crate) struct TileAtlases {
    glyphs: AtlasLease,
    images: AtlasLease,
    /// Where the layout's patterns are, which drawing them and feature state
    /// read.
    pattern_positions: ImagePositions,
}

/// What a tile's dependencies resolved to, with the leases on the atlas
/// bins their positions name. `atlases` is `None` for a layout that waited
/// for nothing.
#[derive(Clone, Default)]
struct Answer {
    resolved: Arc<Resolved>,
    atlases: Option<Rc<TileAtlases>>,
    /// Whether it lacked glyphs of a range that failed to load.
    glyphs_failed: bool,
}

/// The layout a tile shows: its job's result, the dependencies it used and
/// what they resolved to, and the render thread's copies of its bucket
/// geometry.
#[derive(Clone)]
pub(crate) struct TileLayout {
    pub(crate) result: LayoutResult,
    answer: Answer,
    deps: Rc<Dependencies>,
    textures: Rc<TileAtlasTextures>,
    bucket_geometry: Rc<BucketGeometry>,
}

impl TileLayout {
    fn new(result: LayoutResult, answer: Answer, deps: Dependencies) -> Self {
        let textures = answer.atlases.as_ref().map_or_else(TileAtlasTextures::default, |atlases| {
            TileAtlasTextures::new(Rc::clone(atlases.glyphs.page_texture()), Rc::clone(atlases.images.page_texture()))
        });
        TileLayout { result, answer, deps: Rc::new(deps), textures: Rc::new(textures), bucket_geometry: Rc::default() }
    }

    fn pattern_positions(&self) -> Option<&ImagePositions> {
        self.answer.atlases.as_ref().map(|atlases| &atlases.pattern_positions)
    }
}

/// Port-only: lets go of a tile's layout. The last reference frees the
/// buckets and the feature index on the orchestrator's workers, since a view
/// that drops many tiles would otherwise spend frames freeing them. The
/// atlas leases drop here, on the render thread.
fn release_layout(layout: Rc<TileLayout>, thread_pool: &TaggedScheduler) {
    if let Some(TileLayout { result: LayoutResult { layer_render_data, feature_index }, answer, .. }) =
        Rc::into_inner(layout)
    {
        drop_on_workers(thread_pool, (layer_render_data, feature_index));
        drop(answer);
    }
}

struct GeometryTileRenderData {
    layout: Option<Rc<TileLayout>>,
    thread_pool: TaggedScheduler,
}

impl Drop for GeometryTileRenderData {
    fn drop(&mut self) {
        if let Some(layout) = self.layout.take() {
            release_layout(layout, &self.thread_pool);
        }
    }
}

impl TileRenderData for GeometryTileRenderData {
    fn get_atlas_textures(&self) -> Option<&Rc<TileAtlasTextures>> {
        self.layout.as_ref().map(|layout| &layout.textures)
    }

    fn get_pattern(&self, pattern: &str) -> Option<ImagePosition> {
        self.layout.as_ref()?.pattern_positions()?.get(pattern).cloned()
    }

    fn get_layer_render_data(&self, layer_impl: &dyn layer::Impl) -> Option<&LayerRenderData> {
        self.layout.as_ref().and_then(|layout| layout.result.get_layer_render_data(layer_impl))
    }

    fn get_bucket(&self, layer_impl: &dyn layer::Impl) -> Option<&BucketPtr> {
        self.get_layer_render_data(layer_impl).map(|data| &data.bucket)
    }

    fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.layout.as_ref().map(|layout| Arc::clone(&layout.result.feature_index))
    }

    fn get_bucket_geometry(&self) -> Option<&BucketGeometry> {
        self.layout.as_ref().map(|layout| &*layout.bucket_geometry)
    }

    fn upload(&self, context: &Context) {
        let Some(layout) = &self.layout else {
            return;
        };

        for entry in layout.result.layer_render_data.values() {
            if entry.bucket.needs_upload() {
                entry.bucket.upload(context);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FadeState {
    Loaded,
    NeedsFirstPlacement,
    NeedsSecondPlacement,
    CanRemove,
}

/// What the tiles resolve their jobs' dependencies with.
pub(crate) struct DependencyStores<'a> {
    pub(crate) glyphs: &'a mut GlyphStore,
    pub(crate) images: &'a mut ImageStore,
    pub(crate) atlas: &'a mut DynamicTextureAtlas,
}

/// A wait for the stores to resolve a job's dependencies.
#[derive(Default)]
struct DependencyWait {
    glyphs: GlyphWait,
    images: ImageWait,
    /// The stores' revisions at the last try, which only another revision
    /// can change the outcome of.
    revisions: Option<(u64, u64)>,
}

/// Where a tile's layout stands.
enum Work {
    /// Nothing to do for this generation: the tile shows its layout, failed,
    /// or waits for the data or style it lacks.
    Current,
    /// The generation has no job yet.
    Outdated,
    /// A job is in flight, of this generation or (when stale) an earlier
    /// one.
    Running(Running),
    /// The job answered `NeedsDeps`: the tile waits for the stores.
    Waiting { lane: Lane, needs: Dependencies, wait: DependencyWait, images: Arc<VecSet<String>> },
    /// The next job is ready but waits for a lane with room: a resume with
    /// its answer, or a job sent again with its data.
    Ready(Next),
}

struct Running {
    id: JobId,
    lane: Lane,
    /// The job is a resume, whose `answer` resolves `known`.
    known: Option<Dependencies>,
    answer: Answer,
    /// The images that exist, as the job got them.
    images: Arc<VecSet<String>>,
    superseded: Arc<AtomicBool>,
}

struct Next {
    /// The lane of the job before, for a resume.
    lane: Option<Lane>,
    known: Option<Dependencies>,
    answer: Answer,
    /// The images the job before got, which a resume discovers with again;
    /// `None` for a first try, which gets the images as they are.
    images: Option<Arc<VecSet<String>>>,
}

/// The tile's data, as it arrived: `None` for a tile without data.
struct Loaded {
    key: DataKey,
    data: Option<TileData>,
}

/// A tile's side of its jobs.
struct Driver {
    generation: u64,
    work: Work,
    /// `None` until the data arrives, and after a reset.
    data: Option<Loaded>,
    /// `None` until the source's style arrives, and after a reset.
    style: Option<Arc<SourceStyle>>,
    /// The images that exist, as the tile last heard.
    available_images: Arc<VecSet<String>>,
    /// The parameters of the next job; `first_load` is set as it starts.
    params: LayoutParams,
    /// The lane that last got the data lost it: the next job carries it.
    full: bool,
    /// The shown layout's answer resolved glyphs or images that changed
    /// since: the next first try starts without it.
    answer_outdated: bool,
}

impl Driver {
    /// Takes a new generation: a job in flight becomes stale, and anything
    /// else outdated.
    fn bump(&mut self, source: &SourceCounters) {
        self.generation = source.next_generation();
        match &self.work {
            Work::Running(running) => running.superseded.store(true, Ordering::Relaxed),
            _ => self.work = Work::Outdated,
        }
    }

    /// The dependencies the tile's latest work names, apart from what it
    /// shows.
    fn pending_dependencies(&self) -> Option<&Dependencies> {
        match &self.work {
            Work::Running(Running { known, .. }) | Work::Ready(Next { known, .. }) => known.as_ref(),
            Work::Waiting { needs, .. } => Some(needs),
            Work::Current | Work::Outdated => None,
        }
    }
}

/// Whether `result`, of a job that got the images `then`, branched on one
/// that came or went since, as `now` has them: an image its discovery found
/// unavailable that is there now, or one it found available that is gone.
/// What else it names resolved, or resolves, as the stores have it.
fn outdated_by(result: &JobResult, then: &VecSet<String>, now: &VecSet<String>) -> bool {
    let deps = match result {
        JobResult::Laid(output) => &output.deps,
        JobResult::NeedsDeps(needs) => needs,
        JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) | JobResult::Failed(_) => return false,
    };
    deps.unavailable_images.iter().any(|id| now.contains(id))
        || deps.images.keys().any(|id| then.contains(id) && !now.contains(id))
}

/// The render-thread state of a `GeometryTile`.
pub(crate) struct GeometryTileInner {
    pub(crate) base: TileBase,

    source: Rc<SourceCounters>,
    file_source: Option<Arc<dyn FileSource>>,
    /// Set once the tile is cancelled or dropped: it takes no more data.
    obsolete: Cell<bool>,
    /// The tileset has no tile here (`LoaderTile::set_missing`): the tile
    /// lays out without data but isn't renderable.
    pub(crate) missing: Cell<bool>,

    driver: RefCell<Driver>,
    layout: RefCell<Option<Rc<TileLayout>>>,
    /// Where the tile submits a job as soon as its inputs change, outside
    /// the renderer's update.
    jobs: Option<Rc<RefCell<TileJobs>>>,
    /// Where the layouts' data is freed (`release_layout`).
    thread_pool: TaggedScheduler,

    mode: MapMode,
    /// The tile's source updates synchronously: its jobs run on this thread
    /// as soon as its inputs change.
    synchronous: bool,

    show_collision_boxes: Cell<bool>,
    subdivision_granularity: Cell<SubdivisionGranularitySetting>,
    pixel_ratio: Cell<f32>,

    fade_state: Cell<FadeState>,
}

impl GeometryTileInner {
    /// Whether the tile was cancelled.
    pub(crate) fn is_obsolete(&self) -> bool {
        self.obsolete.get()
    }

    fn key(&self) -> TileKey {
        TileKey { source: self.source.key(), id: self.base.id() }
    }

    pub(crate) fn set_error(&self, err: Error) {
        self.base.loaded.set(true);
        // Without data, the tile has nothing to lay out, so it is complete.
        if self.driver.borrow().data.is_none() {
            self.base.pending.set(false);
        }
        self.base.observer().on_tile_error(&self.base, err);
    }

    pub(crate) fn set_data(&self, data: Option<TileData>) {
        if self.is_obsolete() {
            return;
        }

        if !self.base.pending.get() {
            self.base.on_tile_action(TileOperation::StartParse);
        }

        // Mark the tile as pending again if it was complete before to
        // prevent signaling a complete state despite pending parse
        // operations.
        self.base.pending.set(true);

        let key = DataKey { source: self.source.key(), tile: self.base.id().canonical, load: self.source.next_load() };
        self.change(|driver| {
            driver.data = Some(Loaded { key, data });
            driver.full = false;
        });
        self.advance();
    }

    /// Changes an input of the tile's layout, which takes a new generation.
    fn change(&self, f: impl FnOnce(&mut Driver)) {
        let mut driver = self.driver.borrow_mut();
        f(&mut driver);
        driver.bump(&self.source);
    }

    /// The layout the tile shows.
    fn layout(&self) -> Option<Rc<TileLayout>> {
        self.layout.borrow().clone()
    }

    /// Drops the tile's data and style and leaves the tile pending, waiting
    /// for new ones.
    pub(crate) fn reset(&self) {
        // If there is pending work, indicate that work has been cancelled.
        // Clear the pending status.
        if self.base.pending.get() {
            self.base.on_tile_action(TileOperation::Cancelled);
            self.base.pending.set(false);
        }

        // Reset the tile to an unloaded state to avoid signaling completion
        // after clearing the tile's pending status.
        self.base.loaded.set(false);

        self.change(|driver| {
            driver.data = None;
            driver.style = None;
        });
    }

    /// Lays the tile out with `style`, for the pixel ratio and subdivision
    /// granularity set since the last style.
    fn set_style(&self, style: &Arc<SourceStyle>) {
        let mut has_data = false;
        self.change(|driver| {
            has_data = driver.data.is_some();
            driver.style = Some(Arc::clone(style));
            driver.params.pixel_ratio = self.pixel_ratio.get();
            driver.params.subdivision_granularity = self.subdivision_granularity.get();
        });
        // Mark the tile as pending again if it was complete before to prevent
        // signaling a complete state despite pending parse operations. A tile
        // whose data failed to load has nothing to lay out.
        if !self.base.pending.get() && (has_data || !self.base.loaded.get()) {
            self.base.pending.set(true);
            self.base.on_tile_action(TileOperation::StartParse);
        }
        self.advance();
    }

    /// Lays the tile out again, with its data and style, for glyphs or
    /// images that changed: without the answer they had. The tile is
    /// pending until its layout arrives.
    fn relayout(&self) {
        if !self.base.pending.get() {
            self.base.pending.set(true);
            self.base.on_tile_action(TileOperation::StartParse);
        }
        self.change(|driver| driver.answer_outdated = true);
        self.advance();
    }

    /// Takes in a change of images: the ones in `changed` were added,
    /// removed or resized, and `available` exist now. Lays the tile out
    /// again if the images it shows, or that a job resolved for it, name one
    /// of `changed`, or if a job waiting for its images branched on one that
    /// came or went (an image the stores still have to resolve resolves as
    /// it is now). A first try in flight is checked when it replies.
    pub(crate) fn images_changed(&self, changed: &HashSet<String>, available: &Arc<VecSet<String>>) {
        let relayout = {
            let mut driver = self.driver.borrow_mut();
            driver.available_images = Arc::clone(available);
            let names = |deps: &Dependencies| deps.images.keys().any(|id| changed.contains(id));
            let shown = self.layout().is_some_and(|layout| names(&layout.deps));
            let work = match &driver.work {
                Work::Running(Running { known: Some(known), .. }) | Work::Ready(Next { known: Some(known), .. }) => {
                    names(known)
                }
                Work::Waiting { needs, .. } => {
                    needs.unavailable_images.iter().any(|id| changed.contains(id) && available.contains(id))
                        || needs.images.keys().any(|id| changed.contains(id) && !available.contains(id))
                }
                // A first try's reply tells whether the change touched it.
                Work::Running(_) | Work::Current | Work::Outdated | Work::Ready(_) => false,
            };
            shown || work
        };
        if relayout {
            self.relayout();
        }
    }

    /// Lays the tile out again if its layout, or the one it runs, lacked
    /// glyphs of a range that failed to load, which asks for the range
    /// again.
    pub(crate) fn retry_failed_glyphs(&self) {
        let failed = {
            let driver = self.driver.borrow();
            let running = match &driver.work {
                Work::Running(Running { answer, .. }) | Work::Ready(Next { answer, .. }) => answer.glyphs_failed,
                Work::Current | Work::Outdated | Work::Waiting { .. } => false,
            };
            running || self.layout().is_some_and(|layout| layout.answer.glyphs_failed)
        };
        if failed {
            self.relayout();
        }
    }

    /// Calls `f` with the dependencies of the layout the tile shows and of
    /// the one it lays out.
    pub(crate) fn for_each_dependencies(&self, mut f: impl FnMut(&Dependencies)) {
        if let Some(layout) = self.layout() {
            f(&layout.deps);
        }
        if let Some(deps) = self.driver.borrow().pending_dependencies() {
            f(deps);
        }
    }

    /// Takes the reply to the tile's job, if it arrived, and gives up the
    /// job if its lane was lost. A stale reply stays with `jobs`, undecoded.
    /// Returns whether the tile shows a new layout.
    pub(crate) fn take_reply(&self, jobs: &mut TileJobs) -> bool {
        let mut driver = self.driver.borrow_mut();
        let (id, lane) = match &driver.work {
            Work::Running(running) => (running.id, running.lane),
            Work::Waiting { lane, .. } | Work::Ready(Next { lane: Some(lane), .. }) => {
                if jobs.is_lost(*lane) {
                    driver.bump(&self.source);
                }
                return false;
            }
            Work::Current | Work::Outdated | Work::Ready(_) => return false,
        };
        if !jobs.has_reply(&id) {
            if jobs.is_lost(lane) {
                // Its reply never comes, and its leases go.
                driver.bump(&self.source);
                driver.work = Work::Outdated;
            }
            return false;
        }
        if id.generation != driver.generation {
            // The tile lays out its newest generation instead.
            driver.work = Work::Outdated;
            drop(driver);
            self.base.observer().on_update_needed(&self.base);
            return false;
        }
        let Work::Running(running) = std::mem::replace(&mut driver.work, Work::Outdated) else {
            return false;
        };
        drop(driver);
        match jobs.take_reply(&id) {
            Some(result) => self.apply(running, result),
            None => false,
        }
    }

    /// Applies the result of `running`, the tile's job. Returns whether the
    /// tile shows a new layout.
    fn apply(&self, running: Running, result: JobResult) -> bool {
        let Running { id, lane, known, answer, images, .. } = running;
        let mut driver = self.driver.borrow_mut();
        let outdated_images =
            !Arc::ptr_eq(&images, &driver.available_images) && outdated_by(&result, &images, &driver.available_images);
        if id.generation != driver.generation || outdated_images {
            // Stale: the tile lays out its newest generation instead.
            if outdated_images {
                driver.answer_outdated = true;
                driver.bump(&self.source);
            }
            driver.work = Work::Outdated;
            drop(driver);
            drop_on_workers(&self.thread_pool, result);
            self.base.observer().on_update_needed(&self.base);
            return false;
        }
        match result {
            JobResult::Laid(output) => {
                driver.work = Work::Current;
                let data = driver.data.as_ref().and_then(|loaded| loaded.data.clone());
                drop(driver);
                self.install(output, answer, data);
                true
            }
            JobResult::NeedsDeps(needs) if known.is_none() => {
                driver.work = Work::Waiting { lane, needs, wait: DependencyWait::default(), images };
                drop(driver);
                self.base.observer().on_update_needed(&self.base);
                false
            }
            JobResult::NeedsDeps(_) => {
                driver.work = Work::Current;
                drop(driver);
                Log::error(
                    Event::General,
                    format!("Tile {} asked again for the dependencies it was given", id.tile.id),
                );
                self.on_error(Error::native("a layout asked again for the dependencies it was given"));
                false
            }
            JobResult::NeedsData => {
                driver.full = true;
                driver.work = Work::Ready(Next { lane: None, known, answer, images: Some(images) });
                drop(driver);
                self.base.observer().on_update_needed(&self.base);
                false
            }
            JobResult::Failed(error) => {
                driver.work = Work::Current;
                drop(driver);
                self.on_error(error);
                false
            }
            JobResult::Raster(_) | JobResult::Dem(_) => {
                driver.work = Work::Current;
                drop(driver);
                self.on_error(Error::native("a geometry tile's layout answered as a decode"));
                false
            }
        }
    }

    /// Shows a layout of the current generation, built from `data`.
    fn install(&self, output: LayoutOutput, answer: Answer, data: Option<TileData>) {
        let LayoutOutput { mut result, deps } = output;
        // The index crossed without its data, or with the data it was built
        // from, which is the tile's.
        if let Some(feature_index) = Arc::get_mut(&mut result.feature_index) {
            feature_index.set_data(data);
        }

        self.base.loaded.set(true);
        self.base.renderable.set(!self.missing.get());
        self.base.pending.set(false);
        self.base.on_tile_action(TileOperation::EndParse);

        *self.layout.borrow_mut() = Some(Rc::new(TileLayout::new(result, answer, deps)));
        self.base.feature_state_version.set(0);

        self.base.observer().on_tile_changed(&self.base);
    }

    pub(crate) fn on_error(&self, err: Error) {
        self.base.loaded.set(true);
        self.base.pending.set(false);
        self.base.on_tile_action(TileOperation::Error);
        self.base.observer().on_tile_error(&self.base, err);
    }

    /// Moves the tile's layout on: resolves the dependencies it waits for,
    /// once the stores have them all or know them missing, and submits its
    /// next job if a lane has room. Returns whether the tile shows a new
    /// layout (a tile of a source with synchronous updates).
    pub(crate) fn drive(&self, jobs: &mut TileJobs, stores: &mut DependencyStores<'_>) -> bool {
        self.resolve(stores);
        if self.synchronous {
            return self.run_inline();
        }
        self.submit(jobs);
        false
    }

    /// Submits the tile's next job to `jobs`, if it has one and a lane has
    /// room.
    fn submit(&self, jobs: &mut TileJobs) {
        if let Some((run, running)) = self.next_job(|key, full| jobs.place(key, full)) {
            jobs.submit(running.lane, run);
            self.driver.borrow_mut().work = Work::Running(running);
        }
    }

    /// Whether the tile has a job it can submit, or dependencies to resolve.
    pub(crate) fn has_work(&self) -> bool {
        let driver = self.driver.borrow();
        match driver.work {
            Work::Waiting { .. } => true,
            Work::Outdated | Work::Ready(_) => !self.is_obsolete() && driver.data.is_some() && driver.style.is_some(),
            Work::Current | Work::Running(_) => false,
        }
    }

    /// Starts the tile's next job now: on this thread for a synchronous
    /// tile, else on its workers if a lane has room, which the renderer's
    /// update otherwise waits for. A job started outside an update replies
    /// in time for the next.
    fn advance(&self) {
        if self.synchronous {
            self.run_inline();
            return;
        }
        if let Some(jobs) = &self.jobs
            && let Ok(mut jobs) = jobs.try_borrow_mut()
        {
            self.submit(&mut jobs);
        }
        // A job in flight wakes the update when it replies: an update now
        // would only miss the reply.
        if self.has_work() {
            self.base.observer().on_update_needed(&self.base);
        }
    }

    /// Lets go of the tile's job: it takes no more data, and a job not
    /// started yet needn't run.
    fn abandon(&self) {
        self.obsolete.set(true);
        self.change(|_| {});
    }

    /// Runs a synchronous tile's jobs until one waits for dependencies or
    /// the tile shows its layout. Returns whether it does.
    fn run_inline(&self) -> bool {
        let mut installed = false;
        while let Some((run, running)) = self.next_job(|_, _| Some((Lane(0), false))) {
            let reply = InlineWorkers::run(run);
            installed |= self.apply(running, reply.result);
        }
        installed
    }

    /// The tile's next job, placed by `place`, and its running state; `None`
    /// if the tile has no job to run, lacks its data or style, or `place`
    /// finds no lane.
    fn next_job(&self, place: impl FnOnce(DataKey, bool) -> Option<(Lane, bool)>) -> Option<(Run, Running)> {
        if self.is_obsolete() {
            return None;
        }
        let mut driver = self.driver.borrow_mut();
        let driver = &mut *driver;
        let (Some(loaded), Some(style)) = (&driver.data, &driver.style) else {
            return None;
        };
        let layout = self.layout();
        let next = match std::mem::replace(&mut driver.work, Work::Outdated) {
            Work::Outdated => Next {
                lane: None,
                known: None,
                images: None,
                answer: layout
                    .as_ref()
                    .filter(|_| !driver.answer_outdated)
                    .map(|layout| layout.answer.clone())
                    .unwrap_or_default(),
            },
            Work::Ready(next) => next,
            work @ (Work::Current | Work::Running(_) | Work::Waiting { .. }) => {
                driver.work = work;
                return None;
            }
        };
        let Some((lane, cached)) = place(loaded.key, driver.full || loaded.data.is_none()) else {
            // A first try needs nothing kept; a resume keeps its answer.
            if next.known.is_some() || driver.full {
                driver.work = Work::Ready(next);
            }
            return None;
        };
        driver.full = false;
        driver.answer_outdated = false;
        let data = if cached {
            DataInput::Cached(loaded.key)
        } else {
            DataInput::Full { key: loaded.key, data: loaded.data.clone() }
        };
        let id = JobId { tile: self.key(), generation: driver.generation };
        let params = LayoutParams { first_load: layout.is_none(), ..driver.params };
        let images = next.images.unwrap_or_else(|| Arc::clone(&driver.available_images));
        let job = LayoutJob {
            style: Arc::clone(style),
            available_images: Arc::clone(&images),
            data,
            params,
            resolved: Arc::clone(&next.answer.resolved),
        };
        let superseded = Arc::new(AtomicBool::new(false));
        let run = Run { id, job: Job::Layout(job), superseded: Arc::clone(&superseded) };
        Some((run, Running { id, lane, known: next.known, answer: next.answer, images, superseded }))
    }

    /// Resolves the dependencies the tile's job waits for once the stores
    /// have them all or know them missing, packing them into the atlas. Does
    /// nothing for a tile that waits for nothing, or whose stores haven't
    /// changed since it last tried.
    fn resolve(&self, stores: &mut DependencyStores<'_>) {
        let mut driver = self.driver.borrow_mut();
        let Work::Waiting { lane, needs, wait, images: seen } = &mut driver.work else {
            return;
        };
        let revisions = (stores.glyphs.revision(), stores.images.revision());
        if wait.revisions == Some(revisions) {
            return;
        }
        let glyphs = stores.glyphs.resolve(&needs.glyphs, &mut wait.glyphs, self.file_source.as_ref());
        let images = stores.images.resolve(&needs.images, &mut wait.images);
        let (Some(glyphs), Some(images)) = (glyphs, images) else {
            wait.revisions = Some((stores.glyphs.revision(), stores.images.revision()));
            return;
        };

        let glyph_page = stores.atlas.pack_glyphs(&glyphs.glyphs);
        let image_page = stores.atlas.pack_images(&images.icons, &images.patterns);
        let missing_images = needs
            .images
            .keys()
            .filter(|id| !images.icons.contains_key(*id) && !images.patterns.contains_key(*id))
            .cloned()
            .collect();
        let atlases = TileAtlases {
            glyphs: glyph_page.lease,
            images: image_page.lease,
            pattern_positions: image_page.patterns.clone(),
        };
        let resolved = Resolved {
            glyphs: glyph_page.positions,
            shaping: glyphs.shaping,
            icons: image_page.icons,
            patterns: image_page.patterns,
            missing_images,
        };
        let answer =
            Answer { resolved: Arc::new(resolved), atlases: Some(Rc::new(atlases)), glyphs_failed: glyphs.failed };
        let lane = Some(*lane);
        let known = Some(std::mem::take(needs));
        let images = Some(Arc::clone(seen));
        driver.work = Work::Ready(Next { lane, known, answer, images });
    }

    pub(crate) fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.layout().map(|layout| Arc::clone(&layout.result.feature_index))
    }
}

pub(crate) struct GeometryTile {
    inner: Rc<GeometryTileInner>,
}

impl GeometryTile {
    #[must_use]
    pub(crate) fn new(
        id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let source = Rc::clone(&parameters.source);
        let show_collision_boxes = parameters.debug_options.contains(MapDebugOptions::COLLISION);
        let driver = Driver {
            generation: source.next_generation(),
            work: Work::Outdated,
            data: None,
            style: None,
            available_images: Arc::clone(&parameters.available_images),
            params: LayoutParams {
                mode: parameters.mode,
                pixel_ratio: parameters.pixel_ratio,
                show_collision_boxes,
                subdivision_granularity: SubdivisionGranularitySetting::none(),
                first_load: true,
                capture_rendered_features: parameters.capture_rendered_features,
            },
            full: false,
            answer_outdated: false,
        };
        let inner = GeometryTileInner {
            base: TileBase::new(Kind::Geometry, id, source_id, observer),
            source,
            file_source: parameters.file_source.clone(),
            obsolete: Cell::new(false),
            missing: Cell::new(false),
            driver: RefCell::new(driver),
            layout: RefCell::new(None),
            jobs: parameters.jobs.clone(),
            thread_pool: parameters.thread_pool.clone(),
            mode: parameters.mode,
            synchronous: parameters.is_update_synchronous,
            show_collision_boxes: Cell::new(show_collision_boxes),
            subdivision_granularity: Cell::new(SubdivisionGranularitySetting::none()),
            pixel_ratio: Cell::new(parameters.pixel_ratio),
            fade_state: Cell::new(FadeState::Loaded),
        };
        GeometryTile { inner: Rc::new(inner) }
    }

    #[must_use]
    pub(crate) fn inner(&self) -> &Rc<GeometryTileInner> {
        &self.inner
    }

    #[cfg(test)]
    pub(crate) fn set_error(&self, err: Error) {
        self.inner.set_error(err);
    }

    #[cfg(test)]
    pub(crate) fn set_data(&self, data: Option<TileData>) {
        self.inner.set_data(data);
    }

    pub(crate) fn reset(&self) {
        self.inner.reset();
    }

    #[must_use]
    pub(crate) fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.inner.get_feature_index()
    }
}

impl Drop for GeometryTile {
    fn drop(&mut self) {
        self.inner.abandon();

        if self.inner.base.pending.get() {
            // This tile never finished loading or was abandoned, emit a
            // cancellation event
            self.inner.base.on_tile_action(TileOperation::Cancelled);
        }

        let layout = self.inner.layout.borrow_mut().take();
        if let Some(layout) = layout {
            release_layout(layout, &self.inner.thread_pool);
        }
    }
}

impl Tile for GeometryTile {
    fn base(&self) -> &TileBase {
        &self.inner.base
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        Box::new(GeometryTileRenderData { layout: self.inner.layout(), thread_pool: self.inner.thread_pool.clone() })
    }

    fn cancel(&self) {
        self.inner.abandon();
    }

    fn take_reply(&self, jobs: &mut TileJobs) -> bool {
        self.inner.take_reply(jobs)
    }

    fn has_work(&self) -> bool {
        self.inner.has_work()
    }

    fn drive(&self, jobs: &mut TileJobs, stores: &mut DependencyStores<'_>) -> bool {
        self.inner.drive(jobs, stores)
    }

    fn set_style(&self, style: &Arc<SourceStyle>) {
        self.inner.set_style(style);
    }

    fn set_show_collision_boxes(&self, show_collision_boxes: bool) {
        let inner = &self.inner;
        if inner.show_collision_boxes.replace(show_collision_boxes) != show_collision_boxes {
            inner.change(|driver| driver.params.show_collision_boxes = show_collision_boxes);
            inner.advance();
        }
    }

    fn set_subdivision_granularity(&self, value: &SubdivisionGranularitySetting) {
        self.inner.subdivision_granularity.set(*value);
    }

    fn set_pixel_ratio(&self, pixel_ratio: f32) -> bool {
        self.inner.pixel_ratio.replace(pixel_ratio) != pixel_ratio
    }

    #[cfg(test)]
    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.inner.base.set_observer(observer);
    }

    // layers: feature state
    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        let Some(layout) = self.inner.layout() else {
            return;
        };
        // getData(): latestFeatureIndex->getData()
        let Some(layers) = layout.result.feature_index.get_data() else {
            return;
        };
        if states.is_empty() {
            return;
        }
        let no_patterns = ImagePositions::default();
        let pattern_positions = layout.pattern_positions().unwrap_or(&no_patterns);

        for (layer_id, render_data) in &layout.result.layer_render_data {
            let source_layer_id = &render_data.layer_properties.base_impl.source_layer;
            // The worker got the layer from the same data.
            if let Ok(Some(source_layer)) = layers.layer(source_layer_id) {
                let Some(feature_states) = states.get(source_layer_id) else {
                    continue;
                };
                if feature_states.is_empty() {
                    continue;
                }
                if render_data.bucket.has_data() {
                    render_data.bucket.update(feature_states, &source_layer, layer_id, pattern_positions);
                }
            }
        }
    }

    fn layer_properties_updated(&self, layer_properties: &Arc<dyn LayerProperties>) -> bool {
        let mut layout = self.inner.layout.borrow_mut();
        let Some(layout) = layout.as_mut() else {
            return false;
        };
        let layer_impl = &layer_properties.base_impl;
        let Some(render_data) = layout.result.get_layer_render_data(&**layer_impl) else {
            return false;
        };

        if !Arc::ptr_eq(&render_data.layer_properties, layer_properties)
            && render_data.layer_properties.constants_mask() == layer_properties.constants_mask()
        {
            // C++ updates the shared LayoutResult in place; render data
            // created from it before (the previous frame's RenderTiles) see
            // the change too, but are replaced in the next prepare. The port
            // updates a copy if the result is shared.
            let layout = Rc::make_mut(layout);
            if let Some(render_data) = layout.result.layer_render_data.get_mut(&layer_impl.id) {
                render_data.layer_properties = Arc::clone(layer_properties);
            }
        }

        true
    }

    fn hold_for_fade(&self) -> bool {
        self.inner.mode == MapMode::Continuous
            && matches!(self.inner.fade_state.get(), FadeState::NeedsFirstPlacement | FadeState::NeedsSecondPlacement)
    }

    fn mark_rendered_ideal(&self) {
        self.inner.fade_state.set(FadeState::Loaded);
    }

    fn mark_rendered_previously(&self) {
        if self.inner.fade_state.get() == FadeState::Loaded {
            self.inner.fade_state.set(FadeState::NeedsFirstPlacement);
        }
    }

    fn performed_fade_placement(&self) {
        match self.inner.fade_state.get() {
            FadeState::NeedsFirstPlacement => self.inner.fade_state.set(FadeState::NeedsSecondPlacement),
            FadeState::NeedsSecondPlacement => self.inner.fade_state.set(FadeState::CanRemove),
            _ => {}
        }
    }

    // query
    fn get_query_padding(&self, layers: &QueryLayers<'_>) -> f32 {
        let layout = self.inner.layout();
        let mut query_padding: f32 = 0.0;
        for layer in layers.values() {
            let Some(data) =
                layout.as_ref().and_then(|layout| layout.result.get_layer_render_data(&*layer.base().base_impl))
            else {
                continue;
            };
            if data.bucket.has_data() {
                // std::max(queryPadding, radius)
                let radius = data.bucket.get_query_radius(*layer);
                if query_padding < radius {
                    query_padding = radius;
                }
            }
        }
        query_padding
    }

    fn query_rendered_features(
        &self,
        result: &mut QueryResults,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        transform_state: &mln_map_types::TransformState,
        layers: &QueryLayers<'_>,
        options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        proj_matrix: &Mat4,
        feature_state: &SourceFeatureState,
    ) -> Result<(), Error> {
        let layout = self.inner.layout();
        // `if (!getData()) return;`
        let Some(layout) = layout else {
            return Ok(());
        };
        if layout.result.feature_index.get_data().is_none() {
            return Ok(());
        }

        let query_padding = self.get_query_padding(layers);

        let id = self.inner.base.id();
        let projector = mln_map_types::TileProjector::with_data(
            transform_state,
            &id.to_unwrapped(),
            transform_state.get_projection_data_with(&id.to_unwrapped(), proj_matrix),
        );

        layout.result.feature_index.query(
            result,
            query_geometry,
            transform_state,
            &projector,
            mln_util::constants::TILE_SIZE_D * f64::from(id.overscale_factor()),
            2f64.powf(transform_state.get_zoom() - f64::from(id.overscaled_z)),
            options,
            global_state,
            &id,
            layers,
            query_padding * transform_state.max_pitch_scale_factor(),
            feature_state,
        )
    }

    fn query_source_features(
        &self,
        result: &mut Vec<Feature>,
        options: &SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), Error> {
        let layout = self.inner.layout();
        // Data not yet available, or tile is empty
        let Some(layout) = layout else {
            return Ok(());
        };
        let Some(data) = layout.result.feature_index.get_data() else {
            return Ok(());
        };

        // No source layers, specified, nothing to do
        let Some(source_layers) = &options.source_layers else {
            Log::warning(Event::General, "At least one sourceLayer required");
            return Ok(());
        };

        let id = self.inner.base.id();
        for source_layer in source_layers {
            // Go through all sourceLayers, if any
            // to gather all the features
            if let Some(layer) = data.layer(source_layer)? {
                let start = result.len();
                query_layer_source_features(&layer, result, options, global_state, &id)?;
                for feature in &mut result[start..] {
                    feature.source_layer.clone_from(source_layer);
                }
            }
        }
        Ok(())
    }
}

// query
use mln_util::feature::Feature;
use mln_util::mat4::Mat4;

use crate::geometry::feature_index::{QueryLayers, QueryResults};
use crate::query::{RenderedQueryOptions, SourceQueryOptions};
use crate::source_state::SourceFeatureState;

/// The feature loop of `GeometryTile::querySourceFeatures` and
/// `GeoJSONTile::querySourceFeatures`.
pub(crate) fn query_layer_source_features(
    layer: &mln_tile::TileLayer<'_>,
    result: &mut Vec<Feature>,
    options: &SourceQueryOptions,
    global_state: Option<&GlobalStateMap>,
    id: &OverscaledTileID,
) -> Result<(), Error> {
    for i in 0..layer.len() {
        let feature = layer.feature(i)?;

        // Apply filter, if any
        if let Some(filter) = &options.filter {
            let context =
                mln_style::expression::EvaluationContext::with_zoom_feature(f32::from(id.overscaled_z), Some(&feature))
                    .with_global_state(global_state);
            if !filter.evaluate(&context) {
                continue;
            }
        }

        let converted = mln_tile::convert_feature(&feature, &feature.geometry()?, &id.canonical)?;
        result.push(Feature::from(converted));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mln_actor::scheduler::{Scheduler, Task};
    use mln_style_types::image::{ImageMap, Impl as ImageImpl};
    use mln_util::identity::SimpleIdentity;
    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;
    use parking_lot::Mutex;

    use super::*;

    /// A scheduler that runs its tasks only when it is waited for.
    #[derive(Default)]
    struct Held(Mutex<Vec<Task>>);

    impl Scheduler for Held {
        fn schedule(&self, task: Task) {
            self.0.lock().push(task);
        }

        fn wait_for_empty(&self) {
            let tasks = std::mem::take(&mut *self.0.lock());
            for task in tasks {
                task();
            }
        }
    }

    /// The last reference to a layout frees its feature index, and the tile
    /// data in it, on the workers, and gives its atlas bins back here.
    #[test]
    fn a_layout_frees_its_data_on_the_workers_and_its_atlases_here() {
        let workers = Arc::new(Held::default());
        let scheduler: mln_actor::scheduler::SchedulerPtr = Arc::<Held>::clone(&workers);
        let thread_pool = TaggedScheduler::new(scheduler, SimpleIdentity::new());
        let features = Arc::new(Vec::new());
        let data = Arc::downgrade(&features);
        let mut atlas = DynamicTextureAtlas::new();
        let image = Arc::new(
            ImageImpl::new(
                "icon".to_owned(),
                PremultipliedImage::new(Size::new(4, 4)),
                1.0,
                false,
                Vec::new(),
                Vec::new(),
                None,
                None,
                None,
            )
            .expect("image"),
        );
        let images = atlas.pack_images(&ImageMap::from([(image.id.clone(), image)]), &ImageMap::new());
        let glyphs = atlas.pack_glyphs(&mln_text::glyph::GlyphMap::new());
        let atlases = TileAtlases { glyphs: glyphs.lease, images: images.lease, pattern_positions: images.patterns };
        let result =
            LayoutResult::new(HashMap::default(), Arc::new(FeatureIndex::new(Some(TileData::Features(features)))));
        let answer = Answer { atlases: Some(Rc::new(atlases)), ..Answer::default() };
        let layout = Rc::new(TileLayout::new(result, answer, Dependencies::default()));
        let render_data = Rc::clone(&layout);

        release_layout(layout, &thread_pool);
        atlas.free_released();
        assert_eq!(atlas.page_count(), 1, "the render data still holds the layout");
        release_layout(render_data, &thread_pool);
        atlas.free_released();
        assert_eq!(atlas.page_count(), 0, "the bins went back to the atlas");
        assert!(data.upgrade().is_some(), "the data waits for the workers");
        workers.wait_for_empty();
        assert!(data.upgrade().is_none(), "the workers freed the data");
    }
}
