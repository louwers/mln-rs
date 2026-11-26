// The downloads of a database's offline regions live with the database, on
// the thread that owns both (the database's worker): `Downloads` keeps one
// `OfflineDownload` per region, which works on the database it's handed.
// What a download sets off comes back as a task on that thread (`Post`),
// which names the region and the request: a request's response, or the
// queued check of a resource the region may hold. A request dropped from the
// download, as a pause drops them all, finds nothing when its task comes.
//
// The observer is called while the download works; it reaches the download
// again only through a task.

use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use mln_renderer::util::tile_cover::{TileCover, covering_zoom_level, tile_count_bounds, tile_count_geometry};
use mln_storage::offline::{
    OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionObserver, OfflineRegionStatus,
};
use mln_storage::offline_database::{DatabaseError, OfflineDatabase};
use mln_storage::resource::{Priority, TileFormat, Usage};
use mln_storage::response::{Caching, Reason, ResourceError};
use mln_storage::{FileSource, Kind, LoadingMethod, Request, Resource, Response};
use mln_style::i18n::glyph_range_is_entirely_locally_generated;
use mln_style::layer::SourceKind;
use mln_style::source::{OfflineResources, SourceTypeInfo};
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Parser, StyleRegistry};
use mln_text::glyph::{
    GLYPH_RANGES_PER_FONT_STACK, GLYPHS_PER_GLYPH_RANGE, GlyphID, NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK,
    get_glyph_range,
};
use mln_util::chrono::now;
use mln_util::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS;
use mln_util::hash::{HashMap, HashSet};
use mln_util::range::Range;
use mln_util::tileset::{Tileset, VectorEncoding};
use mln_vec_map::VecMap;

const K_RESOURCES_BATCH_SIZE: usize = 64;
const K_MARK_BATCH_SIZE: usize = 200;

// Generic functions

fn covering_zoom_range(
    definition: &OfflineRegionDefinition,
    round_zoom: bool,
    tile_size: u16,
    zoom_range: Range<u8>,
) -> Range<u8> {
    // `std::max<double>` / `std::min<double>`.
    let a = f64::from(covering_zoom_level(definition.min_zoom(), round_zoom, tile_size));
    let b = f64::from(zoom_range.min);
    let min_z = if a < b { b } else { a };
    let a = f64::from(covering_zoom_level(definition.max_zoom(), round_zoom, tile_size));
    let b = f64::from(zoom_range.max);
    let max_z = if b < a { b } else { a };

    debug_assert!(min_z >= 0.0);
    debug_assert!(max_z >= 0.0);
    debug_assert!(min_z < f64::from(u8::MAX));
    debug_assert!(max_z < f64::from(u8::MAX));
    Range::new(min_z as u8, max_z as u8)
}

fn tile_cover(
    definition: &OfflineRegionDefinition,
    round_zoom: bool,
    tile_size: u16,
    zoom_range: Range<u8>,
    mut f: impl FnMut(CanonicalTileID),
) {
    let clamped_zoom_range = covering_zoom_range(definition, round_zoom, tile_size, zoom_range);

    for z in clamped_zoom_range.min..=clamped_zoom_range.max {
        let cover = match definition {
            OfflineRegionDefinition::TilePyramid(reg) => Some(TileCover::new_from_bounds(&reg.bounds, z)),
            OfflineRegionDefinition::Geometry(reg) => TileCover::new_from_geometry(&reg.geometry, z).ok(),
        };
        if let Some(mut cover) = cover {
            while cover.has_next() {
                if let Some(tile) = cover.next() {
                    f(tile.canonical);
                }
            }
        }
    }
}

