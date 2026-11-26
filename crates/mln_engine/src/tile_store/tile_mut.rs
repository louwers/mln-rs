//! A tile's transitions. [`TileMut`] is the one place that settles a tile,
//! submits its jobs, replaces what it shows, and reports its actions: a
//! `StartParse` when it becomes pending, an `EndParse` when contents install
//! (after a `StartParse` if none was reported), and a `Cancelled` when a
//! pending tile is reset or let go of.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_style::error::Error;
use mln_style::hash::HashSet;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::layer::layer_drawing::ChangedBuckets;
use crate::layer::tile_layout::{Dependencies, LayoutParams};
use crate::log_target;
use crate::render::orchestrator::drop_on_workers;
use crate::source::erased::ErasedOrigin;
use crate::storage::Data;
use crate::text::glyph_store::GlyphRanges;
use crate::tile::TileData;
use crate::tile_store::contents::{Contents, GeometryContents, LayoutResult, release};
use crate::tile_store::dem::DemContents;
use crate::tile_store::loading::TileTimes;
use crate::tile_store::pipeline::{
    DecodeWork, DependencyWait, InFlight, Layout, LayoutData, LayoutWork, Pipeline, Resume, Running, Waiting,
    outdated_by,
};
use crate::tile_store::raster::RasterContents;
use crate::tile_store::store::{SourceInfo, StoreContext};
use crate::tile_store::tile::{Shown, Tile, TileNecessity, TileOperation};
use crate::worker::job::{
    DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceStyle, TileKey,
};
use crate::worker::tile_workers::{InlineWorkers, Lane, ReplyBody};

/// A tile, its source and the store, borrowed for a transition.
pub(crate) struct TileMut<'a> {
    source: &'a mut SourceInfo,
    tile: &'a mut Tile,
    store: &'a mut StoreContext,
    /// Whether the tile was pending when its parse events last said so.
    reported: bool,
    /// The tile showed new contents.
    pub(super) installed: bool,
}

