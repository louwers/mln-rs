//! A tile's transitions: how what arrives for a tile, and what its source
//! asks of it, changes its load, its pipeline and what it shows. The load
//! (tile/loading.rs) and the pipeline (tile/pipeline.rs) say what each
//! change means; [`TileMut`] carries it out with the store, and is the one
//! place that settles a tile, submits its jobs, replaces what it shows, and
//! reports its actions, errors and changes.
//!
//! A tile's parse events follow one rule: `StartParse` when the tile becomes
//! pending, `EndParse` when contents install (preceded by `StartParse` if
//! none was reported), and `Cancelled` when a pending tile is reset or let
//! go of.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mln_storage::response::Reason;
use mln_storage::{Request, Response};
use mln_style::custom_tile_loader::SubscriberKey;
use mln_style::sources::geojson::GeoJSONData;
use mln_util::error::Error;
use mln_util::hash::HashSet;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::render_orchestrator::drop_on_workers;
use crate::tile::contents::{Contents, GeometryContents};
use crate::tile::inbox::{SourceData, TileEvent};
use crate::tile::job::{DataKey, JobId, JobResult, LayoutOutput, Run, SourceStyle};
use crate::tile::loading::{self, CustomLoader, DataOrigin, Method, NecessityStep, OriginLoad, Taken};
use crate::tile::pipeline::{DecodeStep, Layout, LayoutData, LayoutStep, Pipeline, Running};
use crate::tile::store::{SourceInfo, StoreContext};
use crate::tile::tile::{Shown, Tile, TileInput, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_workers::{InlineWorkers, Lane, ReplyBody};

/// A tile, its source and the store, borrowed for a transition. Only
/// [`SourceTiles::update`](crate::tile::source_tiles::SourceTiles::update)
/// and `update_each` make one.
pub(crate) struct TileMut<'a> {
    source: &'a mut SourceInfo,
    tile: &'a mut Tile,
    store: &'a mut StoreContext,
    /// Whether the tile was pending when its parse events last said so.
    reported: bool,
    /// The tile showed new contents.
    installed: bool,
}

fn cant_load_tile_error() -> Error {
    Error::native("Can't load tile.")
}

impl<'a> TileMut<'a> {
    pub(super) fn new(source: &'a mut SourceInfo, tile: &'a mut Tile, store: &'a mut StoreContext) -> Self {
        let reported = tile.is_pending();
        TileMut { source, tile, store, reported, installed: false }
    }

    pub(crate) fn tile(&self) -> &Tile {
        self.tile
    }

    /// Whether the tile showed new contents since the borrow began.
    pub(crate) fn installed(&self) -> bool {
        self.installed
    }

    /// The tile and its source, for a test to reach into.
    #[cfg(test)]
    pub(crate) fn parts(&mut self) -> (&mut SourceInfo, &mut Tile) {
        (self.source, self.tile)
    }

    /// Starts a new tile's load: from the cache, for a tileset's tile; from
    /// the data, for a GeoJSON source's; nothing until it is required, for
    /// a custom source's.
    pub(crate) fn start_load(&mut self) {
        match &self.source.origin {
            Some(DataOrigin::Tileset { file_source, .. }) => match file_source {
                None => self.fail(cant_load_tile_error()),
                // When supported, the first request is always optional, even
                // if the tile is required. That way, the first optional
                // request can continue to load when the tile is later
                // changed from required to optional.
                Some(file_source) => {
                    if file_source.supports_cache_only() {
                        self.request(Method::Cache);
                    }
                }
            },
            Some(DataOrigin::GeoJson(data)) => {
                let data = Arc::clone(data);
                self.load_geojson(&data, false);
            }
            Some(DataOrigin::Custom { .. }) | None => {}
        }
        self.report();
    }

    /// Sets how much the tile is needed, and carries out what that asks of
    /// its origin (`Load::set_necessity`).
    pub(crate) fn set_necessity(&mut self, necessity: TileNecessity) {
        match self.tile.load.set_necessity(necessity, self.tile.is_renderable()) {
            NecessityStep::Nothing => {}
            NecessityStep::RequestNetwork => self.request(Method::Network),
            NecessityStep::Subscribe => {
                if self.custom_loader().is_some() {
                    let key = self.source.next();
                    self.tile.load.subscribe(key);
                    self.fetch(key);
                }
            }
            NecessityStep::Fetch(key) => self.fetch(key),
            NecessityStep::Cancel(key) => {
                if let Some((loader, _)) = self.custom_loader() {
                    loader.cancel(self.tile.id, key);
                }
            }
        }
        self.report();
    }

