//! The tile store owns every tile of every tiled source, the workers their
//! jobs run on, the glyph and image stores and the atlas their dependencies
//! resolve through, and the one inbox where what arrives for them waits.
//!
//! A tile's job (tile/job.rs) lays out its data, or decodes its image, for
//! one generation of its inputs. Each change of an input (its data, its
//! source's style, its layout parameters, an image its layout used or asked
//! for, the lane running its job) takes a new generation from its source's
//! counter, and a reply applies only if it names the current generation: a
//! stale result is dropped, and the tile keeps showing its contents while it
//! lays out again. At most one job per tile is in flight.
//!
//! The renderer drives the store: `receive` takes in the inbox's events and
//! starts the jobs they make ready, the sources update their covers, and
//! `drive` resolves what the waiting jobs need and starts the next jobs, in
//! the order the covers rank the tiles. A job that answers `NeedsDeps`
//! waits until the stores have its glyphs and images, which they pack into
//! one glyph page and one image page of the atlas, and runs again with
//! their positions. A tile of a source with synchronous updates runs its
//! jobs on the render thread as soon as they're ready.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mln_actor::TaggedScheduler;
use mln_storage::FileSource;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::logging::{Event, Log};
use mln_vec_map::VecSet;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image_store::ImageStore;
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::render_orchestrator::drop_on_workers;
use crate::source_state::SourceFeatureState;
use crate::text::glyph_store::GlyphStore;
use crate::tile::contents::{Answer, Contents, GeometryContents, TileAtlases, release};
use crate::tile::inbox::{Inbox, InboxSender, TileEvent, Waker};
use crate::tile::job::{
    DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceKey, SourceStyle, TileKey,
};
use crate::tile::loading::{DataOrigin, Load};
use crate::tile::source_tiles::SourceTiles;
use crate::tile::tile::{
    DependencyWait, Loaded, Next, Running, Tile, TileInput, TileKind, TileLoadOrder, TileOperation, Waiting, WorkState,
};
use crate::tile::tile_workers::{InlineWorkers, Jobs, Lane, ReplyBody, TileWorkers};

/// What the store reports to the renderer.
#[derive(Debug)]
pub(crate) enum StoreEvent {
    /// A tile changed: the renderer needs a frame to show it.
    Changed,
    /// Tile `tile` of source `source` failed to load or lay out.
    Error { source: String, tile: OverscaledTileID, error: Error },
    /// Tile `tile` of source `source` went through `op`.
    Action { op: TileOperation, tile: OverscaledTileID, source: String },
}

/// The parts of the store a source's tiles work with.
pub(crate) struct StoreContext {
    pub(crate) inbox: InboxSender,
    pub(crate) jobs: Jobs,
    pub(crate) glyphs: GlyphStore,
    pub(crate) images: ImageStore,
    pub(crate) atlas: DynamicTextureAtlas,
    /// The renderer's workers, which free what tiles let go of.
    pub(crate) pool: TaggedScheduler,
    /// Where glyphs load from: the last update's file source.
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    events: Vec<StoreEvent>,
}

impl StoreContext {
    pub(crate) fn changed(&mut self) {
        self.events.push(StoreEvent::Changed);
    }

    pub(crate) fn error(&mut self, source: &str, tile: OverscaledTileID, error: Error) {
        self.events.push(StoreEvent::Error { source: source.to_owned(), tile, error });
    }

    /// Replaces `tile`'s contents with `contents`, freeing the old ones.
    fn show(&self, tile: &mut Tile, contents: Option<Contents>) {
        let old = std::mem::replace(&mut tile.contents, contents.map(Rc::new));
        if let Some(old) = old {
            release(old, &self.pool);
        }
    }
}

/// What a source's tiles share: its identity, where their data comes from,
/// the counter their loads and generations take values from, and its
/// feature state.
pub(crate) struct SourceInfo {
    pub(crate) key: SourceKey,
    pub(crate) id: String,
    pub(crate) kind: TileKind,
    pub(crate) origin: Option<DataOrigin>,
    /// The last value taken. Loads and generations share it, so no two of
    /// the source's loads or jobs share a value.
    counter: u64,
    /// The source updates synchronously: its tiles' jobs run on the render
    /// thread as soon as they're ready.
    pub(crate) synchronous: bool,
    /// Whether the data of the source's tiles has one layer, which a source
    /// query gets whatever source layer it names.
    pub(crate) single_layer: bool,
    pub(crate) feature_state: SourceFeatureState,
}

