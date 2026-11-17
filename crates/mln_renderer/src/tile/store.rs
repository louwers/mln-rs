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

use mln_actor::TaggedScheduler;
use mln_storage::FileSource;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image_store::ImageStore;
use crate::layout::tile_layout::{LayoutParams, Resolved};
use crate::render_orchestrator::drop_on_workers;
use crate::source_state::SourceFeatureState;
use crate::text::glyph_store::GlyphStore;
use crate::tile::contents::{Answer, Contents, GeometryContents, TileAtlases, release};
use crate::tile::inbox::{Inbox, InboxSender, TileEvent, Waker};
use crate::tile::job::{DataKey, JobId, JobResult, LayoutOutput, Run, SourceKey, SourceStyle, TileKey};
use crate::tile::loading::{DataOrigin, Load};
use crate::tile::pipeline::{DecodeStep, Layout, LayoutData, LayoutStep, Pipeline, Resume, Running, Waiting};
use crate::tile::source_tiles::SourceTiles;
use crate::tile::tile::{Shown, Tile, TileInput, TileKind, TileLoadOrder, TileOperation};
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

    /// Replaces what `tile` shows with `shown`, freeing the old contents.
    fn show(&self, tile: &mut Tile, shown: Option<Shown>) {
        if let Some(old) = std::mem::replace(&mut tile.shown, shown) {
            release(old.contents, &self.pool);
        }
    }

    /// Resolves the dependencies `waiting` needs once the stores have them
    /// all or know them missing, packing them into the atlas: the resume of
    /// its job. `None` while the stores lack some, or haven't changed since
    /// the last try.
    fn resolve(&mut self, waiting: &mut Waiting) -> Option<Resume> {
        let Waiting { lane, needs, wait, images: seen } = waiting;
        let revisions = (self.glyphs.revision(), self.images.revision());
        if wait.revisions == Some(revisions) {
            return None;
        }
        let glyphs = self.glyphs.resolve(&needs.glyphs, &mut wait.glyphs, self.file_source.as_ref(), &self.inbox);
        let images = self.images.resolve(&needs.images, &mut wait.images);
        let (Some(glyphs), Some(images)) = (glyphs, images) else {
            wait.revisions = Some((self.glyphs.revision(), self.images.revision()));
            return None;
        };

        let glyph_page = self.atlas.pack_glyphs(&glyphs.glyphs);
        let image_page = self.atlas.pack_images(&images.icons, &images.patterns);
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
        Some(Resume { lane: Some(*lane), known: std::mem::take(needs), answer, images: Arc::clone(seen) })
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
        Some(Tile::new(id, load, Pipeline::new(self.kind, generation, params)))
    }

    /// The key of `tile` in the store's messages.
    fn tile_key(&self, tile: &Tile) -> TileKey {
        TileKey { source: self.key, id: tile.id }
    }

    /// Changes an input of a geometry `tile`'s layout through `f`, at a new
    /// generation; reports the start of a parse if the tile has work again.
    /// Returns whether the tile lays out.
    fn change(&mut self, tile: &mut Tile, ctx: &mut StoreContext, f: impl FnOnce(&mut Layout, u64)) -> bool {
        let pending = tile.is_pending();
        let generation = self.next();
        let Some(layout) = tile.pipeline.layout_mut() else {
            return false;
        };
        f(layout, generation);
        if !pending && tile.is_pending() {
            self.action(tile, ctx, TileOperation::StartParse);
        }
        true
    }

    /// Takes in what a load of `tile` delivered. A raster tile without an
    /// image shows nothing in its place at once, renderable unless
    /// `missing`.
    pub(crate) fn set_data(&mut self, tile: &mut Tile, ctx: &mut StoreContext, input: TileInput) {
        match input {
            TileInput::Geometry { data, missing } => {
                let key = DataKey { source: self.key, tile: tile.id.canonical, load: self.next() };
                if self.change(tile, ctx, |layout, generation| {
                    layout.set_data(LayoutData { key, data, missing }, generation);
                }) {
                    self.advance(tile, ctx);
                }
            }
            TileInput::Image { data, missing } => {
                let pending = tile.is_pending();
                let has_image = data.is_some();
                let generation = self.next();
                let Pipeline::Decode(decode) = &mut tile.pipeline else {
                    return;
                };
                decode.set_image(data, generation);
                if !pending {
                    self.action(tile, ctx, TileOperation::StartParse);
                }
                if !has_image {
                    self.install(tile, ctx, (!missing).then_some(Contents::Empty));
                }
            }
        }
    }

    /// Lays `tile` out with `style`, for `pixel_ratio` and `granularity`.
    pub(crate) fn set_style(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        style: &Arc<SourceStyle>,
        pixel_ratio: f32,
        granularity: SubdivisionGranularitySetting,
    ) {
        if self.change(tile, ctx, |layout, generation| {
            layout.set_style(Arc::clone(style), pixel_ratio, granularity, generation);
        }) {
            self.advance(tile, ctx);
        }
    }

    pub(crate) fn set_show_collision_boxes(&mut self, tile: &mut Tile, ctx: &mut StoreContext, show: bool) {
        if tile.pipeline.layout().is_none_or(|layout| layout.params().show_collision_boxes == show) {
            return;
        }
        if self.change(tile, ctx, |layout, generation| layout.set_show_collision_boxes(show, generation)) {
            self.advance(tile, ctx);
        }
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
        if let Some(layout) = tile.pipeline.layout_mut() {
            layout.reset(generation);
        }
    }

    /// Lays `tile` out again, with its data and style, for glyphs or images
    /// that changed: without the answer the layout it shows had.
    fn relayout(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        if let Some(shown) = &mut tile.shown {
            shown.answer_outdated = true;
        }
        if self.change(tile, ctx, Layout::bump) {
            self.advance(tile, ctx);
        }
    }

    /// Takes in a change of images: the ones in `changed` were added,
    /// removed or resized, and `available` exist now. Lays `tile` out again
    /// if the images it shows name one of `changed`, or if its work does
    /// (`Layout::touches_images`).
    pub(crate) fn images_changed(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        changed: &HashSet<String>,
        available: &VecSet<String>,
    ) {
        let shown = tile
            .contents()
            .and_then(Contents::geometry)
            .is_some_and(|contents| contents.deps.images.keys().any(|id| changed.contains(id)));
        let work = tile.pipeline.layout().is_some_and(|layout| layout.touches_images(changed, available));
        if shown || work {
            self.relayout(tile, ctx);
        }
    }

    /// Lays `tile` out again if its layout, or the one it runs, lacked
    /// glyphs of a range that failed to load, which asks for the range
    /// again.
    pub(crate) fn retry_failed_glyphs(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        let running = tile.pipeline.layout().is_some_and(Layout::lacks_failed_glyphs);
        let shown = tile.contents().and_then(Contents::geometry).is_some_and(|contents| contents.answer.glyphs_failed);
        if running || shown {
            self.relayout(tile, ctx);
        }
    }

    /// Lets go of `tile`, which leaves its source: a job in flight needn't
    /// run, its request is cancelled, and its contents are freed.
    pub(crate) fn drop_tile(&self, mut tile: Tile, ctx: &mut StoreContext) {
        // This tile never finished loading or was abandoned.
        if tile.is_pending() {
            self.action(&tile, ctx, TileOperation::Cancelled);
        }
        if let Some(job) = tile.pipeline.in_flight() {
            job.superseded.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.abandon_load(&tile);
        ctx.show(&mut tile, None);
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
        if let Some(layout) = tile.pipeline.layout_mut() {
            layout.resolve(|waiting| ctx.resolve(waiting));
        }
        if self.synchronous {
            return self.run_inline(tile, ctx);
        }
        if let Some((lane, run)) = self.next_job(tile, ctx, false) {
            ctx.jobs.submit(lane, run, &ctx.inbox);
        }
        false
    }

    /// Runs `tile`'s jobs on this thread until one waits for dependencies or
    /// the tile shows its contents. Returns whether it does.
    fn run_inline(&mut self, tile: &mut Tile, ctx: &mut StoreContext) -> bool {
        let mut installed = false;
        while let Some((_, run)) = self.next_job(tile, ctx, true) {
            let reply = InlineWorkers::run(run);
            installed |= self.apply_result(tile, ctx, reply.id, reply.result) == Applied::Installed;
        }
        installed
    }

    /// Puts `tile`'s next job in flight, and returns it with its lane;
    /// `None` if the tile has no job to run, lacks its data or style, or no
    /// lane has room. `inline` runs it on this thread.
    fn next_job(&mut self, tile: &mut Tile, ctx: &mut StoreContext, inline: bool) -> Option<(Lane, Run)> {
        let key = self.tile_key(tile);
        match &mut tile.pipeline {
            Pipeline::Layout(layout) => {
                let images = ctx.images.available_images();
                let jobs = &mut ctx.jobs;
                layout.next_job(key, tile.shown.as_ref(), images, |data, full| {
                    if inline { Some((Lane(0), false)) } else { jobs.place(data, full) }
                })
            }
            Pipeline::Decode(decode) => {
                decode.next_job(key, self.kind, || if inline { Some(Lane(0)) } else { ctx.jobs.lane() })
            }
        }
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
        if id.generation != tile.pipeline.generation() {
            // The tile lays out or decodes its newest generation instead.
            tile.pipeline.stale_replied();
            drop_on_workers(&ctx.pool, body);
            return false;
        }
        // A reply of a worker lost since: the loss, which comes next, runs
        // the job again.
        let Some(result) = ctx.jobs.decode(lane, body) else {
            return false;
        };
        self.apply_result(tile, ctx, id, result) == Applied::Installed
    }

    /// Applies `result`, the reply to `tile`'s job `id`, which is in flight.
    fn apply_result(&mut self, tile: &mut Tile, ctx: &mut StoreContext, id: JobId, result: JobResult) -> Applied {
        match &mut tile.pipeline {
            Pipeline::Decode(decode) => match decode.apply(id, result) {
                DecodeStep::Stale => Applied::Nothing,
                DecodeStep::Show(contents) => {
                    self.install(tile, ctx, Some(contents));
                    Applied::Installed
                }
                DecodeStep::Failed(error) => {
                    self.job_error(tile, ctx, error);
                    Applied::Nothing
                }
            },
            Pipeline::Layout(layout) => {
                let Some(running) = layout.take_running() else {
                    return Applied::Nothing;
                };
                self.apply_layout(tile, ctx, running, result)
            }
        }
    }

    /// Applies `result`, the reply to `running`, `tile`'s layout job.
    fn apply_layout(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        running: Running,
        result: JobResult,
    ) -> Applied {
        let Some(layout) = tile.pipeline.layout_mut() else {
            return Applied::Nothing;
        };
        match layout.apply(running, result, ctx.images.available_images()) {
            LayoutStep::Show { output, answer, first_try } => {
                // A resume's answer is what it asked for; a first try's may be
                // the layout before's, which can hold more.
                let answer =
                    if first_try { answer.trimmed(&output.deps, &mut ctx.atlas).unwrap_or(answer) } else { answer };
                let LayoutOutput { mut result, deps } = output;
                let (data, missing) = layout.data().map_or((None, false), |data| (data.data.clone(), data.missing));
                // The index crossed without its data, or with the data it was
                // built from, which is the tile's.
                if let Some(feature_index) = Arc::get_mut(&mut result.feature_index) {
                    feature_index.set_data(data);
                }
                let contents = GeometryContents::new(result, missing, answer, deps);
                self.install(tile, ctx, Some(Contents::Geometry(contents)));
                Applied::Installed
            }
            LayoutStep::Wait => Applied::Nothing,
            LayoutStep::Resend(key) => {
                // The lane that last got the data lost it: the next job
                // carries it.
                ctx.jobs.forget(key);
                Applied::Nothing
            }
            LayoutStep::Outdated { images_moved, result } => {
                if images_moved {
                    let generation = self.next();
                    if let Some(layout) = tile.pipeline.layout_mut() {
                        layout.bump(generation);
                    }
                    if let Some(shown) = &mut tile.shown {
                        shown.answer_outdated = true;
                    }
                }
                drop_on_workers(&ctx.pool, result);
                Applied::Nothing
            }
            LayoutStep::Failed(error) => {
                self.job_error(tile, ctx, error);
                Applied::Nothing
            }
        }
    }

    /// Shows `contents`, or nothing, as the tile's newest: geometry contents
    /// with the source's feature state applied.
    fn install(&self, tile: &mut Tile, ctx: &mut StoreContext, contents: Option<Contents>) {
        let shown = contents.map(|contents| {
            let mut version = 0;
            if let Some(geometry) = contents.geometry() {
                geometry.set_feature_state(self.feature_state.current_states());
                version = self.feature_state.version();
            }
            Shown::new(contents, version)
        });
        ctx.show(tile, shown);
        tile.loaded = true;
        self.action(tile, ctx, TileOperation::EndParse);
        ctx.changed();
    }

    /// Applies `result` as the reply to `tile`'s job of its current
    /// generation.
    #[cfg(test)]
    pub(crate) fn apply_now(&mut self, tile: &mut Tile, ctx: &mut StoreContext, result: JobResult) -> bool {
        let key = self.tile_key(tile);
        let id = JobId { tile: key, generation: tile.pipeline.generation() };
        let applied = match &mut tile.pipeline {
            Pipeline::Layout(layout) => {
                let running = layout.run_now(key, Arc::clone(ctx.images.available_images()));
                self.apply_layout(tile, ctx, running, result)
            }
            Pipeline::Decode(_) => self.apply_result(tile, ctx, id, result),
        };
        applied == Applied::Installed
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
        if tile.pipeline.lane() == Some(lane) {
            let generation = self.next();
            tile.pipeline.lane_lost(generation);
        }
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
            let lanes: Vec<Lane> = source.slots().filter_map(|slot| slot.tile.pipeline.lane()).collect();
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
                    let running = |tile: &Tile| tile.pipeline.in_flight().is_some_and(|job| job.id == id);
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
                        .filter(|slot| slot.tile.has_work())
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
                slot.tile.dependencies(|deps| used.extend(deps.images.keys().cloned()));
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