impl<'a> TileMut<'a> {
    /// Runs `f` on `tile`, then reports `StartParse` if `f` left it pending.
    /// Returns `f`'s result and whether the tile showed new contents.
    pub(super) fn with<R>(
        source: &'a mut SourceInfo,
        tile: &'a mut Tile,
        store: &'a mut StoreContext,
        f: impl FnOnce(&mut TileMut<'a>) -> R,
    ) -> (R, bool) {
        let reported = tile.is_pending();
        let mut tile = TileMut { source, tile, store, reported, installed: false };
        let result = f(&mut tile);
        tile.report();
        (result, tile.installed)
    }

    pub(crate) fn tile(&self) -> &Tile {
        self.tile
    }

    #[cfg(test)]
    pub(crate) fn parts(&mut self) -> (&mut SourceInfo, &mut Tile) {
        (self.source, self.tile)
    }

    pub(crate) fn tile_mut(&mut self) -> &mut Tile {
        self.tile
    }

    pub(crate) fn store(&self) -> &StoreContext {
        self.store
    }

    pub(crate) fn store_mut(&mut self) -> &mut StoreContext {
        self.store
    }

    /// The next value of the source's counter.
    pub(crate) fn next(&mut self) -> u64 {
        self.source.next()
    }

    pub(crate) fn key(&self) -> TileKey {
        self.source.tile_key(self.tile)
    }

    /// Whether the tile's jobs run on the render thread as soon as they're
    /// ready.
    pub(crate) fn is_synchronous(&self) -> bool {
        self.source.synchronous
    }

    /// Runs `f` with the origin's state of the tile, out of the tile for
    /// it; nothing for a tile without one.
    pub(crate) fn with_state(&mut self, f: impl FnOnce(&mut dyn Any, &mut Self)) {
        let Some(mut state) = self.tile.load.state.take() else {
            return;
        };
        f(&mut *state, self);
        self.tile.load.state = Some(state);
    }

    /// Runs `f` with the source's origin and its state of the tile, out of
    /// the source and the tile for it; nothing while the origin is out for
    /// its update.
    fn with_origin(&mut self, f: impl FnOnce(&mut dyn ErasedOrigin, &mut dyn Any, &mut Self)) {
        let Some(mut origin) = self.source.origin.take() else {
            return;
        };
        self.with_state(|state, tile| f(&mut *origin, state, tile));
        self.source.origin = Some(origin);
    }

    /// Starts a new tile: from `copy`, of the tile in another world copy,
    /// it shows the copy's contents at once, and its origin may take them
    /// as its answer.
    pub(crate) fn start(&mut self, copy: Option<CopyStart<'_>>) {
        let mut origin_copy = None;
        if let Some(CopyStart { shown, answer, state }) = copy {
            self.tile.shown = Some(shown);
            self.installed = true;
            self.store.changed();
            origin_copy = state.map(|state| (state, answer));
        }
        if let Some(mut origin) = self.source.origin.take() {
            self.tile.load.state = Some(origin.start(origin_copy, self));
            self.source.origin = Some(origin);
        }
    }

    /// Settles a tile that starts from another world copy on the copy's
    /// contents, which it shows, as `answer` says.
    pub(crate) fn take_copy(&mut self, CopyAnswer { data, times }: CopyAnswer) {
        self.tile.settled = true;
        self.tile.load.failed = false;
        self.tile.load.times = times;
        self.tile.load.tried_optional = true;
        if let Some(data) = data {
            self.change_layout(|layout| layout.data = Some(data));
        } else {
            // A raster or raster-dem tile shows all it will: its parse starts
            // and ends at once.
            self.action(TileOperation::StartParse);
            self.action(TileOperation::EndParse);
        }
    }

    /// Sets how much the cover needs the tile, which its origin acts on.
    pub(crate) fn set_necessity(&mut self, necessity: TileNecessity) {
        let previous = std::mem::replace(&mut self.tile.load.necessity, necessity);
        self.with_origin(|origin, state, tile| origin.set_necessity(state, previous, tile));
    }

    /// Retries a required tile's failed loads. A retry that begins a load
    /// or delivers starts the tile over: it hasn't done what it can until
    /// that ends. One that does neither leaves it as it was.
    pub(crate) fn retry_failed_load(&mut self) {
        if self.tile.load.necessity != TileNecessity::Required {
            return;
        }
        let before = (self.tile.load.current, self.tile.pipeline.generation());
        let settled = std::mem::replace(&mut self.tile.settled, false);
        self.with_origin(|origin, state, tile| origin.retry(state, tile));
        if (self.tile.load.current, self.tile.pipeline.generation()) == before {
            self.tile.settled |= settled;
        }
    }

    /// Takes in `reply`, which came for the tile's latest load.
    pub(crate) fn receive(&mut self, reply: Box<dyn Any + Send>) {
        self.with_origin(|origin, state, tile| origin.receive(state, reply, tile));
    }

    /// Takes in a geometry tile's `data`, which a load delivered: `None` for
    /// an empty tile, or, if `missing`, for one the source doesn't have.
    pub(crate) fn take_data(&mut self, data: Option<TileData>, missing: bool) {
        let key = DataKey { source: self.source.key, tile: self.tile.id.canonical, load: self.source.next() };
        self.change_layout(|layout| layout.data = Some(LayoutData { key, data, missing }));
        // A request that follows, such as for the network after the cache's
        // data, comes after the parse it starts.
        self.report();
    }

    /// Takes in a load's answer that the source has no tile here: a tile of
    /// another zoom level draws in its place.
    pub(crate) fn take_missing(&mut self) {
        match self.tile.pipeline {
            Pipeline::Layout(_) => self.take_data(None, true),
            Pipeline::Decode(_) => self.take_image(None, true),
        }
    }

    /// Takes in a raster or raster-dem tile's encoded image, which a load
    /// delivered. A tile without one shows nothing at once, renderable
    /// unless `missing`.
    pub(crate) fn take_image(&mut self, data: Option<Data>, missing: bool) {
        let Pipeline::Decode(decode) = &mut self.tile.pipeline else {
            return;
        };
        let has_image = data.is_some();
        decode.generation = self.source.next();
        decode.work = match std::mem::replace(&mut decode.work, DecodeWork::Current) {
            // A decode in flight goes on, stale, until it replies.
            DecodeWork::Running { job, .. } => {
                job.superseded.store(true, Ordering::Relaxed);
                DecodeWork::Running { job, image: data }
            }
            DecodeWork::Current | DecodeWork::Queued(_) => data.map_or(DecodeWork::Current, DecodeWork::Queued),
        };
        if !has_image {
            self.install((!missing).then_some(Contents::Empty));
        }
        self.report();
    }

    /// A load found the tile's data unchanged: the tile has done what it
    /// can, unless work on that data is under way, which settles it as it
    /// ends.
    pub(crate) fn keep_data(&mut self) {
        if !self.tile.pipeline.is_pending(true) {
            self.tile.settled = true;
        }
    }

    /// The tile couldn't load: it keeps what it shows.
    pub(crate) fn fail(&mut self, error: Error) {
        self.tile.settled = true;
        self.action(TileOperation::Error);
        self.store.error(&self.source.id, self.tile.id, error);
    }

    /// The tile couldn't lay out or decode what its origin delivered: it
    /// keeps what it shows, and the origin hears of it.
    fn fail_work(&mut self, error: Error) {
        self.fail(error);
        self.with_origin(|origin, state, tile| origin.failed(state, tile));
    }

    pub(crate) fn set_style(
        &mut self,
        style: &Arc<SourceStyle>,
        pixel_ratio: f32,
        granularity: SubdivisionGranularitySetting,
    ) {
        self.change_layout(|layout| {
            layout.style = Some(Arc::clone(style));
            layout.params.pixel_ratio = pixel_ratio;
            layout.params.subdivision_granularity = granularity;
        });
    }

    /// Lays the tile out again if the images in `changed` touch what it shows
    /// or its work. A first try's reply tells for itself.
    pub(crate) fn images_changed(&mut self, changed: &HashSet<String>, available: &VecSet<String>) {
        let names = |deps: &Dependencies| deps.images.keys().any(|id| changed.contains(id));
        let shown = self.tile.contents().and_then(Contents::geometry).is_some_and(|contents| names(&contents.deps));
        let work = self.tile.pipeline.layout().is_some_and(|layout| match &layout.work {
            LayoutWork::Running(Running { known: Some(known), .. }) | LayoutWork::Ready(Resume { known, .. }) => {
                names(known)
            }
            LayoutWork::Waiting(Waiting { needs, .. }) => {
                needs.unavailable_images.iter().any(|id| changed.contains(id) && available.contains(id))
                    || needs.images.keys().any(|id| changed.contains(id) && !available.contains(id))
            }
            LayoutWork::Running(_) | LayoutWork::Current | LayoutWork::Outdated => false,
        });
        if shown || work {
            self.relayout();
        }
    }

    /// Lays the tile out again if it lacked glyphs of a range that failed.
    pub(crate) fn retry_failed_glyphs(&mut self) {
        self.relayout_if_lacked(|failed| !failed.is_empty());
    }

    /// Lays the tile out again if it lacked glyphs of a range in
    /// `recovered`, ranges that loaded after failing.
    pub(crate) fn glyphs_recovered(&mut self, recovered: &GlyphRanges) {
        self.relayout_if_lacked(|failed| failed.iter().any(|range| recovered.contains(range)));
    }

    /// Lays the tile out again if `lacked` holds for the failed glyph
    /// ranges of the answer it shows or its work has.
    fn relayout_if_lacked(&mut self, lacked: impl Fn(&GlyphRanges) -> bool) {
        let running = self.tile.pipeline.layout().is_some_and(|layout| match &layout.work {
            LayoutWork::Running(Running { answer, .. }) | LayoutWork::Ready(Resume { answer, .. }) => {
                lacked(&answer.failed_glyphs)
            }
            LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Waiting(_) => false,
        });
        let shown = (self.tile.contents().and_then(Contents::geometry))
            .is_some_and(|contents| lacked(&contents.answer.failed_glyphs));
        if running || shown {
            self.relayout();
        }
    }

    /// Resolves the dependencies the tile waits for, and starts its next job.
    pub(crate) fn drive(&mut self) {
        if let Pipeline::Layout(layout) = &mut self.tile.pipeline
            && let LayoutWork::Waiting(waiting) = &mut layout.work
            && let Some(resume) = self.store.resolve(waiting)
        {
            layout.work = LayoutWork::Ready(resume);
        }
        if self.source.synchronous {
            self.run_inline();
        } else if let Some((lane, run)) = self.next_job(false) {
            self.store.jobs.submit(lane, run, &self.store.inbox);
        }
    }

    pub(crate) fn apply_reply(&mut self, lane: Lane, id: JobId, body: ReplyBody) {
        if id.generation != self.tile.pipeline.generation() {
            // The work goes on with the current generation.
            match &mut self.tile.pipeline {
                Pipeline::Layout(layout) => layout.work = LayoutWork::Outdated,
                Pipeline::Decode(decode) => decode.requeue(),
            }
            drop_on_workers(&self.store.pool, body);
        } else if let Some(result) = self.store.jobs.decode(lane, body) {
            self.apply_result(id, result);
        }
        // Else a reply of a worker lost since: the loss, which comes next,
        // runs the job again.
    }

    /// The worker of `lane` is gone: the work it held starts again.
    pub(crate) fn lane_lost(&mut self, lane: Lane) {
        if self.tile.pipeline.lane() != Some(lane) {
            return;
        }
        let generation = self.source.next();
        match &mut self.tile.pipeline {
            Pipeline::Layout(layout) => {
                layout.bump(generation);
                layout.work = LayoutWork::Outdated;
            }
            Pipeline::Decode(decode) => {
                decode.generation = generation;
                decode.requeue();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn apply_decode_now(&mut self, result: JobResult) {
        let id = JobId { tile: self.source.tile_key(self.tile), generation: self.tile.pipeline.generation() };
        assert!(matches!(self.tile.pipeline, Pipeline::Decode(_)), "a raster or raster-dem tile");
        self.apply_result(id, result);
    }

    pub(crate) fn action(&mut self, op: TileOperation) {
        self.store.action(&self.source.id, self.tile.id, op);
    }

    /// Emits `StartParse` if the tile became pending since it last said so.
    fn report(&mut self) {
        let pending = self.tile.is_pending();
        if pending && !self.reported {
            self.action(TileOperation::StartParse);
        }
        self.reported = pending;
    }

    /// Shows `contents`, with the source's feature state applied.
    fn install(&mut self, contents: Option<Contents>) {
        let shown = contents.map(|mut contents| {
            let mut version = 0;
            if let Some(geometry) = contents.geometry_mut() {
                // New buckets, which no draws copied yet.
                let mut changed = ChangedBuckets::default();
                geometry.set_feature_state(self.source.feature_state.current_states(), &mut changed);
                version = self.source.feature_state.version();
            }
            Shown::new(contents, version)
        });
        if let Some(old) = std::mem::replace(&mut self.tile.shown, shown) {
            release(old.contents, &self.store.pool);
        }
        self.tile.settled = true;
        // A parse nothing reported, such as of a raster tile without an
        // image, starts and ends at once.
        if !self.reported {
            self.action(TileOperation::StartParse);
        }
        self.action(TileOperation::EndParse);
        self.reported = self.tile.is_pending();
        self.installed = true;
        self.store.changed();
    }

    /// Drops the tile's data and style, and leaves it waiting for new ones.
    pub(crate) fn reset(&mut self) {
        if self.tile.is_pending() {
            self.action(TileOperation::Cancelled);
        }
        self.reported = false;
        self.tile.settled = false;
        let generation = self.source.next();
        if let Pipeline::Layout(layout) = &mut self.tile.pipeline {
            layout.data = None;
            layout.style = None;
            layout.bump(generation);
        }
    }

    /// Changes a geometry tile's layout through `f`, at a new generation.
    fn change_layout(&mut self, f: impl FnOnce(&mut Layout)) {
        if let Pipeline::Layout(layout) = &mut self.tile.pipeline {
            f(layout);
            layout.bump(self.source.next());
            if self.source.synchronous {
                self.run_inline();
            }
        }
    }

    /// Lays the tile out again, without the answer of the layout it shows.
    fn relayout(&mut self) {
        if let Some(shown) = &mut self.tile.shown {
            shown.answer_outdated = true;
        }
        self.change_layout(|_| {});
    }

    /// Runs the tile's jobs here until it shows contents or waits.
    fn run_inline(&mut self) {
        loop {
            self.report();
            let Some((_, run)) = self.next_job(true) else {
                break;
            };
            let reply = InlineWorkers::run(run);
            self.apply_result(reply.id, reply.result);
        }
    }

    /// Puts the tile's next job in flight, if it has one and a lane has room.
    fn next_job(&mut self, inline: bool) -> Option<(Lane, Run)> {
        let tile = self.source.tile_key(self.tile);
        let superseded = Arc::new(AtomicBool::new(false));
        let in_flight = |id, lane| InFlight { id, lane, superseded: Arc::clone(&superseded) };
        let (lane, id, job) = match &mut self.tile.pipeline {
            Pipeline::Layout(layout) => {
                let layout: &mut Layout = layout;
                let (Some(data), Some(style)) = (&layout.data, &layout.style) else {
                    return None;
                };
                if !matches!(layout.work, LayoutWork::Outdated | LayoutWork::Ready(_)) {
                    return None;
                }
                let (lane, cached) =
                    if inline { (Lane(0), false) } else { self.store.jobs.place(data.key, data.data.is_none())? };
                let shown = self.tile.shown.as_ref();
                let (known, answer, images) = match std::mem::replace(&mut layout.work, LayoutWork::Outdated) {
                    LayoutWork::Ready(Resume { known, answer, images, .. }) => (Some(known), answer, images),
                    // A first try starts with the answer the tile shows,
                    // unless it is outdated.
                    LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Running(_) | LayoutWork::Waiting(_) => {
                        let answer = shown
                            .filter(|shown| !shown.answer_outdated)
                            .and_then(|shown| shown.contents.geometry())
                            .map(|contents| contents.answer.clone())
                            .unwrap_or_default();
                        (None, answer, Arc::clone(self.store.images.available_images()))
                    }
                };
                let input = if cached {
                    DataInput::Cached(data.key)
                } else {
                    DataInput::Full { key: data.key, data: data.data.clone() }
                };
                let job = Job::Layout(LayoutJob {
                    style: Arc::clone(style),
                    available_images: Arc::clone(&images),
                    data: input,
                    // A world copy's contents have none of the layers that
                    // a first load fades in.
                    params: LayoutParams { first_load: shown.is_none_or(|shown| shown.world_copy), ..layout.params },
                    resolved: Arc::clone(&answer.resolved),
                });
                let id = JobId { tile, generation: layout.generation };
                layout.work = LayoutWork::Running(Running { job: in_flight(id, lane), known, answer, images });
                (lane, id, job)
            }
            Pipeline::Decode(decode) => {
                let DecodeWork::Queued(image) = &decode.work else {
                    return None;
                };
                let lane = if inline { Lane(0) } else { self.store.jobs.lane()? };
                let image = Arc::clone(image);
                let data = Arc::clone(&image);
                let job = match decode.dem {
                    Some(encoding) => Job::Dem { data, encoding },
                    None => Job::Raster { data },
                };
                let id = JobId { tile, generation: decode.generation };
                decode.work = DecodeWork::Running { job: in_flight(id, lane), image: Some(image) };
                (lane, id, job)
            }
        };
        Some((lane, Run { id, job, superseded }))
    }

    /// Applies `result`, the reply to job `id` of the current generation.
    fn apply_result(&mut self, id: JobId, result: JobResult) {
        debug_assert_eq!(id.generation, self.tile.pipeline.generation(), "a reply of the current generation");
        let layout = match &mut self.tile.pipeline {
            Pipeline::Layout(layout) => layout,
            Pipeline::Decode(decode) => {
                decode.work = DecodeWork::Current;
                return match result {
                    JobResult::Raster(image) => {
                        self.install(Some(Contents::Raster(RasterContents::new(image))));
                    }
                    JobResult::Dem(dem) => {
                        self.install(Some(Contents::Dem(DemContents::new(dem))));
                    }
                    JobResult::Failed(error) => self.fail_work(error),
                    JobResult::Laid(_) | JobResult::NeedsDeps(_) | JobResult::NeedsData => {
                        self.fail_work(Error::native("a raster tile's decode answered as a layout"));
                    }
                };
            }
        };
        let Running { job, known, answer, images } = match std::mem::replace(&mut layout.work, LayoutWork::Outdated) {
            LayoutWork::Running(running) => running,
            work => {
                layout.work = work;
                return;
            }
        };
        let available = self.store.images.available_images();
        if !Arc::ptr_eq(&images, available) && outdated_by(&result, &images, available) {
            // The reply branched on an image that came or went since: the
            // tile lays out again, without the answer it shows.
            drop_on_workers(&self.store.pool, result);
            layout.bump(self.source.next());
            if let Some(shown) = &mut self.tile.shown {
                shown.answer_outdated = true;
            }
            return;
        }
        match result {
            JobResult::Laid(LayoutOutput { mut result, deps }) => {
                layout.work = LayoutWork::Current;
                // A first try's answer may be the layout before's, which can
                // hold more than this one uses.
                let answer = if known.is_none() {
                    answer.trimmed(&deps, &mut self.store.atlas).unwrap_or(answer)
                } else {
                    answer
                };
                let (data, missing) =
                    layout.data.as_ref().map_or((None, false), |data| (data.data.clone(), data.missing));
                // The index crossed without its data, or with the tile's.
                if let Some(feature_index) = Arc::get_mut(&mut result.feature_index) {
                    feature_index.set_data(data);
                }
                self.install(Some(Contents::Geometry(GeometryContents::new(result, missing, answer, deps))));
            }
            JobResult::NeedsDeps(needs) if known.is_none() => {
                let wait = DependencyWait::default();
                layout.work = LayoutWork::Waiting(Waiting { lane: job.lane, needs, wait, images });
            }
            JobResult::NeedsData => {
                // A resume keeps its answer and generation; a first try runs
                // again with the images as they are then.
                layout.work = match known {
                    Some(known) => LayoutWork::Ready(Resume { lane: None, known, answer, images }),
                    None => LayoutWork::Outdated,
                };
                if let Some(data) = &layout.data {
                    self.store.jobs.forget(data.key);
                }
            }
            JobResult::NeedsDeps(_) => {
                layout.work = LayoutWork::Current;
                log::error!(
                    target: log_target::GENERAL,
                    "Tile {} asked again for the dependencies it was given",
                    job.id.tile.id
                );
                self.fail_work(Error::native("a layout asked again for the dependencies it was given"));
            }
            JobResult::Failed(error) => {
                layout.work = LayoutWork::Current;
                self.fail_work(error);
            }
            JobResult::Raster(_) | JobResult::Dem(_) => {
                layout.work = LayoutWork::Current;
                self.fail_work(Error::native("a geometry tile's layout answered as a decode"));
            }
        }
    }
}

/// What a new tile starts with from the tile in another world copy: what
/// that tile shows, what taking it settles the new tile on, and the origin's
/// state of the copy.
pub(crate) struct CopyStart<'a> {
    shown: Shown,
    answer: Option<CopyAnswer>,
    state: Option<&'a dyn Any>,
}

/// What a tile that takes its copy's contents as its answer settles on:
/// for a geometry tile, the data it lays out itself, and the times of the
/// copy's data.
pub(crate) struct CopyAnswer {
    data: Option<LayoutData>,
    times: TileTimes,
}

impl<'a> CopyStart<'a> {
    /// A copy of `tile`, if it is renderable. A geometry tile's copy draws
    /// its buckets, apart from those of the layers that are placed: these
    /// hold what placement made of them where their tile is. A raster or
    /// raster-dem tile's copy has a bucket of its own, since each is masked
    /// where it is. Its answer is there while its contents are of its latest
    /// data: a geometry tile's data, which the new tile lays out itself, or
    /// a raster or raster-dem tile's image without a newer one to decode.
    pub(crate) fn of(tile: &'a Tile) -> Option<CopyStart<'a>> {
        let shown = tile.shown.as_ref().filter(|shown| shown.contents.is_renderable())?;
        let contents = match &shown.contents {
            Contents::Geometry(contents) => {
                let layer_render_data =
                    contents.result.layer_render_data.copy_where(|layer| !layer.style().info().cross_tile_index);
                Contents::Geometry(GeometryContents {
                    result: LayoutResult::new(layer_render_data, Arc::clone(&contents.result.feature_index)),
                    missing: contents.missing,
                    answer: contents.answer.clone(),
                    deps: Rc::clone(&contents.deps),
                    textures: contents.textures.clone(),
                    bucket_geometry: contents.bucket_geometry.clone(),
                })
            }
            Contents::Raster(raster) => Contents::Raster(RasterContents::with_shared_image(Arc::clone(&raster.image))),
            Contents::Dem(dem) => Contents::Dem(DemContents::new(dem.dem().clone())),
            Contents::Empty => Contents::Empty,
        };
        let times = tile.load.times;
        let answer = match &tile.pipeline {
            Pipeline::Layout(layout) => layout.data.clone().map(|data| CopyAnswer { data: Some(data), times }),
            // An image still to decode is newer than the one shown.
            Pipeline::Decode(_) => (!tile.is_pending()).then_some(CopyAnswer { data: None, times }),
        };
        let world_copy = matches!(contents, Contents::Geometry(_));
        Some(CopyStart {
            shown: Shown {
                contents,
                feature_state_version: shown.feature_state_version,
                answer_outdated: shown.answer_outdated,
                world_copy,
            },
            answer,
            state: tile.load.state.as_deref(),
        })
    }
}

/// Lets go of `tile`: its job needn't run, its origin's state of it drops,
/// which lets go of what it holds, and its contents are freed.
pub(crate) fn retire(source: &SourceInfo, mut tile: Tile, store: &mut StoreContext) {
    if tile.is_pending() {
        store.action(&source.id, tile.id, TileOperation::Cancelled);
    }
    if let Some(job) = tile.pipeline.in_flight() {
        job.superseded.store(true, Ordering::Relaxed);
    }
    drop(tile.load.state.take());
    if let Some(shown) = tile.shown.take() {
        release(shown.contents, &store.pool);
    }
}