/// What applying a reply did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Applied {
    Nothing,
    /// The tile shows new contents.
    Installed,
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

impl SourceInfo {
    pub(crate) fn new(key: SourceKey, id: String, kind: TileKind, single_layer: bool) -> Self {
        SourceInfo {
            key,
            id,
            kind,
            origin: None,
            counter: 0,
            synchronous: false,
            single_layer,
            feature_state: SourceFeatureState::new(),
        }
    }

    /// A new value of the source's counter.
    pub(crate) fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    pub(crate) fn action(&self, tile: &Tile, ctx: &mut StoreContext, op: TileOperation) {
        ctx.events.push(StoreEvent::Action { op, tile: tile.id, source: self.id.clone() });
    }

    /// A new tile `id`, which lays out with `params`. Its load starts with
    /// `start_load`.
    pub(crate) fn new_tile(&mut self, id: OverscaledTileID, params: LayoutParams) -> Option<Tile> {
        let load = Load::new(self.origin.as_ref()?, &id, params.pixel_ratio);
        let generation = self.next();
        Some(Tile::new(id, self.kind, generation, load, params))
    }

    /// Changes an input of `tile`, which takes a new generation; reports the
    /// start of a parse if the tile has work again.
    fn change(&mut self, tile: &mut Tile, ctx: &mut StoreContext, f: impl FnOnce(&mut Tile)) {
        let pending = tile.is_pending();
        f(tile);
        let generation = self.next();
        tile.bump(generation);
        if !pending && tile.is_pending() {
            self.action(tile, ctx, TileOperation::StartParse);
        }
    }

    /// Takes in the data of a load of `tile`. A raster tile without data
    /// shows nothing in its place at once, renderable unless `missing`.
    pub(crate) fn set_data(&mut self, tile: &mut Tile, ctx: &mut StoreContext, input: TileInput, missing: bool) {
        let key = DataKey { source: self.key, tile: tile.id.canonical, load: self.next() };
        match self.kind {
            TileKind::Geometry => {
                self.change(tile, ctx, |tile| {
                    tile.data = Some(Loaded { key, input });
                    tile.full = false;
                });
                self.advance(tile, ctx);
            }
            TileKind::Raster | TileKind::Dem(_) => {
                let has_data = matches!(input, TileInput::Image(Some(_)));
                let pending = tile.is_pending();
                tile.data = Some(Loaded { key, input });
                let generation = self.next();
                tile.bump(generation);
                if !pending {
                    self.action(tile, ctx, TileOperation::StartParse);
                }
                if !has_data {
                    tile.work = WorkState::Current;
                    self.show(tile, ctx, (!missing).then_some(Contents::Empty));
                }
            }
        }
    }

    /// Lays `tile` out with `style`, for the pixel ratio and subdivision
    /// granularity set since the last style.
    pub(crate) fn set_style(&mut self, tile: &mut Tile, ctx: &mut StoreContext, style: &Arc<SourceStyle>) {
        if tile.kind != TileKind::Geometry {
            return;
        }
        self.change(tile, ctx, |tile| {
            tile.style = Some(Arc::clone(style));
            tile.params.pixel_ratio = tile.pixel_ratio;
            tile.params.subdivision_granularity = tile.granularity;
        });
        self.advance(tile, ctx);
    }

    pub(crate) fn set_show_collision_boxes(&mut self, tile: &mut Tile, ctx: &mut StoreContext, show: bool) {
        if tile.kind != TileKind::Geometry || tile.params.show_collision_boxes == show {
            return;
        }
        self.change(tile, ctx, |tile| tile.params.show_collision_boxes = show);
        self.advance(tile, ctx);
    }

    /// Drops `tile`'s data and style, and leaves it waiting for new ones.
    pub(crate) fn reset(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        // A parse in progress is cancelled.
        if tile.is_pending() {
            self.action(tile, ctx, TileOperation::Cancelled);
        }
        // An unloaded tile signals no completion until it has data again.
        tile.loaded = false;
        let generation = self.next();
        tile.data = None;
        tile.style = None;
        tile.bump(generation);
    }

    /// Lays `tile` out again, with its data and style, for glyphs or images
    /// that changed: without the answer they had.
    fn relayout(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        self.change(tile, ctx, |tile| tile.answer_outdated = true);
        self.advance(tile, ctx);
    }