    /// Sets the parameters of the tile's network requests. A network
    /// request under way is made again with them.
    pub(crate) fn set_update_parameters(&mut self, parameters: TileUpdateParameters) {
        if let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin
            && tileset.set_update_parameters(parameters)
        {
            self.request(Method::Network);
        }
        self.report();
    }

    /// Requests a required tile whose last response was an error again, and
    /// makes it wait for the answer.
    pub(crate) fn retry_failed_load(&mut self) {
        if !self.tile.load.retry_failed() {
            return;
        }
        self.restart();
        self.request(Method::Network);
        self.report();
    }

    /// Applies the response to the tile's request `load`; one to another
    /// request does nothing.
    pub(crate) fn apply_response(&mut self, load: u64, res: &Response) {
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        match tileset.answer(load) {
            None => return,
            Some(Method::Network) => self.take_response(Method::Network, res),
            Some(Method::Cache) => {
                // When the cache-only request could not be satisfied, don't
                // treat it as an error. A cache lookup could still return
                // data, _and_ an error, in particular when we were able to
                // find the data, but it is expired and the Cache-Control
                // headers indicated that we aren't allowed to use expired
                // responses. In this case, we still get the data which we
                // can use in our conditional network request.
                let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
                if not_found {
                    tileset.keep_prior(res);
                }
                self.store.changed();
                if !not_found {
                    self.take_response(Method::Cache, res);
                }
                if self.tile.load.wants_network() {
                    self.request(Method::Network);
                }
            }
        }
        self.report();
    }

    /// Applies what a GeoJSON or custom source gave the tile for load
    /// `load`; what it gave another load does nothing.
    pub(crate) fn apply_source_data(&mut self, load: u64, data: SourceData) {
        if !self.tile.load.accepts(load) {
            return;
        }
        match data {
            SourceData::Data(data) => self.take_input(TileInput::Geometry { data, missing: false }),
            SourceData::Error(error) => self.fail(error),
            SourceData::Stale => {
                if self.tile.load.mark_stale() {
                    self.store.changed();
                }
            }
        }
        self.report();
    }

    /// Loads the tile from GeoJSON `data`. With `relayout`, the tile drops
    /// its data and style first, and is pending again once it has new ones.
    /// A tile of a source with synchronous updates, or whose data answers at
    /// once, takes the data now.
    pub(crate) fn load_geojson(&mut self, data: &Arc<dyn GeoJSONData>, relayout: bool) {
        if relayout {
            self.reset();
        }
        let load = self.source.next();
        self.tile.load.start_geojson(load);
        let key = self.source.tile_key(self.tile);
        if let Some(data) = loading::get_geojson_tile(data, key, load, &self.store.inbox, self.source.synchronous) {
            self.take_input(TileInput::Geometry { data: Some(data), missing: false });
        }
        self.report();
    }

    /// Takes in what a load of the tile delivered. A raster tile without an
    /// image shows nothing in its place at once, renderable unless
    /// `missing`.
    pub(crate) fn take_input(&mut self, input: TileInput) {
        match input {
            TileInput::Geometry { data, missing } => {
                let key = DataKey { source: self.source.key, tile: self.tile.id.canonical, load: self.source.next() };
                self.change_layout(|layout, generation| layout.set_data(LayoutData { key, data, missing }, generation));
            }
            TileInput::Image { data, missing } => {
                let has_image = data.is_some();
                let generation = self.source.next();
                let Pipeline::Decode(decode) = &mut self.tile.pipeline else {
                    return;
                };
                decode.set_image(data, generation);
                if !has_image {
                    self.install((!missing).then_some(Contents::Empty));
                }
            }
        }
        self.report();
    }

    /// The tile couldn't load or lay out: it keeps what it shows, and is
    /// done for now.
    pub(crate) fn fail(&mut self, error: Error) {
        self.tile.settled = true;
        self.action(TileOperation::Error);
        self.store.error(&self.source.id, self.tile.id, error);
        self.report();
    }

