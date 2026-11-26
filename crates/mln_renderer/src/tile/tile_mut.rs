//! A tile's transitions. [`TileMut`] is the one place that settles a tile,
//! submits its jobs, replaces what it shows, and reports its actions: a
//! `StartParse` when it becomes pending, an `EndParse` when contents install
//! (after a `StartParse` if none was reported), and a `Cancelled` when a
//! pending tile is reset or let go of.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::response::Reason;
use mln_storage::{Request, Response};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::error::Error;
use mln_util::hash::{HashSet, hash_bytes};
use mln_vec_map::VecSet;

use crate::layer_drawing::ChangedBuckets;
use crate::layout::tile_layout::{Dependencies, LayoutParams};
use crate::log_target;
use crate::render_orchestrator::drop_on_workers;
use crate::text::glyph_store::GlyphRanges;
use crate::tile::contents::{Contents, GeometryContents, LayoutResult, release};
use crate::tile::custom_tile_loader::SubscriberKey;
use crate::tile::dem::DemContents;
use crate::tile::inbox::{SourceData, TileEvent};
use crate::tile::job::{DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceStyle};
use crate::tile::loading::{
    self, CustomLoad, CustomLoader, DataOrigin, Fetch, Fetched, Method, OriginLoad, TilesetAnswered,
};
use crate::tile::pipeline::{
    DecodeWork, DependencyWait, InFlight, Layout, LayoutData, LayoutWork, Pipeline, Resume, Running, Waiting,
    outdated_by,
};
use crate::tile::raster::RasterContents;
use crate::tile::store::{SourceInfo, StoreContext};
use crate::tile::tile::{Shown, Tile, TileInput, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_workers::{InlineWorkers, Lane, ReplyBody};

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

    /// Starts a new tile's load; a custom source's waits until the tile is
    /// required.
    pub(crate) fn start_load(&mut self) {
        match &self.source.origin {
            Some(DataOrigin::Tileset { file_source: None, .. }) => self.fail(Error::native("Can't load tile.")),
            // The first request only looks in the cache, even for a required
            // tile, so that it can go on if the tile becomes optional.
            Some(DataOrigin::Tileset { file_source: Some(file_source), .. }) => {
                if file_source.supports_cache_only() {
                    self.request(Method::Cache);
                }
            }
            Some(DataOrigin::GeoJson { .. }) => self.load_geojson(false),
            Some(DataOrigin::Custom { .. }) | None => {}
        }
    }

    /// Starts a new tile from `copy`, of the tile in another world copy: it
    /// shows the copy's contents at once. With the copy's load, it takes
    /// that as a cache's answer, and lays out the data for contents of its
    /// own; otherwise it loads as any new tile.
    pub(crate) fn start_as_copy(&mut self, copy: WorldCopy) {
        let WorldCopy { shown, load } = copy;
        self.tile.shown = Some(shown);
        self.installed = true;
        self.store.changed();
        let taken = match (load, &mut self.tile.load.origin) {
            (Some((answered, data)), OriginLoad::Tileset(tileset)) => tileset.take_answered(answered).then_some(data),
            _ => None,
        };
        let Some(data) = taken else {
            self.start_load();
            return;
        };
        self.tile.settled = true;
        if let Some(data) = data {
            self.change_layout(|layout| layout.data = Some(data));
        } else {
            // A raster or raster-dem tile shows all it will: its parse
            // starts and ends at once.
            self.action(TileOperation::StartParse);
            self.action(TileOperation::EndParse);
        }
    }

    /// A required tile asks the network, unless it has data from there that
    /// never expires, or loads from its custom loader; an optional one stops,
    /// unless it is a custom tile that is renderable.
    pub(crate) fn set_necessity(&mut self, necessity: TileNecessity) {
        let unchanged = std::mem::replace(&mut self.tile.load.necessity, necessity) == necessity;
        let required = necessity == TileNecessity::Required;
        let renderable = self.tile.is_renderable();
        match &mut self.tile.load.origin {
            OriginLoad::Tileset(tileset) if !unchanged => {
                if required {
                    if tileset.is_idle() && !tileset.has_lasting_data() {
                        self.request(Method::Network);
                    }
                } else if matches!(tileset.fetch, Fetch::Network { .. }) {
                    // Only a network-only request is aborted, with the
                    // file source's retries and revalidations.
                    tileset.fetch = Fetch::Idle;
                }
            }
            // Fetching a custom tile's data is assumed to be expensive: only
            // required tiles fetch.
            OriginLoad::Custom(custom) if !unchanged || !matches!(custom, CustomLoad::Subscribed(_)) => {
                match (required, *custom) {
                    (true, CustomLoad::Subscribed(key)) => self.fetch(key),
                    (true, CustomLoad::Stale(_) | CustomLoad::Unsubscribed) => {
                        if matches!(self.source.origin, Some(DataOrigin::Custom { .. })) {
                            let key = self.source.next();
                            *custom = CustomLoad::Subscribed(key);
                            self.fetch(key);
                        }
                    }
                    (false, CustomLoad::Subscribed(key) | CustomLoad::Stale(key)) if !renderable => {
                        if let Some((loader, _)) = self.custom_loader() {
                            loader.cancel(self.tile.id, key);
                        }
                    }
                    (false, _) => {}
                }
            }
            OriginLoad::Tileset(_) | OriginLoad::Custom(_) | OriginLoad::GeoJson { .. } => {}
        }
    }

    /// A network request under way is made again with the new parameters.
    pub(crate) fn set_update_parameters(&mut self, parameters: TileUpdateParameters) {
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        if std::mem::replace(&mut tileset.update_parameters, parameters) != parameters
            && matches!(tileset.fetch, Fetch::Network { .. })
        {
            tileset.fetch = Fetch::Idle;
            self.request(Method::Network);
        }
    }

    /// Requests a required tile whose last response was an error again,
    /// now rather than after the file source's backoff.
    pub(crate) fn retry_failed_load(&mut self) {
        if let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin
            && self.tile.load.necessity == TileNecessity::Required
            && tileset.failed
        {
            tileset.fetch = Fetch::Idle;
            self.tile.settled = false;
            self.request(Method::Network);
        }
    }

    /// Applies the response to request `load`, if the tile waits for it.
    pub(crate) fn apply_response(&mut self, load: u64, res: &Response) {
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        let method = match tileset.fetch {
            Fetch::Cache { load: current, .. } if current == load => Method::Cache,
            Fetch::Network { load: current, .. } if current == load => Method::Network,
            Fetch::Idle | Fetch::Cache { .. } | Fetch::Network { .. } => return,
        };
        let now = mln_util::chrono::now();
        if method == Method::Network {
            // The request stays open: the file source answers it again after
            // an error, and when the data expires.
            if let Some(error) = res.error.as_ref().filter(|error| error.reason != Reason::NotFound) {
                // A streak of errors reports once.
                if !std::mem::replace(&mut tileset.failed, true) {
                    self.fail(Error::native(error.message.clone()));
                }
                return;
            }
            tileset.failed = false;
            // A 304 renews the copy the tile held back, if the file source
            // didn't deliver it.
            let delivered = res.not_modified.then(|| tileset.resource.deliver(res.clone()));
            self.take_response(method, delivered.as_ref().unwrap_or(res));
            return;
        }
        // A cache-only request answers once.
        tileset.fetch = Fetch::Idle;
        tileset.cache_answered = true;
        self.store.changed();
        // A cache-only request that found nothing isn't an error. A copy
        // shows while it revalidates unless its caching rules forbid that;
        // then the tile holds it back for the network request to revalidate.
        // An empty copy has nothing to hold back: the network loads anew.
        if res.error.is_none() {
            if res.caching.is_usable(now) {
                tileset.resource.set_prior(res);
                self.take_response(method, res);
            } else if !res.no_content {
                // The network request carries it, so that a 304 delivers it.
                tileset.resource.set_prior(res);
                tileset.resource.prior_data.clone_from(&res.data);
            }
        }
        if self.tile.load.necessity == TileNecessity::Required
            && matches!(&self.tile.load.origin, OriginLoad::Tileset(tileset) if tileset.is_idle())
        {
            self.request(Method::Network);
        }
    }

    /// Applies what a GeoJSON or custom source gave load `load`, if current.
    pub(crate) fn apply_source_data(&mut self, load: u64, data: SourceData) {
        if !self.tile.load.accepts(load) {
            return;
        }
        match data {
            SourceData::Data(data) => self.take_input(TileInput::Geometry { data, missing: false }),
            SourceData::Error(error) => self.fail(error),
            SourceData::Stale => {
                if let OriginLoad::Custom(custom) = &mut self.tile.load.origin {
                    if let CustomLoad::Subscribed(key) = *custom {
                        *custom = CustomLoad::Stale(key);
                    }
                    self.store.changed();
                }
            }
        }
    }

    /// Loads the tile from its source's GeoJSON data: on the workers unless
    /// the source updates synchronously. With `relayout`, the tile first
    /// drops its data and style.
    pub(crate) fn load_geojson(&mut self, relayout: bool) {
        let Some(DataOrigin::GeoJson { data, cuts }) = &self.source.origin else {
            return;
        };
        let (data, cuts) = (Arc::clone(data), (!self.source.synchronous).then(|| cuts.clone()));
        if relayout {
            self.reset();
        }
        let load = self.source.next();
        if let OriginLoad::GeoJson { load: current } = &mut self.tile.load.origin {
            *current = Some(load);
        }
        let key = self.source.tile_key(self.tile);
        if let Some(data) = loading::request_geojson_tile(&data, key, load, &self.store.inbox, cuts.as_ref()) {
            self.take_input(TileInput::Geometry { data: Some(data), missing: false });
        }
    }

    /// Takes in what a load delivered. A raster tile without an image shows
    /// nothing at once, renderable unless `missing`.
    pub(crate) fn take_input(&mut self, input: TileInput) {
        match input {
            TileInput::Geometry { data, missing } => {
                let key = DataKey { source: self.source.key, tile: self.tile.id.canonical, load: self.source.next() };
                self.change_layout(|layout| layout.data = Some(LayoutData { key, data, missing }));
            }
            TileInput::Image { data, missing } => {
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
            }
        }
        // A request that follows, such as for the network after the cache's
        // data, comes after the parse it starts.
        self.report();
    }

    /// The tile couldn't load or lay out: it keeps what it shows.
    pub(crate) fn fail(&mut self, error: Error) {
        self.tile.settled = true;
        // Data that failed isn't fresh: the tile requests it again when
        // required again, and no longer holds it for the tiles cut from it,
        // nor for itself.
        if let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin {
            tileset.from_network = false;
            tileset.held = None;
        }
        self.action(TileOperation::Error);
        self.store.error(&self.source.id, self.tile.id, error);
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

    fn action(&mut self, op: TileOperation) {
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
    fn reset(&mut self) {
        if self.tile.is_pending() {
            self.action(TileOperation::Cancelled);
        }
        self.reported = false;
        self.tile.settled = false;
        // The tile has no data to compare the next with.
        if let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin {
            tileset.data_hash = None;
        }
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
                    JobResult::Failed(error) => self.fail(error),
                    JobResult::Laid(_) | JobResult::NeedsDeps(_) | JobResult::NeedsData => {
                        self.fail(Error::native("a raster tile's decode answered as a layout"));
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
                self.fail(Error::native("a layout asked again for the dependencies it was given"));
            }
            JobResult::Failed(error) => {
                layout.work = LayoutWork::Current;
                self.fail(error);
            }
            JobResult::Raster(_) | JobResult::Dem(_) => {
                layout.work = LayoutWork::Current;
                self.fail(Error::native("a geometry tile's layout answered as a decode"));
            }
        }
    }

    fn request(&mut self, mut method: Method) {
        let fresh =
            matches!(&self.tile.load.origin, OriginLoad::Tileset(tileset) if tileset.is_fresh(mln_util::chrono::now()));
        if !fresh && self.take_in_memory() {
            // A required tile whose data expires still asks the network,
            // which the file source holds until the data expires.
            let lasting = matches!(&self.tile.load.origin, OriginLoad::Tileset(tileset) if tileset.has_lasting_data());
            if self.tile.load.necessity != TileNecessity::Required || lasting {
                return;
            }
            method = Method::Network;
        }
        let Some(file_source) = self.source.file_source() else {
            self.fail(Error::native("Can't load tile."));
            return;
        };
        self.action(match method {
            Method::Cache => TileOperation::RequestedFromCache,
            Method::Network => TileOperation::RequestedFromNetwork,
        });
        let load = self.source.next();
        let key = self.source.tile_key(self.tile);
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        let inbox = self.store.inbox.clone();
        // A cache-only request answers once; a network request stays open
        // while the tile is required.
        let (request, responder) = match method {
            Method::Cache => {
                Request::once(move |response| inbox.send(TileEvent::Response { tile: key, load, response }))
            }
            Method::Network => {
                Request::new(move |response| inbox.send(TileEvent::Response { tile: key, load, response }))
            }
        };
        file_source.load(tileset.resource_for(method), responder);
        debug_assert!(tileset.is_idle(), "one request at a time");
        // A new request's errors are a new streak, which fails the tile.
        tileset.failed = false;
        tileset.fetch = match method {
            Method::Cache => Fetch::Cache { load, request },
            Method::Network => Fetch::Network { load, request },
        };
    }

    /// A tile whose data at its tileset's last zoom another tile's load
    /// brought fresh from the network, whether a tile cut from it or the
    /// tile itself, takes its data from that, as that load's answer, instead
    /// of loading it again.
    fn take_in_memory(&mut self) -> bool {
        let (Some(origin), OriginLoad::Tileset(tileset)) = (&mut self.source.origin, &mut self.tile.load.origin) else {
            return false;
        };
        let now = mln_util::chrono::now();
        let Some((data, fetched, held)) = origin.in_memory(&self.tile.id, &tileset.resource.url, now) else {
            return false;
        };
        let Fetched { modified, caching, etag } = fetched;
        let resource = &mut tileset.resource;
        (resource.prior_modified, resource.prior_etag) = (modified, etag);
        resource.prior_caching = caching;
        tileset.took(modified, caching, true);
        tileset.data_hash = None;
        tileset.held = held;
        self.take_input(TileInput::Geometry { data: Some(data), missing: false });
        true
    }

    /// Takes a copy of the tile's data that `method` brought, which caching
    /// rules allow it to show: new data, or, for a 304, the data it has. The
    /// same bytes as the data it has count as a 304.
    fn take_response(&mut self, method: Method, res: &Response) {
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        if res.not_modified {
            tileset.resource.prior_caching = res.caching;
        } else {
            tileset.resource.set_prior(res);
        }
        tileset.took(res.modified, res.caching, method == Method::Network);
        let hash = res.data.as_deref().filter(|_| !res.no_content).map(|data| hash_bytes(data));
        let unchanged = res.not_modified || hash.is_some_and(|hash| tileset.data_hash == Some(hash));
        if !unchanged {
            tileset.data_hash = hash;
        }
        self.action(match method {
            Method::Network => TileOperation::LoadFromNetwork,
            Method::Cache => TileOperation::LoadFromCache,
        });
        if unchanged {
            // The tile keeps its data: it has done what it can, unless work
            // on that data is under way, which settles it as it ends.
            if !self.tile.pipeline.is_pending(true) {
                self.tile.settled = true;
            }
        } else if let (Some(origin), OriginLoad::Tileset(tileset)) =
            (&mut self.source.origin, &mut self.tile.load.origin)
        {
            let network = method == Method::Network;
            let (input, held) = origin.input(self.source.kind, &self.tile.id, &tileset.resource.url, res, network);
            tileset.held = held;
            self.take_input(input);
        }
    }

    fn custom_loader(&self) -> Option<(Arc<dyn CustomLoader>, bool)> {
        match &self.source.origin {
            Some(DataOrigin::Custom { loader, fast_pfor_enabled }) => Some((Arc::clone(loader), *fast_pfor_enabled)),
            Some(DataOrigin::Tileset { .. } | DataOrigin::GeoJson { .. }) | None => None,
        }
    }

    fn fetch(&mut self, key: SubscriberKey) {
        if let Some((loader, fast_pfor_enabled)) = self.custom_loader() {
            let tile_key = self.source.tile_key(self.tile);
            loader.fetch(self.tile.id, key, tile_key, &self.store.inbox, fast_pfor_enabled);
        }
    }
}

/// What a new tile starts with from the tile in another world copy: what
/// that tile shows and, for a tileset tile whose contents are of its latest
/// data, its load: what its requests know, and the data a geometry tile lays
/// out.
pub(crate) struct WorldCopy {
    shown: Shown,
    load: Option<(TilesetAnswered, Option<LayoutData>)>,
}

impl WorldCopy {
    /// A copy of `tile`, if it is renderable. A geometry tile's copy draws
    /// its buckets, apart from those of the layers that are placed: these
    /// hold what placement made of them where their tile is. A raster or
    /// raster-dem tile's copy has a bucket of its own, since each is masked
    /// where it is.
    pub(crate) fn of(tile: &Tile) -> Option<WorldCopy> {
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
        let load = match (&tile.load.origin, &tile.pipeline) {
            // The copy lays the latest data out itself.
            (OriginLoad::Tileset(tileset), Pipeline::Layout(layout)) => {
                layout.data.clone().map(|data| (tileset.answered(), Some(data)))
            }
            // An image still to decode is newer than the one shown.
            (OriginLoad::Tileset(tileset), Pipeline::Decode(_)) => {
                (!tile.is_pending()).then(|| (tileset.answered(), None))
            }
            (OriginLoad::GeoJson { .. } | OriginLoad::Custom(_), _) => None,
        };
        let world_copy = matches!(contents, Contents::Geometry(_));
        Some(WorldCopy {
            shown: Shown {
                contents,
                feature_state_version: shown.feature_state_version,
                answer_outdated: shown.answer_outdated,
                world_copy,
            },
            load,
        })
    }
}

/// Lets go of `tile`: its job needn't run, its custom loader forgets it, its
/// request drops and its contents are freed.
pub(crate) fn retire(source: &SourceInfo, mut tile: Tile, store: &mut StoreContext) {
    if tile.is_pending() {
        store.action(&source.id, tile.id, TileOperation::Cancelled);
    }
    if let Some(job) = tile.pipeline.in_flight() {
        job.superseded.store(true, Ordering::Relaxed);
    }
    if let (Some(key), Some(DataOrigin::Custom { loader, .. })) = (tile.load.subscription(), &source.origin) {
        loader.remove(tile.id, key);
    }
    if let Some(shown) = tile.shown.take() {
        release(shown.contents, &store.pool);
    }
}