    /// Takes in a change of images: the ones in `changed` were added,
    /// removed or resized, and `available` exist now. Lays `tile` out again
    /// if the images it shows, or that a job resolved for it, name one of
    /// `changed`, or if a job waiting for its images branched on one that
    /// came or went (an image the stores still have to resolve resolves as
    /// it is now). A first try in flight is checked when it replies.
    pub(crate) fn images_changed(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        changed: &HashSet<String>,
        available: &VecSet<String>,
    ) {
        let names = |deps: &Dependencies| deps.images.keys().any(|id| changed.contains(id));
        let shown = tile.contents.as_deref().and_then(Contents::geometry).is_some_and(|contents| names(&contents.deps));
        let work = match &tile.work {
            WorkState::Running(Running { known: Some(known), .. })
            | WorkState::Ready(Next { known: Some(known), .. }) => names(known),
            WorkState::Waiting(Waiting { needs, .. }) => {
                needs.unavailable_images.iter().any(|id| changed.contains(id) && available.contains(id))
                    || needs.images.keys().any(|id| changed.contains(id) && !available.contains(id))
            }
            // A first try's reply tells whether the change touched it.
            WorkState::Running(_) | WorkState::Current | WorkState::Outdated | WorkState::Ready(_) => false,
        };
        if shown || work {
            self.relayout(tile, ctx);
        }
    }

    /// Lays `tile` out again if its layout, or the one it runs, lacked
    /// glyphs of a range that failed to load, which asks for the range
    /// again.
    pub(crate) fn retry_failed_glyphs(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        let running = match &tile.work {
            WorkState::Running(Running { answer, .. }) | WorkState::Ready(Next { answer, .. }) => answer.glyphs_failed,
            WorkState::Current | WorkState::Outdated | WorkState::Waiting(_) => false,
        };
        let shown =
            tile.contents.as_deref().and_then(Contents::geometry).is_some_and(|contents| contents.answer.glyphs_failed);
        if running || shown {
            self.relayout(tile, ctx);
        }
    }

    /// Calls `f` with the dependencies of the layout `tile` shows and of the
    /// one it lays out.
    pub(crate) fn for_each_dependencies(tile: &Tile, mut f: impl FnMut(&Dependencies)) {
        if let Some(contents) = tile.contents.as_deref().and_then(Contents::geometry) {
            f(&contents.deps);
        }
        if let Some(deps) = tile.work.dependencies() {
            f(deps);
        }
    }