fn tile_count(definition: &OfflineRegionDefinition, round_zoom: bool, tile_size: u16, zoom_range: Range<u8>) -> u64 {
    let clamped_zoom_range = covering_zoom_range(definition, round_zoom, tile_size, zoom_range);

    let mut result: u64 = 0;
    for z in clamped_zoom_range.min..=clamped_zoom_range.max {
        result += match definition {
            OfflineRegionDefinition::TilePyramid(reg) => tile_count_bounds(&reg.bounds, z),
            OfflineRegionDefinition::Geometry(reg) => tile_count_geometry(&reg.geometry, z).unwrap_or(0),
        };
    }

    result
}

/// A task for the thread that owns a database and its downloads.
pub type DownloadTask = Box<dyn FnOnce(&mut OfflineDatabase, &mut Downloads) + Send>;

/// Hands a task to the thread that owns the database and its downloads,
/// which runs it after what it runs now. Any thread may call it.
pub type Post = Arc<dyn Fn(DownloadTask) + Send + Sync>;

/// The downloads of a database's offline regions, by region id, on the
/// thread that owns the database.
pub struct Downloads {
    downloads: HashMap<i64, OfflineDownload>,
    network: Option<Arc<dyn FileSource>>,
    post: Post,
    registry: Arc<StyleRegistry>,
}

impl Downloads {
    /// Downloads that load through `network`, come back through `post`, and
    /// read their styles against `registry`.
    #[must_use]
    pub fn new(network: Option<Arc<dyn FileSource>>, post: Post, registry: Arc<StyleRegistry>) -> Self {
        Downloads { downloads: HashMap::default(), network, post, registry }
    }

    /// The download of the region `region_id`, made at its first use. Fails
    /// without a network, and for a region that the database doesn't have.
    pub fn download(
        &mut self,
        db: &mut OfflineDatabase,
        region_id: i64,
    ) -> Result<&mut OfflineDownload, DatabaseError> {
        match self.downloads.entry(region_id) {
            Entry::Occupied(download) => Ok(download.into_mut()),
            Entry::Vacant(entry) => {
                let Some(network) = &self.network else {
                    return Err(DatabaseError::Runtime("Network file source unavailable.".to_owned()));
                };
                let definition = db.get_region(region_id)?.ok_or(DatabaseError::RegionNotFound(region_id))?.definition;
                let download = OfflineDownload::new(
                    region_id,
                    definition,
                    Arc::clone(network),
                    Arc::clone(&self.post),
                    Arc::clone(&self.registry),
                );
                Ok(entry.insert(download))
            }
        }
    }

    /// Adds a download that was made apart, in place of the region's.
    pub fn insert(&mut self, download: OfflineDownload) {
        self.downloads.insert(download.id, download);
    }

    /// The download of `region_id`, if it has one.
    pub fn get_mut(&mut self, region_id: i64) -> Option<&mut OfflineDownload> {
        self.downloads.get_mut(&region_id)
    }

    /// Stops and forgets the download of `region_id`.
    pub fn remove(&mut self, region_id: i64) -> Option<OfflineDownload> {
        self.downloads.remove(&region_id)
    }

    /// A response to the request `request` of the region's download.
    fn on_response(&mut self, db: &mut OfflineDatabase, region_id: i64, request: u64, response: Response) {
        if let Some(download) = self.downloads.get_mut(&region_id) {
            download.on_online_response(db, request, response);
        }
    }

    /// The queued check of a resource that the region's download needs.
    fn on_ensure(&mut self, db: &mut OfflineDatabase, region_id: i64, request: u64) {
        if let Some(download) = self.downloads.get_mut(&region_id) {
            download.ensure_resource_task(db, request);
        }
    }
}

/// The default observer.
struct NullObserver;

impl OfflineRegionObserver for NullObserver {}

/// What a downloaded resource lists, which the download goes on to load.
#[derive(Clone)]
enum Then {
    /// Nothing: a resource that lists no others.
    Nothing,
    /// The style, which lists the sources, glyphs and sprites.
    Style,
    /// A source's TileJSON, which lists its tiles.
    Source { url: String, source: mln_style::Source, tile_size: u16 },
}

impl Then {
    fn reads(&self) -> bool {
        !matches!(self, Then::Nothing)
    }
}