    /// Lays the tile out with `style`, for `pixel_ratio` and `granularity`.
    pub(crate) fn set_style(
        &mut self,
        style: &Arc<SourceStyle>,
        pixel_ratio: f32,
        granularity: SubdivisionGranularitySetting,
    ) {
        self.change_layout(|layout, generation| {
            layout.set_style(Arc::clone(style), pixel_ratio, granularity, generation);
        });
        self.report();
    }

    pub(crate) fn set_show_collision_boxes(&mut self, show: bool) {
        if self.tile.pipeline.layout().is_none_or(|layout| layout.params().show_collision_boxes == show) {
            return;
        }
        self.change_layout(|layout, generation| layout.set_show_collision_boxes(show, generation));
        self.report();
    }

    /// Takes in a change of images: the ones in `changed` were added,
    /// removed or resized, and `available` exist now. Lays the tile out
    /// again if the images it shows name one of `changed`, or if its work
    /// does (`Layout::touches_images`).
    pub(crate) fn images_changed(&mut self, changed: &HashSet<String>, available: &VecSet<String>) {
        let shown = self
            .tile
            .contents()
            .and_then(Contents::geometry)
            .is_some_and(|contents| contents.deps.images.keys().any(|id| changed.contains(id)));
        let work = self.tile.pipeline.layout().is_some_and(|layout| layout.touches_images(changed, available));
        if shown || work {
            self.relayout();
        }
        self.report();
    }

    /// Lays the tile out again if its layout, or the one it runs, lacked
    /// glyphs of a range that failed to load, which asks for the range
    /// again.
    pub(crate) fn retry_failed_glyphs(&mut self) {
        let running = self.tile.pipeline.layout().is_some_and(Layout::lacks_failed_glyphs);
        let shown =
            self.tile.contents().and_then(Contents::geometry).is_some_and(|contents| contents.answer.glyphs_failed);
        if running || shown {
            self.relayout();
        }
        self.report();
    }

    /// Moves the tile's work on: resolves the dependencies it waits for,
    /// once the stores have them all or know them missing, and starts its
    /// next job if a lane has room, or at once for a synchronous source.
    pub(crate) fn drive(&mut self) {
        if let Some(layout) = self.tile.pipeline.layout_mut() {
            let store = &mut *self.store;
            layout.resolve(|waiting| store.resolve(waiting));
        }
        if self.source.synchronous {
            self.run_inline();
        } else if let Some((lane, run)) = self.next_job(false) {
            self.store.submit(lane, run);
        }
        self.report();
    }

    /// Applies the reply to the tile's job `id`, which `lane` ran.
    pub(crate) fn apply_reply(&mut self, lane: Lane, id: JobId, body: ReplyBody) {
        if id.generation != self.tile.pipeline.generation() {
            // The tile lays out or decodes its newest generation instead.
            self.tile.pipeline.stale_replied();
            drop_on_workers(&self.store.pool, body);
        } else if let Some(result) = self.store.jobs.decode(lane, body) {
            self.apply_result(id, result);
        }
        // Else a reply of a worker lost since: the loss, which comes next,
        // runs the job again.
        self.report();
    }

    /// The worker of `lane` is gone: the tile's job there, or the state its
    /// worker kept for it, goes, and its work starts again.
    pub(crate) fn lane_lost(&mut self, lane: Lane) {
        if self.tile.pipeline.lane() == Some(lane) {
            let generation = self.source.next();
            self.tile.pipeline.lane_lost(generation);
        }
        self.report();
    }

    /// Applies `result` as the reply to the tile's job of its current
    /// generation.
    #[cfg(test)]
    pub(crate) fn apply_now(&mut self, result: JobResult) {
        let key = self.source.tile_key(self.tile);
        let id = JobId { tile: key, generation: self.tile.pipeline.generation() };
        let images = Arc::clone(self.store.images.available_images());
        match &mut self.tile.pipeline {
            Pipeline::Layout(layout) => {
                let running = layout.run_now(key, images);
                self.apply_layout(running, result);
            }
            Pipeline::Decode(_) => self.apply_result(id, result),
        }
        self.report();
    }

    /// Notifies the tile of a visible layer's updated properties. Returns
    /// whether the tile draws the layer.
    #[cfg(test)]
    pub(crate) fn layer_properties_updated(
        &mut self,
        layer_properties: &Arc<dyn crate::layer_type::ErasedEvaluatedLayer>,
    ) -> bool {
        self.tile.layer_properties_updated(layer_properties)
    }