    /// Lets go of `tile`, which leaves its source: a job in flight needn't
    /// run, its request is cancelled, and its contents are freed.
    pub(crate) fn drop_tile(&self, mut tile: Tile, ctx: &mut StoreContext) {
        // This tile never finished loading or was abandoned.
        if tile.is_pending() {
            self.action(&tile, ctx, TileOperation::Cancelled);
        }
        if let WorkState::Running(running) = &tile.work {
            running.superseded.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.abandon_load(&tile);
        ctx.show(&mut tile, None);
    }

    /// Whether `tile` has a job it can submit, or dependencies to resolve.
    pub(crate) fn has_work(tile: &Tile) -> bool {
        match tile.work {
            WorkState::Waiting(_) => true,
            // A tile that has yet to get its data has nothing to submit.
            WorkState::Outdated | WorkState::Ready(_) => tile.data.is_some() && tile.is_pending(),
            WorkState::Current | WorkState::Running(_) => false,
        }
    }

    /// Runs a synchronous source's ready jobs at once.
    fn advance(&mut self, tile: &mut Tile, ctx: &mut StoreContext) -> bool {
        self.synchronous && self.run_inline(tile, ctx)
    }

    /// Moves `tile`'s work on: resolves the dependencies it waits for, once
    /// the stores have them all or know them missing, and starts its next
    /// job if a lane has room, or at once for a synchronous source. Returns
    /// whether the tile shows new contents.
    pub(crate) fn drive(&mut self, tile: &mut Tile, ctx: &mut StoreContext) -> bool {
        Self::resolve(tile, ctx);
        if self.synchronous {
            return self.run_inline(tile, ctx);
        }
        if let Some((run, running)) = self.next_job(tile, ctx, false) {
            ctx.jobs.submit(running.lane, run, &ctx.inbox);
            tile.work = WorkState::Running(running);
        }
        false
    }

    /// Runs `tile`'s jobs on this thread until one waits for dependencies or
    /// the tile shows its contents. Returns whether it does.
    fn run_inline(&mut self, tile: &mut Tile, ctx: &mut StoreContext) -> bool {
        let mut installed = false;
        while let Some((run, running)) = self.next_job(tile, ctx, true) {
            let reply = InlineWorkers::run(run);
            installed |= self.apply_result(tile, ctx, running, reply.result) == Applied::Installed;
        }
        installed
    }

    /// `tile`'s next job and its running state; `None` if the tile has no
    /// job to run, lacks its data or style, or no lane has room. `inline`
    /// runs it on this thread.
    fn next_job(&mut self, tile: &mut Tile, ctx: &mut StoreContext, inline: bool) -> Option<(Run, Running)> {
        let id = JobId { tile: TileKey { source: self.key, id: tile.id }, generation: tile.generation };
        if tile.kind != TileKind::Geometry {
            let (WorkState::Outdated, Some(Loaded { input: TileInput::Image(Some(data)), .. })) =
                (&tile.work, &tile.data)
            else {
                return None;
            };
            let lane = if inline { Lane(0) } else { ctx.jobs.lane()? };
            let job = match tile.kind {
                TileKind::Dem(encoding) => Job::Dem { data: Arc::clone(data), encoding },
                TileKind::Raster | TileKind::Geometry => Job::Raster { data: Arc::clone(data) },
            };
            let superseded = Arc::new(AtomicBool::new(false));
            let run = Run { id, job, superseded: Arc::clone(&superseded) };
            let running =
                Running { id, lane, known: None, answer: Answer::default(), images: Arc::default(), superseded };
            return Some((run, running));
        }

        let (Some(loaded), Some(style)) = (&tile.data, &tile.style) else {
            return None;
        };
        let shown = tile.contents.as_deref().and_then(Contents::geometry);
        let next = match std::mem::replace(&mut tile.work, WorkState::Outdated) {
            WorkState::Outdated => Next {
                lane: None,
                known: None,
                images: None,
                answer: shown
                    .filter(|_| !tile.answer_outdated)
                    .map(|contents| contents.answer.clone())
                    .unwrap_or_default(),
            },
            WorkState::Ready(next) => next,
            work @ (WorkState::Current | WorkState::Running(_) | WorkState::Waiting(_)) => {
                tile.work = work;
                return None;
            }
        };
        let full = tile.full || loaded.data().is_none();
        let placed = if inline { Some((Lane(0), false)) } else { ctx.jobs.place(loaded.key, full) };
        let Some((lane, cached)) = placed else {
            // A first try needs nothing kept; a resume keeps its answer.
            if next.known.is_some() || tile.full {
                tile.work = WorkState::Ready(next);
            }
            return None;
        };
        let data = if cached {
            DataInput::Cached(loaded.key)
        } else {
            DataInput::Full { key: loaded.key, data: loaded.data().cloned() }
        };
        let params = LayoutParams { first_load: tile.contents.is_none(), ..tile.params };
        let images = next.images.unwrap_or_else(|| Arc::clone(ctx.images.available_images()));
        let job = LayoutJob {
            style: Arc::clone(style),
            available_images: Arc::clone(&images),
            data,
            params,
            resolved: Arc::clone(&next.answer.resolved),
        };
        tile.full = false;
        tile.answer_outdated = false;
        let superseded = Arc::new(AtomicBool::new(false));
        let run = Run { id, job: Job::Layout(job), superseded: Arc::clone(&superseded) };
        Some((run, Running { id, lane, known: next.known, answer: next.answer, images, superseded }))
    }

    /// Resolves the dependencies `tile`'s job waits for once the stores have
    /// them all or know them missing, packing them into the atlas. Does
    /// nothing for a tile that waits for nothing, or whose stores haven't
    /// changed since it last tried.
    fn resolve(tile: &mut Tile, ctx: &mut StoreContext) {
        let WorkState::Waiting(Waiting { lane, needs, wait, images: seen }) = &mut tile.work else {
            return;
        };
        let revisions = (ctx.glyphs.revision(), ctx.images.revision());
        if wait.revisions == Some(revisions) {
            return;
        }
        let glyphs = ctx.glyphs.resolve(&needs.glyphs, &mut wait.glyphs, ctx.file_source.as_ref(), &ctx.inbox);
        let images = ctx.images.resolve(&needs.images, &mut wait.images);
        let (Some(glyphs), Some(images)) = (glyphs, images) else {
            wait.revisions = Some((ctx.glyphs.revision(), ctx.images.revision()));
            return;
        };

        let glyph_page = ctx.atlas.pack_glyphs(&glyphs.glyphs);
        let image_page = ctx.atlas.pack_images(&images.icons, &images.patterns);
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
        let next =
            Next { lane: Some(*lane), known: Some(std::mem::take(needs)), answer, images: Some(Arc::clone(seen)) };
        tile.work = WorkState::Ready(next);
    }

    /// Applies the reply to job `id` of `tile`, which runs it. Returns
    /// whether the tile shows new contents.
    pub(crate) fn apply_reply(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        lane: Lane,
        id: JobId,
        body: ReplyBody,
    ) -> bool {
        if id.generation != tile.generation {
            // The tile lays out its newest generation instead.
            tile.work = WorkState::Outdated;
            drop_on_workers(&ctx.pool, body);
            return false;
        }
        // A reply of a worker lost since: the loss, which comes next, runs
        // the job again.
        let Some(result) = ctx.jobs.decode(lane, body) else {
            return false;
        };
        let WorkState::Running(running) = std::mem::replace(&mut tile.work, WorkState::Outdated) else {
            return false;
        };
        self.apply_result(tile, ctx, running, result) == Applied::Installed
    }

    /// Applies the result of `running`, `tile`'s job.
    fn apply_result(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        running: Running,
        result: JobResult,
    ) -> Applied {
        if tile.kind != TileKind::Geometry {
            return self.apply_decode(tile, ctx, running.id, result);
        }
        let Running { id, lane, known, answer, images, .. } = running;
        let available = ctx.images.available_images();
        let outdated_images = !Arc::ptr_eq(&images, available) && outdated_by(&result, &images, available);
        if id.generation != tile.generation || outdated_images {
            // Stale: the tile lays out its newest generation instead.
            if outdated_images {
                tile.answer_outdated = true;
                let generation = self.next();
                tile.bump(generation);
            }
            tile.work = WorkState::Outdated;
            drop_on_workers(&ctx.pool, result);
            return Applied::Nothing;
        }
        match result {
            JobResult::Laid(output) => {
                tile.work = WorkState::Current;
                // A resume's answer is what it asked for; a first try's may be
                // the layout before's, which can hold more.
                let answer = if known.is_none() {
                    answer.trimmed(&output.deps, &mut ctx.atlas).unwrap_or(answer)
                } else {
                    answer
                };
                self.install(tile, ctx, output, answer);
                Applied::Installed
            }
            JobResult::NeedsDeps(needs) if known.is_none() => {
                tile.work = WorkState::Waiting(Waiting { lane, needs, wait: DependencyWait::default(), images });
                Applied::Nothing
            }
            JobResult::NeedsDeps(_) => {
                tile.work = WorkState::Current;
                Log::error(
                    Event::General,
                    format!("Tile {} asked again for the dependencies it was given", id.tile.id),
                );
                self.job_error(tile, ctx, Error::native("a layout asked again for the dependencies it was given"));
                Applied::Nothing
            }
            JobResult::NeedsData => {
                tile.full = true;
                tile.work = WorkState::Ready(Next { lane: None, known, answer, images: Some(images) });
                Applied::Nothing
            }
            JobResult::Failed(error) => {
                tile.work = WorkState::Current;
                self.job_error(tile, ctx, error);
                Applied::Nothing
            }
            JobResult::Raster(_) | JobResult::Dem(_) => {
                tile.work = WorkState::Current;
                self.job_error(tile, ctx, Error::native("a geometry tile's layout answered as a decode"));
                Applied::Nothing
            }
        }
    }

    /// Applies the result of a raster or raster-dem tile's decode: the tile
    /// lets go of the image it decoded.
    fn apply_decode(&mut self, tile: &mut Tile, ctx: &mut StoreContext, id: JobId, result: JobResult) -> Applied {
        if id.generation != tile.generation {
            tile.work = WorkState::Outdated;
            return Applied::Nothing;
        }
        tile.work = WorkState::Current;
        if let Some(Loaded { input: TileInput::Image(data), .. }) = &mut tile.data {
            *data = None;
        }
        let contents = match result {
            JobResult::Raster(image) => Contents::Raster(Arc::new(RasterBucket::new(image))),
            JobResult::Dem(dem) => Contents::Dem(Arc::new(HillshadeBucket::with_dem_data(dem))),
            JobResult::Failed(error) => {
                self.job_error(tile, ctx, error);
                return Applied::Nothing;
            }
            JobResult::Laid(_) | JobResult::NeedsDeps(_) | JobResult::NeedsData => {
                self.job_error(tile, ctx, Error::native("a raster tile's decode answered as a layout"));
                return Applied::Nothing;
            }
        };
        self.show(tile, ctx, Some(contents));
        Applied::Installed
    }

    /// Shows `contents`, or nothing, as the tile's decode or lack of data.
    fn show(&self, tile: &mut Tile, ctx: &mut StoreContext, contents: Option<Contents>) {
        ctx.show(tile, contents);
        tile.loaded = true;
        self.action(tile, ctx, TileOperation::EndParse);
        ctx.changed();
    }

    /// Shows a layout of the current generation, with its feature index on
    /// the data it was built from and the source's feature state applied.
    fn install(&mut self, tile: &mut Tile, ctx: &mut StoreContext, output: LayoutOutput, answer: Answer) {
        let LayoutOutput { mut result, deps } = output;
        let Some(loaded) = &tile.data else {
            return;
        };
        // The index crossed without its data, or with the data it was built
        // from, which is the tile's.
        if let Some(feature_index) = Arc::get_mut(&mut result.feature_index) {
            feature_index.set_data(loaded.data().cloned());
        }
        let contents = GeometryContents::new(result, tile.data_missing(), answer, deps);
        contents.set_feature_state(self.feature_state.current_states());
        tile.feature_state_version = self.feature_state.version();
        tile.loaded = true;
        self.action(tile, ctx, TileOperation::EndParse);
        ctx.show(tile, Some(Contents::Geometry(contents)));
        ctx.changed();
    }

    /// Applies `result` as the reply to `tile`'s job of its current
    /// generation.
    #[cfg(test)]
    pub(crate) fn apply_now(&mut self, tile: &mut Tile, ctx: &mut StoreContext, result: JobResult) -> bool {
        let id = JobId { tile: TileKey { source: self.key, id: tile.id }, generation: tile.generation };
        let running = Running {
            id,
            lane: Lane(0),
            known: None,
            answer: Answer::default(),
            images: Arc::clone(ctx.images.available_images()),
            superseded: Arc::default(),
        };
        self.apply_result(tile, ctx, running, result) == Applied::Installed
    }

    /// `tile`'s job failed: it keeps what it shows.
    pub(crate) fn job_error(&self, tile: &mut Tile, ctx: &mut StoreContext, error: Error) {
        tile.loaded = true;
        self.action(tile, ctx, TileOperation::Error);
        ctx.error(&self.id, tile.id, error);
    }

    /// The worker of `lane` is gone: `tile`'s job there, or the state its
    /// worker kept for it, goes, and its work starts again.
    pub(crate) fn lane_lost(&mut self, tile: &mut Tile, lane: Lane) {
        if tile.work.lane() != Some(lane) {
            return;
        }
        let generation = self.next();
        tile.bump(generation);
        // Its reply never comes, and its leases go.
        tile.work = WorkState::Outdated;
    }
}

/// The source of `key` and the id of its tile `is` holds for: the tile of
/// `key`'s id if it holds, else one that moved to another wrap since.
fn find(
    sources: &mut HashMap<SourceKey, SourceTiles>,
    key: TileKey,
    is: impl Fn(&Tile) -> bool,
) -> Option<(&mut SourceTiles, OverscaledTileID)> {
    let source = sources.get_mut(&key.source)?;
    if source.tile(&key.id).is_some_and(&is) {
        return Some((source, key.id));
    }
    let id = source.slots().find(|slot| is(&slot.tile))?.tile.id;
    Some((source, id))
}

/// Where the tile store keeps what it owns.
pub(crate) struct TileStore {
    sources: HashMap<SourceKey, SourceTiles>,
    /// The sources in the style's order, which orders the tiles' jobs within
    /// a load order.
    order: Vec<SourceKey>,
    next_source: u64,
    /// Whether the sources keep the tiles they no longer show for a while.
    cache_enabled: bool,
    inbox: Inbox,
    ctx: StoreContext,
}

impl TileStore {
    /// A store whose tiles' jobs run on `workers`, which frees on `pool` and
    /// calls `wake` when something arrives for its tiles.
    pub(crate) fn new(
        workers: Box<dyn TileWorkers>,
        glyphs: GlyphStore,
        images: ImageStore,
        pool: TaggedScheduler,
        wake: Waker,
    ) -> Self {
        let inbox = Inbox::new(wake);
        let ctx = StoreContext {
            inbox: inbox.sender().clone(),
            jobs: Jobs::new(workers),
            glyphs,
            images,
            atlas: DynamicTextureAtlas::new(),
            pool,
            file_source: None,
            events: Vec::new(),
        };
        TileStore { sources: HashMap::default(), order: Vec::new(), next_source: 0, cache_enabled: true, inbox, ctx }
    }