/// A resource the download waits for.
enum Pending {
    /// Its check of what the region holds is queued.
    Queued { resource: Resource, then: Then },
    /// It loads from the network, revalidating the stored copy, if any, with
    /// its stored size.
    Loading { _request: Request, resource: Resource, then: Then, stale: Option<(Response, u64)> },
}

/// The download of an offline region's resources into the database.
pub struct OfflineDownload {
    id: i64,
    definition: OfflineRegionDefinition,
    network: Arc<dyn FileSource>,
    post: Post,
    registry: Arc<StyleRegistry>,
    /// The most requests the download has in flight at once.
    maximum_concurrent_requests: u32,
    status: OfflineRegionStatus,
    observer: Box<dyn OfflineRegionObserver>,
    /// The resources it waits for, by request id.
    requests: VecMap<u64, Pending>,
    next_request_id: u64,
    required_source_urls: HashSet<String>,
    resources_remaining: VecDeque<Resource>,
    resources_to_be_marked_as_used: Vec<Resource>,
    buffer: Vec<(Resource, Response)>,
}

impl OfflineDownload {
    /// The download of the region `id`, which loads through `network`, comes
    /// back through `post`, and reads the region's style against `registry`.
    #[must_use]
    pub fn new(
        id: i64,
        definition: OfflineRegionDefinition,
        network: Arc<dyn FileSource>,
        post: Post,
        registry: Arc<StyleRegistry>,
    ) -> Self {
        OfflineDownload {
            id,
            definition,
            network,
            post,
            registry,
            maximum_concurrent_requests: DEFAULT_MAXIMUM_CONCURRENT_REQUESTS,
            status: OfflineRegionStatus::default(),
            observer: Box::new(NullObserver),
            requests: VecMap::new(),
            next_request_id: 0,
            required_source_urls: HashSet::default(),
            resources_remaining: VecDeque::new(),
            resources_to_be_marked_as_used: Vec::new(),
            buffer: Vec::new(),
        }
    }

    /// The region's id.
    #[must_use]
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Sets the most requests the download has in flight at once.
    pub fn set_maximum_concurrent_requests(&mut self, maximum: u32) {
        self.maximum_concurrent_requests = maximum;
    }

    /// Sets the download's observer, or the default one that ignores
    /// everything.
    pub fn set_observer(&mut self, observer: Option<Box<dyn OfflineRegionObserver>>) {
        self.observer = observer.unwrap_or_else(|| Box::new(NullObserver));
    }

    fn notify_status_changed(&self) {
        self.observer.status_changed(self.status);
    }