    fn action(&mut self, op: TileOperation) {
        self.store.action(&self.source.id, self.tile.id, op);
    }

    /// Emits `StartParse` if the tile became pending since it was last
    /// reported so.
    fn report(&mut self) {
        let pending = self.tile.is_pending();
        if pending && !self.reported {
            self.action(TileOperation::StartParse);
        }
        self.reported = pending;
    }

    /// Shows `contents`, or nothing, as the tile's newest: geometry contents
    /// with the source's feature state applied.
    fn install(&mut self, contents: Option<Contents>) {
        let shown = contents.map(|contents| {
            let mut version = 0;
            if let Some(geometry) = contents.geometry() {
                geometry.set_feature_state(self.source.feature_state.current_states());
                version = self.source.feature_state.version();
            }
            Shown::new(contents, version)
        });
        if let Some(old) = std::mem::replace(&mut self.tile.shown, shown) {
            self.store.release(old.contents);
        }
        self.tile.settled = true;
        // A parse that nothing reported started, such as of a raster tile
        // without an image, starts and ends at once.
        if !self.reported {
            self.action(TileOperation::StartParse);
        }
        self.action(TileOperation::EndParse);
        self.reported = self.tile.is_pending();
        self.installed = true;
        self.store.changed();
    }

    /// The tile's load starts again: it hasn't done what it can.
    fn restart(&mut self) {
        self.tile.settled = false;
    }

    /// Drops the tile's data and style, and leaves it waiting for new ones.
    fn reset(&mut self) {
        // A parse in progress is cancelled.
        if self.tile.is_pending() {
            self.action(TileOperation::Cancelled);
        }
        self.reported = false;
        self.restart();
        let generation = self.source.next();
        if let Some(layout) = self.tile.pipeline.layout_mut() {
            layout.reset(generation);
        }
    }

    /// Changes an input of a geometry tile's layout through `f`, at a new
    /// generation, and runs a synchronous source's ready jobs. A raster
    /// tile has no layout: `f` doesn't run.
    fn change_layout(&mut self, f: impl FnOnce(&mut Layout, u64)) {
        let generation = self.source.next();
        let Some(layout) = self.tile.pipeline.layout_mut() else {
            return;
        };
        f(layout, generation);
        self.advance();
    }

    /// Lays the tile out again, with its data and style, for glyphs or
    /// images that changed: without the answer the layout it shows had.
    fn relayout(&mut self) {
        if let Some(shown) = &mut self.tile.shown {
            shown.answer_outdated = true;
        }
        self.change_layout(Layout::bump);
    }

    /// Runs a synchronous source's ready jobs at once.
    fn advance(&mut self) {
        if self.source.synchronous {
            self.run_inline();
        }
    }

    /// Runs the tile's jobs on this thread until one waits for dependencies
    /// or the tile shows its contents.
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

    /// Puts the tile's next job in flight, and returns it with its lane;
    /// `None` if the tile has no job to run, lacks its data or style, or no
    /// lane has room. `inline` runs it on this thread.
    fn next_job(&mut self, inline: bool) -> Option<(Lane, Run)> {
        let key = self.source.tile_key(self.tile);
        match &mut self.tile.pipeline {
            Pipeline::Layout(layout) => {
                let images = self.store.images.available_images();
                let jobs = &mut self.store.jobs;
                layout.next_job(key, self.tile.shown.as_ref(), images, |data, full| {
                    if inline { Some((Lane(0), false)) } else { jobs.place(data, full) }
                })
            }
            Pipeline::Decode(decode) => {
                let jobs = &mut self.store.jobs;
                decode.next_job(key, self.source.kind, || if inline { Some(Lane(0)) } else { jobs.lane() })
            }
        }
    }

    /// Applies `result`, the reply to the tile's job `id`, which is in
    /// flight.
    fn apply_result(&mut self, id: JobId, result: JobResult) {
        match &mut self.tile.pipeline {
            Pipeline::Decode(decode) => match decode.apply(id, result) {
                DecodeStep::Stale => {}
                DecodeStep::Show(contents) => self.install(Some(contents)),
                DecodeStep::Failed(error) => self.fail(error),
            },
            Pipeline::Layout(layout) => {
                if let Some(running) = layout.take_running() {
                    self.apply_layout(running, result);
                }
            }
        }
    }