    pub(crate) fn glyphs(&mut self) -> &mut GlyphStore {
        &mut self.ctx.glyphs
    }

    pub(crate) fn images(&self) -> &ImageStore {
        &self.ctx.images
    }

    pub(crate) fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.ctx.images
    }

    pub(crate) fn atlas(&mut self) -> &mut DynamicTextureAtlas {
        &mut self.ctx.atlas
    }

    /// Where events for the tiles go.
    #[cfg(test)]
    pub(crate) fn inbox(&self) -> &InboxSender {
        &self.ctx.inbox
    }

    /// Sets where glyphs load from.
    pub(crate) fn set_file_source(&mut self, file_source: Option<Arc<dyn FileSource>>) {
        self.ctx.file_source = file_source;
    }

    /// Runs the tiles' jobs in `workers`. The jobs in flight on the previous
    /// workers run again there.
    pub(crate) fn set_workers(&mut self, workers: Box<dyn TileWorkers>) {
        self.ctx.jobs.set_workers(workers);
        for source in self.sources.values_mut() {
            let lanes: Vec<Lane> = source.slots().filter_map(|slot| slot.tile.work.lane()).collect();
            for lane in lanes {
                source.lane_lost(lane);
            }
        }
    }

    /// Adds a source whose tiles are of `kind`; `single_layer` for one whose
    /// data has one layer. The source comes after the others in the style's
    /// order.
    pub(crate) fn add_source(&mut self, id: String, kind: TileKind, single_layer: bool) -> SourceKey {
        self.next_source += 1;
        let key = SourceKey(self.next_source);
        let mut source = SourceTiles::new(SourceInfo::new(key, id, kind, single_layer));
        source.set_cache_enabled(self.cache_enabled);
        self.sources.insert(key, source);
        self.order.push(key);
        key
    }