    fn next_request(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Starts or stops the download.
    pub fn set_state(&mut self, state: OfflineRegionDownloadState) {
        if self.status.download_state == state {
            return;
        }
        self.status.download_state = state;
        if state == OfflineRegionDownloadState::Active {
            self.activate_download();
        } else {
            self.deactivate_download();
        }
        self.notify_status_changed();
    }

    /// The download's status: while it's active, what it found so far, else
    /// what the region holds of what its stored style lists.
    #[must_use]
    pub fn get_status(&self, db: &mut OfflineDatabase) -> OfflineRegionStatus {
        if self.status.download_state == OfflineRegionDownloadState::Active {
            return self.status;
        }

        let Ok(mut result) = db.get_region_completed_status(self.id) else {
            // We can't find this offline region because the database is
            // unavailable, or the download does not exist.
            return OfflineRegionStatus::default();
        };

        result.required_resource_count += 1;
        let style_response = db.get(&Resource::style(self.definition.style_url()));
        let mut parser = Parser::new(&self.registry);
        // A download reports a style it can't read, and the count stays
        // imprecise.
        let Some(style_data) = style_response.and_then(|response| response.data) else {
            return result;
        };
        if parser.parse(&String::from_utf8_lossy(&style_data)).is_err() {
            return result;
        }

        result.required_resource_count_is_precise = true;

        for source in &parser.sources {
            let record = source.clone();
            let round_zoom = record.info().round_zoom;
            let (tileset, tile_size) = match record.data().offline() {
                OfflineResources::None => continue,
                OfflineResources::Source { .. } | OfflineResources::Image { .. } => {
                    result.required_resource_count += 1;
                    continue;
                }
                // A source without tile URLs has no tiles to download.
                OfflineResources::Tiles { tileset, tile_size } => (Some(tileset), tile_size),
                OfflineResources::TileJson { url, tile_size } => {
                    result.required_resource_count += 1;
                    let source_response = db.get(&Resource::source(&url));
                    let tileset = source_response
                        .and_then(|response| response.data)
                        .and_then(|data| record.data().offline_tileset(&data).ok());
                    // As a download does, count no tiles for a TileJSON that
                    // is missing, can't be read, or has no tile URLs, and
                    // leave the count imprecise.
                    if tileset.as_ref().is_none_or(|tileset| tileset.tiles.is_empty()) {
                        result.required_resource_count_is_precise = false;
                    }
                    (tileset, tile_size)
                }
            };
            if let Some(tileset) = tileset.filter(|tileset| !tileset.tiles.is_empty()) {
                let count = tile_count(&self.definition, round_zoom, tile_size, tileset.zoom_range);
                result.required_tile_count += count;
                result.required_resource_count += count;
            }
        }

        if !parser.glyph_url.is_empty() {
            let ttf_count = parser.font_faces.as_ref().map_or(0, |faces| faces.len() as u64); // custom faces
            let per_font_stack = if self.definition.include_ideographs() {
                u64::from(GLYPH_RANGES_PER_FONT_STACK) + ttf_count
            } else {
                u64::from(NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK) + ttf_count
            };
            result.required_resource_count += parser.font_stacks().len() as u64 * per_font_stack;
        }

        if !parser.sprites.is_empty() {
            result.required_resource_count += 4 * parser.sprites.len() as u64;
        }

        result
    }

    fn activate_download(&mut self) {
        self.status = OfflineRegionStatus::default();
        self.status.download_state = OfflineRegionDownloadState::Active;
        self.status.required_resource_count += 1;

        let mut style_resource = Resource::style(self.definition.style_url());
        style_resource.set_priority(Priority::Low);
        style_resource.set_usage(Usage::Offline);
        self.ensure_resource(style_resource, Then::Style);
    }

    /// Reads what a downloaded style or TileJSON lists, and queues it.
    fn read(&mut self, then: &Then, response: &Response) -> Result<(), String> {
        match then {
            Then::Nothing => Ok(()),
            Then::Style => self.on_style_response(response),
            Then::Source { url, source, tile_size } => {
                let Some(data) = &response.data else {
                    return Err("the TileJSON has no content".to_owned());
                };
                let tileset = source.data().offline_tileset(data)?;
                if tileset.tiles.is_empty() {
                    return Err("the TileJSON has no tile URLs".to_owned());
                }
                self.queue_tiles(source.info(), *tile_size, &tileset);
                if self.required_source_urls.remove(url) && self.required_source_urls.is_empty() {
                    self.status.required_resource_count_is_precise = true;
                }
                Ok(())
            }
        }
    }

    /// Queues the resources that the style lists.
    fn on_style_response(&mut self, style_response: &Response) -> Result<(), String> {
        let Some(style_data) = &style_response.data else {
            return Err("the style has no content".to_owned());
        };
        let mut parser = Parser::new(&self.registry);
        parser.parse(&String::from_utf8_lossy(style_data)).map_err(|error| error.to_string())?;
        self.status.required_resource_count_is_precise = true;

        for source in &parser.sources {
            let record = source.clone();
            match record.data().offline() {
                OfflineResources::None => {}
                OfflineResources::Tiles { tileset, tile_size } => {
                    self.queue_tiles(record.info(), tile_size, &tileset);
                }
                OfflineResources::TileJson { url, tile_size } => {
                    self.status.required_resource_count_is_precise = false;
                    self.status.required_resource_count += 1;
                    self.required_source_urls.insert(url.clone());

                    let mut source_resource = Resource::source(&url);
                    source_resource.set_priority(Priority::Low);
                    source_resource.set_usage(Usage::Offline);
                    self.ensure_resource(source_resource, Then::Source { url, source: record, tile_size });
                }
                OfflineResources::Source { url } => self.queue_resource(Resource::source(&url)),
                OfflineResources::Image { url } => {
                    if !url.is_empty() {
                        self.queue_resource(Resource::image(&url));
                    }
                }
            }
        }

        if !parser.glyph_url.is_empty() {
            let include_ideographs = self.definition.include_ideographs();
            for font_stack in parser.font_stacks() {
                for i in 0..GLYPH_RANGES_PER_FONT_STACK {
                    // Only skip a range when every codepoint in it can be
                    // generated locally; some ranges (e.g. 0xFF00) are mixed.
                    let start = (i * GLYPHS_PER_GLYPH_RANGE) as u16;
                    if include_ideographs || !glyph_range_is_entirely_locally_generated(start) {
                        let range = get_glyph_range(GlyphID::from(start));
                        self.queue_resource(Resource::glyphs(
                            &parser.glyph_url,
                            &font_stack,
                            (range.first, range.second),
                        ));
                    }
                    if let Some(faces) = &parser.font_faces {
                        for face in faces.iter() {
                            self.queue_resource(Resource::font_face(&face.url));
                        }
                    }
                }
            }
        }

        if !parser.sprites.is_empty() {
            // Always request 1x and @2x sprite images for portability.
            for sprite in &parser.sprites {
                let sprite_url = &sprite.sprite_url;
                self.queue_resource(Resource::sprite_image(sprite_url, 1.0));
                self.queue_resource(Resource::sprite_image(sprite_url, 2.0));
                self.queue_resource(Resource::sprite_json(sprite_url, 1.0));
                self.queue_resource(Resource::sprite_json(sprite_url, 2.0));
            }
        }
        Ok(())
    }

    // Fill up our own request queue by requesting the next few resources.
    // This is called when activating the download, or when a request
    // completes successfully.
    //
    // Note "successfully"; it's not called when a requests receives an
    // error. A request that errors will be retried after some delay. So in
    // that sense it's still "active" and consuming resources, notably the
    // request object, its timer, and network resources when the timer fires.
    //
    // We could try to squeeze in subsequent requests while we wait for the
    // errored request to retry. But that risks overloading the upstream
    // request queue -- defeating our own metering -- if there are a lot of
    // errored requests that all come up for retry at the same time. And many
    // times, the cause of a request error will apply to many requests of the
    // same type. For instance if a server is unreachable, all the requests
    // to that host are going to error. In that case, continuing to try
    // subsequent resources after the first few errors is fruitless anyway.
    fn continue_download(&mut self, db: &mut OfflineDatabase) {
        if !self.is_active() {
            return;
        }

        if self.resources_remaining.is_empty() {
            // Flush pending buffers.
            self.flush_resources_buffer(db);
            if self.status.complete() {
                self.mark_pending_used_resources(db);
                self.set_state(OfflineRegionDownloadState::Inactive);
                return;
            }
        }

        if self.resources_to_be_marked_as_used.len() >= K_MARK_BATCH_SIZE {
            self.mark_pending_used_resources(db);
        }

        while self.requests.len() < self.maximum_concurrent_requests as usize {
            let Some(resource) = self.resources_remaining.pop_front() else {
                break;
            };
            self.ensure_resource(resource, Then::Nothing);
        }
    }

    fn deactivate_download(&mut self) {
        self.required_source_urls.clear();
        self.resources_remaining.clear();
        self.requests.clear();
        self.buffer.clear();
    }

    fn is_active(&self) -> bool {
        self.status.download_state == OfflineRegionDownloadState::Active
    }

    fn flush_resources_buffer(&mut self, db: &mut OfflineDatabase) {
        if self.buffer.is_empty() {
            return;
        }
        db.put_region_resources(self.id, &self.buffer, &mut self.status);
        self.buffer.clear();
        self.notify_status_changed();
    }

    fn queue_resource(&mut self, mut resource: Resource) {
        resource.set_priority(Priority::Low);
        resource.set_usage(Usage::Offline);
        self.status.required_resource_count += 1;
        if resource.kind == Kind::Tile {
            self.status.required_tile_count += 1;
        }
        self.resources_remaining.push_front(resource);
    }

    /// Queues the tiles of `tileset`, of a source of type `source`, that cover
    /// the region.
    fn queue_tiles(&mut self, source: SourceTypeInfo, tile_size: u16, tileset: &Tileset) {
        // A source without tile URLs has no tiles to download.
        let Some(url_template) = tileset.tiles.first() else {
            return;
        };
        let format = match source.layer_kind {
            SourceKind::Geometry if tileset.vector_encoding == Some(VectorEncoding::MLT) => TileFormat::Mlt,
            SourceKind::Geometry => TileFormat::Mvt,
            SourceKind::None | SourceKind::Raster | SourceKind::RasterDem => TileFormat::Image,
        };
        let (status, remaining) = (&mut self.status, &mut self.resources_remaining);
        let pixel_ratio = self.definition.pixel_ratio();
        tile_cover(&self.definition, source.round_zoom, tile_size, tileset.zoom_range, |tile: CanonicalTileID| {
            status.required_resource_count += 1;
            status.required_tile_count += 1;

            let mut tile_resource = Resource::tile(
                url_template,
                pixel_ratio,
                tile.x as i32,
                tile.y as i32,
                tile.z as i8,
                tileset.scheme,
                format,
                LoadingMethod::All,
            );

            tile_resource.set_priority(Priority::Low);
            tile_resource.set_usage(Usage::Offline);

            remaining.push_back(tile_resource);
        });
    }

    fn mark_pending_used_resources(&mut self, db: &mut OfflineDatabase) {
        db.mark_used_resources(self.id, &self.resources_to_be_marked_as_used);
        self.resources_to_be_marked_as_used.clear();
    }

    /// Queues the check of `resource` against what the region holds, which
    /// loads it unless the region holds a usable copy.
    fn ensure_resource(&mut self, resource: Resource, then: Then) {
        debug_assert_eq!(resource.priority, Priority::Low);
        debug_assert_eq!(resource.usage, Usage::Offline);

        let request = self.next_request();
        self.requests.insert(request, Pending::Queued { resource, then });
        let region = self.id;
        (self.post)(Box::new(move |db, downloads| downloads.on_ensure(db, region, request)));
    }

    /// The queued check of `ensure_resource`.
    fn ensure_resource_task(&mut self, db: &mut OfflineDatabase, request: u64) {
        let Some(Pending::Queued { resource, then }) = self.requests.remove(&request) else {
            // Its download stopped since.
            return;
        };

        // The region's copy is complete while it's fresh. A stale one is
        // revalidated, and stays as it is until its refresh arrives.
        let fresh = |caching: Caching| caching.is_fresh(now());
        let stale = if then.reads() {
            match db.get_region_resource(&resource) {
                Some((response, size)) if fresh(response.caching) => {
                    if let Err(error) = self.read(&then, &response) {
                        self.report_unparsable(&resource, &error);
                    }
                    self.complete_stored_resource(db, &resource, size);
                    return;
                }
                stale => stale,
            }
        } else {
            match db.check_region_resource(&resource) {
                Some((size, caching)) if fresh(caching) => {
                    self.complete_stored_resource(db, &resource, size);
                    return;
                }
                Some(_) => db.get_region_resource(&resource),
                None => None,
            }
        };

        let mut online_resource = resource.clone();
        if let Some((stored, _)) = &stale {
            // Lets the server answer 304 Not Modified.
            online_resource.set_prior(stored);
        }

        let request = self.next_request();
        let (region, post) = (self.id, Arc::clone(&self.post));
        let (handle, responder) = Request::new(move |response: Response| {
            post(Box::new(move |db, downloads| downloads.on_response(db, region, request, response)));
        });
        self.requests.insert(request, Pending::Loading { _request: handle, resource, then, stale });
        self.network.load(&online_resource, responder);
    }

    /// A response to the request `request` of `ensure_resource_task`.
    fn on_online_response(&mut self, db: &mut OfflineDatabase, request: u64, online_response: Response) {
        let Some(Pending::Loading { resource, then, stale, .. }) = self.requests.get(&request) else {
            return;
        };
        let (resource, then, stale) = (resource.clone(), then.clone(), stale.clone());

        if let Some(error) = &online_response.error {
            self.observer.response_error((**error).clone());
            // Another error keeps the request, which retries.
            if error.reason == Reason::NotFound {
                self.requests.remove(&request);
                if let Some(stale) = &stale
                    && then.reads()
                {
                    self.continue_from_stored_copy(db, &resource, &then, stale);
                    return;
                }

                // On error 404, we skip this request and go further.
                assert!(self.status.required_resource_count > 0);
                self.status.required_resource_count -= 1;
                if resource.kind == Kind::Source
                    && self.required_source_urls.remove(&resource.url)
                    && self.required_source_urls.is_empty()
                {
                    self.status.required_resource_count_is_precise = true;
                }
                self.notify_status_changed();
                self.continue_download(db);
            }
            return;
        }

        self.requests.remove(&request);

        if then.reads() {
            // A 304 response has no data, so pass on the stored copy.
            let parsed = match &stale {
                Some((stored, _)) if online_response.not_modified => self.read(&then, stored),
                _ => self.read(&then, &online_response),
            };
            if let Err(error) = &parsed {
                self.report_unparsable(&resource, error);
            }
            if let (Err(_), Some(stale), false) = (&parsed, &stale, online_response.not_modified) {
                self.continue_from_stored_copy(db, &resource, &then, stale);
                return;
            }
        }

        // Queue up for batched insertion
        self.buffer.push((resource, online_response));

        // Flush buffer periodically.
        // Have to keep `resourcesRemaining.empty()` as the following
        // condition would fail otherwise.
        if self.buffer.len() == K_RESOURCES_BATCH_SIZE || self.resources_remaining.is_empty() {
            self.flush_resources_buffer(db);
        }

        self.continue_download(db);
    }

    /// A style or TileJSON lists the region's other resources. When its
    /// refresh is missing or can't be read, the download goes on from the
    /// stored copy.
    fn continue_from_stored_copy(
        &mut self,
        db: &mut OfflineDatabase,
        resource: &Resource,
        then: &Then,
        (stored, size): &(Response, u64),
    ) {
        if let Err(error) = self.read(then, stored) {
            self.report_unparsable(resource, &error);
        }
        self.complete_stored_resource(db, resource, *size);
    }

    /// Counts a resource the region already holds as complete.
    fn complete_stored_resource(&mut self, db: &mut OfflineDatabase, resource: &Resource, size: u64) {
        self.resources_to_be_marked_as_used.push(resource.clone());
        self.status.completed_resource_count += 1;
        self.status.completed_resource_size = self.status.completed_resource_size.wrapping_add(size);
        if resource.kind == Kind::Tile {
            self.status.completed_tile_count += 1;
            self.status.completed_tile_size = self.status.completed_tile_size.wrapping_add(size);
        }
        self.notify_status_changed();
        self.continue_download(db);
    }

    fn report_unparsable(&self, resource: &Resource, error: &str) {
        let message = format!("Can't parse {}: {error}", resource.url);
        self.observer.response_error(ResourceError::new(Reason::Other, message));
    }
}