    /// Applies `result`, the reply to `running`, the tile's layout job.
    fn apply_layout(&mut self, running: Running, result: JobResult) {
        let Some(layout) = self.tile.pipeline.layout_mut() else {
            return;
        };
        match layout.apply(running, result, self.store.images.available_images()) {
            LayoutStep::Show { output, answer, first_try } => {
                // A resume's answer is what it asked for; a first try's may be
                // the layout before's, which can hold more.
                let answer = if first_try {
                    answer.trimmed(&output.deps, &mut self.store.atlas).unwrap_or(answer)
                } else {
                    answer
                };
                let LayoutOutput { mut result, deps } = output;
                let (data, missing) = layout.data().map_or((None, false), |data| (data.data.clone(), data.missing));
                // The index crossed without its data, or with the data it was
                // built from, which is the tile's.
                if let Some(feature_index) = Arc::get_mut(&mut result.feature_index) {
                    feature_index.set_data(data);
                }
                let contents = GeometryContents::new(result, missing, answer, deps);
                self.install(Some(Contents::Geometry(contents)));
            }
            LayoutStep::Wait => {}
            LayoutStep::Resend(key) => {
                // The lane that last got the data lost it: the next job
                // carries it.
                self.store.jobs.forget(key);
            }
            LayoutStep::Outdated { images_moved, result } => {
                if images_moved {
                    let generation = self.source.next();
                    if let Some(layout) = self.tile.pipeline.layout_mut() {
                        layout.bump(generation);
                    }
                    if let Some(shown) = &mut self.tile.shown {
                        shown.answer_outdated = true;
                    }
                }
                drop_on_workers(&self.store.pool, result);
            }
            LayoutStep::Failed(error) => self.fail(error),
        }
    }

    /// Requests the tile's resource with `method`, as a new load whose
    /// response goes to the inbox.
    fn request(&mut self, method: Method) {
        let Some(file_source) = self.source.file_source() else {
            self.fail(cant_load_tile_error());
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
        // One response per request: a reload is a new request.
        let (request, responder) =
            Request::direct_once(move |response| inbox.send(TileEvent::Response { tile: key, load, response }));
        file_source.load(tileset.resource_for(method), responder);
        tileset.begin(method, load, request);
    }

    /// Takes in response `res` to the tile's request with `method`, which
    /// answered: its error fails the load, and its data goes to the tile.
    fn take_response(&mut self, method: Method, res: &Response) {
        let OriginLoad::Tileset(tileset) = &mut self.tile.load.origin else {
            return;
        };
        let loaded = match method {
            Method::Network => TileOperation::LoadFromNetwork,
            Method::Cache => TileOperation::LoadFromCache,
        };
        match tileset.take(res) {
            Taken::Failed(error) => self.fail(error),
            // A `not_modified` response leaves the tile with the data it has.
            Taken::NotModified => self.action(loaded),
            Taken::Data => {
                self.action(loaded);
                if let Some(origin) = &self.source.origin {
                    let input = origin.input(self.source.kind, res);
                    self.take_input(input);
                }
            }
        }
    }

    /// The source's custom loader, and whether its MLT tiles decode with
    /// `fast_pfor_enabled`.
    fn custom_loader(&self) -> Option<(Arc<dyn CustomLoader>, bool)> {
        match &self.source.origin {
            Some(DataOrigin::Custom { loader, fast_pfor_enabled }) => Some((Arc::clone(loader), *fast_pfor_enabled)),
            Some(DataOrigin::Tileset { .. } | DataOrigin::GeoJson(_)) | None => None,
        }
    }

    /// Asks the custom loader for the tile's data, under subscription `key`.
    fn fetch(&mut self, key: SubscriberKey) {
        if let Some((loader, fast_pfor_enabled)) = self.custom_loader() {
            let tile_key = self.source.tile_key(self.tile);
            loader.fetch(self.tile.id, key, tile_key, &self.store.inbox, fast_pfor_enabled);
        }
    }
}

/// Lets go of `tile`, which leaves `source`: a parse in progress is
/// cancelled, a job in flight needn't run, a custom loader forgets it, its
/// request is cancelled as it drops, and its contents are freed.
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
        store.release(shown.contents);
    }
}