    /// Removes a source and lets go of its tiles.
    pub(crate) fn remove_source(&mut self, key: SourceKey) {
        if let Some(mut source) = self.sources.remove(&key) {
            source.clear_all(&mut self.ctx);
        }
        self.order.retain(|other| *other != key);
    }

    /// Removes every source.
    pub(crate) fn clear(&mut self) {
        for key in std::mem::take(&mut self.order) {
            self.remove_source(key);
        }
    }

    pub(crate) fn source(&self, key: SourceKey) -> Option<&SourceTiles> {
        self.sources.get(&key)
    }

    /// The source `key`, and the parts of the store its tiles work with.
    pub(crate) fn source_mut(&mut self, key: SourceKey) -> Option<(&mut SourceTiles, &mut StoreContext)> {
        Some((self.sources.get_mut(&key)?, &mut self.ctx))
    }

    /// Takes the store's events, oldest first.
    pub(crate) fn take_events(&mut self) -> Vec<StoreEvent> {
        std::mem::take(&mut self.ctx.events)
    }

    /// Takes in what arrived for the tiles: their data, their jobs' replies,
    /// the lanes lost and the glyphs. Tiles whose layout arrives show it.
    pub(crate) fn receive(&mut self) {
        self.ctx.glyphs.update();
        while let Some(event) = self.inbox.next() {
            match event {
                TileEvent::Response { tile, load, response } => {
                    let awaits = |tile: &Tile| tile.load.awaits_response(load);
                    if let Some((source, tile)) = find(&mut self.sources, tile, awaits) {
                        source.update(&tile, &mut self.ctx, |info, tile, ctx| {
                            info.apply_response(tile, ctx, load, &response);
                        });
                    }
                }
                TileEvent::Data { tile, load, data } => {
                    if let Some((source, tile)) = find(&mut self.sources, tile, |tile| tile.load.accepts(load)) {
                        source.update(&tile, &mut self.ctx, |info, tile, ctx| {
                            info.apply_source_data(tile, ctx, load, data);
                        });
                    }
                }
                TileEvent::Reply { lane, id, body, times } => {
                    if !self.ctx.jobs.replied(&id, times) {
                        drop_on_workers(&self.ctx.pool, body);
                        continue;
                    }
                    let running = |tile: &Tile| matches!(&tile.work, WorkState::Running(running) if running.id == id);
                    match find(&mut self.sources, id.tile, running) {
                        Some((source, tile)) => source.apply_reply(&tile, &mut self.ctx, lane, id, body),
                        None => drop_on_workers(&self.ctx.pool, body),
                    }
                }
                TileEvent::LaneLost(lane) => {
                    self.ctx.jobs.lost(lane);
                    for source in self.sources.values_mut() {
                        source.lane_lost(lane);
                    }
                }
                TileEvent::Glyphs { font_stack, range, id, result } => {
                    self.ctx.glyphs.take_response(&font_stack, range, id, result);
                }
            }
        }
    }

    /// Resolves the dependencies the tiles' jobs wait for, once the stores
    /// have them or know them missing, and submits the tiles' next jobs, the
    /// view's first, while the workers have room. Returns whether a tile of
    /// a source with synchronous updates laid out.
    pub(crate) fn drive(&mut self) -> bool {
        let mut tiles: Vec<(TileLoadOrder, usize, OverscaledTileID)> = Vec::new();
        for (index, key) in self.order.iter().enumerate() {
            if let Some(source) = self.sources.get(key) {
                tiles.extend(
                    source
                        .slots()
                        .filter(|slot| SourceInfo::has_work(&slot.tile))
                        .map(|slot| (slot.place.order(), index, slot.tile.id)),
                );
            }
        }
        tiles.sort_by_key(|(order, index, _)| (*order, *index));
        let mut laid_out = false;
        for (_, index, id) in tiles {
            let Some(source) = self.sources.get_mut(&self.order[index]) else {
                continue;
            };
            laid_out |= source.drive(&id, &mut self.ctx);
        }
        laid_out
    }

    /// Takes in a change of images: those in `changed` were added, removed
    /// or resized. Lays out again the tiles that it touches.
    pub(crate) fn images_changed(&mut self, changed: &HashSet<String>) {
        let available = Arc::clone(self.ctx.images.available_images());
        for key in &self.order {
            if let Some(source) = self.sources.get_mut(key) {
                source.update_each(
                    &mut self.ctx,
                    |_| true,
                    |info, tile, ctx| info.images_changed(tile, ctx, changed, &available),
                );
            }
        }
    }

    /// Lays out again the tiles laid out without the glyphs of a failed
    /// range, which requests the range again, and requests the tiles whose
    /// load failed again.
    pub(crate) fn retry_failed(&mut self) {
        for key in &self.order {
            if let Some(source) = self.sources.get_mut(key) {
                source.update_each(&mut self.ctx, |_| true, SourceInfo::retry_failed_glyphs);
                source.retry_failed_tiles(&mut self.ctx);
            }
        }
    }

    /// The images the tiles' dependencies name.
    pub(crate) fn images_used(&self) -> HashSet<String> {
        let mut used = HashSet::default();
        for source in self.sources.values() {
            for slot in source.slots() {
                SourceInfo::for_each_dependencies(&slot.tile, |deps| used.extend(deps.images.keys().cloned()));
            }
        }
        used
    }

    pub(crate) fn update_fading_tiles(&mut self) {
        for source in self.sources.values_mut() {
            source.update_fading_tiles();
        }
    }

    pub(crate) fn has_fading_tiles(&self) -> bool {
        self.sources.values().any(SourceTiles::has_fading_tiles)
    }

    pub(crate) fn set_cache_enabled(&mut self, enable: bool) {
        self.cache_enabled = enable;
        for source in self.sources.values_mut() {
            source.set_cache_enabled(enable);
        }
    }

    pub(crate) fn reduce_memory_use(&mut self) {
        for source in self.sources.values_mut() {
            source.reduce_memory_use(&mut self.ctx);
        }
    }

    pub(crate) fn dump_debug_logs(&self) {
        for source in self.sources.values() {
            source.dump_debug_logs();
        }
        self.ctx.images.dump_debug_logs();
        self.ctx.jobs.log_stats();
    }

    pub(crate) fn log_stats(&self) {
        self.ctx.jobs.log_stats();
    }

    /// The renderer's workers, which free what tiles let go of.
    pub(crate) fn pool(&self) -> &TaggedScheduler {
        &self.ctx.pool
    }

    /// Whether no job is in flight.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.ctx.jobs.is_idle()
    }
}
