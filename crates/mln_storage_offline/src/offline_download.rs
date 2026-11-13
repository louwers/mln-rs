// Ports platform/default/include/mln/storage/offline_download.hpp and
// platform/default/src/mln/storage/offline_download.cpp.
//
// C++ holds the OfflineDatabase and the online FileSource by reference; here
// they are shared (`Rc<RefCell<OfflineDatabase>>`, `Arc<dyn FileSource>`).
// The download's state is behind `Rc` with interior mutability, because the
// C++ code is re-entrant: the observer may call `setState`/`getStatus` from
// inside `statusChanged`, and response callbacks queue new work. No borrow
// is held across a call to the observer, a callback or the database.
// Callbacks hold the download weakly (C++ captures `this`; the requests
// that run them are owned by the download and die with it).
//
// `requests` is keyed by an id (C++: a `std::list` of requests, erased by
// iterator). `RunLoop::invokeCancellable` is `invoke_cancellable` below.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use feature::Value;
use mln_actor::{LocalObject, RunLoop};
use mln_conversion::Error as ConversionError;
use mln_renderer::util::tile_cover::{TileCover, covering_zoom_level, tile_count_bounds, tile_count_geometry};
use mln_storage::file_source::MAX_CONCURRENT_REQUESTS_KEY;
use mln_storage::offline::{
    OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionObserver, OfflineRegionStatus,
};
use mln_storage::offline_database::OfflineDatabase;
use mln_storage::resource::{Priority, Usage};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::{AsyncRequest, FileSource, Kind, LoadingMethod, Resource, Response};
use mln_style::conversion::tileset::convert_json_tileset;
use mln_style::parser::Parser;
use mln_style::sources::image_source::ImageSource;
use mln_style::sources::{GeoJSONSource, RasterDEMSource, RasterSource, UrlOrTileset, VectorSource};
use mln_style_types::SourceType;
use mln_text::glyph::{
    GLYPH_RANGES_PER_FONT_STACK, GLYPHS_PER_GLYPH_RANGE, GlyphID, NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK,
    get_glyph_range,
};
use mln_tile_id::tile_id::CanonicalTileID;
use mln_util::constants::{DEFAULT_MAXIMUM_CONCURRENT_REQUESTS, TILE_SIZE_I};
use mln_util::i18n::glyph_range_is_entirely_locally_generated;
use mln_util::range::Range;
use mln_util::tileset::Tileset;
use mln_vec_map::VecMap;

const K_RESOURCES_BATCH_SIZE: usize = 64;
const K_MARK_BATCH_SIZE: usize = 200;

/// `ResourceCallback` of `ensureResource`: reads a style or TileJSON, and
/// returns why it can't when it can't.
type ResponseCallback = Rc<dyn Fn(&Response) -> Result<(), String>>;

// Generic functions

/// `coveringZoomRange(definition, type, tileSize, zoomRange)`.
fn covering_zoom_range(
    definition: &OfflineRegionDefinition,
    type_: SourceType,
    tile_size: u16,
    zoom_range: Range<u8>,
) -> Range<u8> {
    // `std::max<double>` / `std::min<double>`.
    let a = f64::from(covering_zoom_level(definition.min_zoom(), type_, tile_size));
    let b = f64::from(zoom_range.min);
    let min_z = if a < b { b } else { a };
    let a = f64::from(covering_zoom_level(definition.max_zoom(), type_, tile_size));
    let b = f64::from(zoom_range.max);
    let max_z = if b < a { b } else { a };

    debug_assert!(min_z >= 0.0);
    debug_assert!(max_z >= 0.0);
    debug_assert!(min_z < f64::from(u8::MAX));
    debug_assert!(max_z < f64::from(u8::MAX));
    Range::new(min_z as u8, max_z as u8)
}

/// `tileCover(definition, type, tileSize, zoomRange, fn)`.
fn tile_cover(
    definition: &OfflineRegionDefinition,
    type_: SourceType,
    tile_size: u16,
    zoom_range: Range<u8>,
    mut f: impl FnMut(CanonicalTileID),
) {
    let clamped_zoom_range = covering_zoom_range(definition, type_, tile_size, zoom_range);

    for z in clamped_zoom_range.min..=clamped_zoom_range.max {
        let cover = match definition {
            OfflineRegionDefinition::TilePyramid(reg) => Some(TileCover::new_from_bounds(&reg.bounds, z)),
            // C++ `util::TileCover(geometry, z)` (project = true).
            OfflineRegionDefinition::Geometry(reg) => TileCover::new_from_geometry(&reg.geometry, z, true).ok(),
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

/// `tileCount(definition, type, tileSize, zoomRange)`.
fn tile_count(definition: &OfflineRegionDefinition, type_: SourceType, tile_size: u16, zoom_range: Range<u8>) -> u64 {
    let clamped_zoom_range = covering_zoom_range(definition, type_, tile_size, zoom_range);

    let mut result: u64 = 0;
    for z in clamped_zoom_range.min..=clamped_zoom_range.max {
        result += match definition {
            OfflineRegionDefinition::TilePyramid(reg) => tile_count_bounds(&reg.bounds, z),
            OfflineRegionDefinition::Geometry(reg) => tile_count_geometry(&reg.geometry, z).unwrap_or(0),
        };
    }

    result
}

/// A task of [`invoke_cancellable`], until it runs or is cancelled.
type CancellableTask = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// `WorkRequest`: dropping it cancels the task if it has not run.
struct WorkRequest {
    task: CancellableTask,
}

impl AsyncRequest for WorkRequest {}

impl Drop for WorkRequest {
    fn drop(&mut self) {
        self.task.borrow_mut().take();
    }
}

/// `util::RunLoop::Get()->invokeCancellable(fn)`: runs `f` on the current
/// `RunLoop` unless the returned request is dropped first. The task stays on
/// this thread (a `LocalObject`); the loop gets a `Send` handle to it.
fn invoke_cancellable(f: impl FnOnce() + 'static) -> Box<dyn AsyncRequest> {
    let task: CancellableTask = Rc::new(RefCell::new(Some(Box::new(f))));
    let handle = LocalObject::from_rc(task.clone());
    RunLoop::get().invoke(move || {
        let task = handle.get().and_then(|task| task.borrow_mut().take());
        if let Some(task) = task {
            task();
        }
    });
    Box::new(WorkRequest { task })
}

/// The default observer (`std::make_unique<OfflineRegionObserver>()`).
struct NullObserver;

impl OfflineRegionObserver for NullObserver {}

/// The members of `OfflineDownload`.
struct Inner {
    id: i64,
    definition: OfflineRegionDefinition,
    offline_database: Rc<RefCell<OfflineDatabase>>,
    online_file_source: Arc<dyn FileSource>,
    status: RefCell<OfflineRegionStatus>,
    observer: RefCell<Rc<dyn OfflineRegionObserver>>,

    requests: RefCell<VecMap<u64, Option<Box<dyn AsyncRequest>>>>,
    next_request_id: Cell<u64>,
    required_source_urls: RefCell<HashSet<String>>,
    resources_remaining: RefCell<VecDeque<Resource>>,
    resources_to_be_marked_as_used: RefCell<Vec<Resource>>,
    buffer: RefCell<Vec<(Resource, Response)>>,

    /// Counts activations and deactivations, so that work an observer
    /// callback interrupted doesn't go on into a later traversal.
    traversal: Cell<u64>,
}

/// `OfflineDownload`: the download of an offline region's resources. Not
/// `Clone`: tests and observers hold a `WeakOfflineDownload`.
pub struct OfflineDownload {
    inner: Rc<Inner>,
}

/// A weak reference to an `OfflineDownload` (for observers that call back
/// into it, as C++ observers capture the download by reference).
#[derive(Clone)]
pub struct WeakOfflineDownload {
    inner: Weak<Inner>,
}

impl WeakOfflineDownload {
    /// `setState` on the download, if it still exists.
    pub fn set_state(&self, state: OfflineRegionDownloadState) {
        if let Some(inner) = self.inner.upgrade() {
            inner.set_state(state);
        }
    }

    /// `getStatus` of the download, if it still exists.
    #[must_use]
    pub fn get_status(&self) -> Option<OfflineRegionStatus> {
        self.inner.upgrade().map(|inner| inner.get_status())
    }
}

impl OfflineDownload {
    /// `OfflineDownload(int64_t id, OfflineRegionDefinition, OfflineDatabase&, FileSource& onlineFileSource)`.
    pub fn new(
        id: i64,
        definition: OfflineRegionDefinition,
        offline_database: Rc<RefCell<OfflineDatabase>>,
        online_file_source: Arc<dyn FileSource>,
    ) -> Self {
        let inner = Rc::new(Inner {
            id,
            definition,
            offline_database,
            online_file_source,
            status: RefCell::new(OfflineRegionStatus::default()),
            observer: RefCell::new(Rc::new(NullObserver)),
            requests: RefCell::new(VecMap::new()),
            next_request_id: Cell::new(0),
            required_source_urls: RefCell::new(HashSet::new()),
            resources_remaining: RefCell::new(VecDeque::new()),
            resources_to_be_marked_as_used: RefCell::new(Vec::new()),
            buffer: RefCell::new(Vec::new()),
            traversal: Cell::new(0),
        });
        let download = OfflineDownload { inner };
        download.set_observer(None);
        download
    }

    /// A weak reference, for observers that call back into the download.
    #[must_use]
    pub fn downgrade(&self) -> WeakOfflineDownload {
        WeakOfflineDownload { inner: Rc::downgrade(&self.inner) }
    }

    /// `setObserver(std::unique_ptr<OfflineRegionObserver>)`.
    pub fn set_observer(&self, observer: Option<Rc<dyn OfflineRegionObserver>>) {
        *self.inner.observer.borrow_mut() = observer.unwrap_or_else(|| Rc::new(NullObserver));
    }

    /// `setState(OfflineRegionDownloadState)`.
    pub fn set_state(&self, state: OfflineRegionDownloadState) {
        self.inner.set_state(state);
    }

    /// `getStatus()`.
    #[must_use]
    pub fn get_status(&self) -> OfflineRegionStatus {
        self.inner.get_status()
    }
}

impl Drop for OfflineDownload {
    fn drop(&mut self) {
        // The requests' callbacks hold the download weakly; dropping the
        // requests cancels them, as the C++ destructor does.
        let requests = std::mem::take(&mut *self.inner.requests.borrow_mut());
        drop(requests);
    }
}

impl Inner {
    fn observer(&self) -> Rc<dyn OfflineRegionObserver> {
        self.observer.borrow().clone()
    }

    /// `observer->statusChanged(status)`.
    fn notify_status_changed(&self) {
        let status = *self.status.borrow();
        self.observer().status_changed(status);
    }

    /// `requests.insert(requests.begin(), nullptr)`.
    fn insert_request(&self) -> u64 {
        let id = self.next_request_id.get();
        self.next_request_id.set(id + 1);
        self.requests.borrow_mut().insert(id, None);
        id
    }

    /// `*it = request` (if the entry still exists).
    fn set_request(&self, id: u64, request: Box<dyn AsyncRequest>) {
        let previous = {
            let mut requests = self.requests.borrow_mut();
            match requests.get_mut(&id) {
                Some(slot) => slot.replace(request),
                None => Some(request),
            }
        };
        drop(previous);
    }

    /// `requests.erase(it)`.
    fn remove_request(&self, id: u64) {
        let request = self.requests.borrow_mut().remove(&id);
        drop(request);
    }

    fn set_state(self: &Rc<Self>, state: OfflineRegionDownloadState) {
        if self.status.borrow().download_state == state {
            return;
        }

        self.status.borrow_mut().download_state = state;

        if state == OfflineRegionDownloadState::Active {
            self.activate_download();
        } else {
            self.deactivate_download();
        }

        self.notify_status_changed();
    }

    fn get_status(&self) -> OfflineRegionStatus {
        if self.status.borrow().download_state == OfflineRegionDownloadState::Active {
            return *self.status.borrow();
        }

        let result = self.offline_database.borrow_mut().get_region_completed_status(self.id);
        let Ok(mut result) = result else {
            // We can't find this offline region because the database is
            // unavailable, or the download does not exist.
            return OfflineRegionStatus::default();
        };

        result.required_resource_count += 1;
        let style_response = self.offline_database.borrow_mut().get(&Resource::style(self.definition.style_url()));
        let mut parser = Parser::default();
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
            let type_ = source.get_type();

            let mut handle_tiled_source = |url_or_tileset: &UrlOrTileset, tile_size: u16| match url_or_tileset {
                UrlOrTileset::Tileset(tileset) => {
                    // A source without tile URLs has no tiles to download.
                    if !tileset.tiles.is_empty() {
                        let tile_source_count = tile_count(&self.definition, type_, tile_size, tileset.zoom_range);
                        result.required_tile_count += tile_source_count;
                        result.required_resource_count += tile_source_count;
                    }
                }
                UrlOrTileset::Url(url) => {
                    result.required_resource_count += 1;
                    let source_response = self.offline_database.borrow_mut().get(&Resource::source(url));
                    let tileset = source_response.and_then(|response| response.data).and_then(|data| {
                        let mut error = ConversionError::default();
                        convert_json_tileset(&data, &mut error)
                    });
                    // As a download does, count no tiles for a TileJSON that
                    // is missing, can't be read, or has no tile URLs, and
                    // leave the count imprecise.
                    match tileset {
                        Some(tileset) if !tileset.tiles.is_empty() => {
                            let tile_source_count = tile_count(&self.definition, type_, tile_size, tileset.zoom_range);
                            result.required_tile_count += tile_source_count;
                            result.required_resource_count += tile_source_count;
                        }
                        _ => result.required_resource_count_is_precise = false,
                    }
                }
            };

            match type_ {
                SourceType::Vector => {
                    if let Some(vector_source) = source.downcast_ref::<VectorSource>() {
                        handle_tiled_source(vector_source.tile_source().get_url_or_tileset(), TILE_SIZE_I);
                    }
                }

                SourceType::Raster => {
                    if let Some(raster_source) = source.downcast_ref::<RasterSource>() {
                        let tile_source = raster_source.tile_source();
                        handle_tiled_source(tile_source.get_url_or_tileset(), tile_source.get_tile_size());
                    }
                }

                SourceType::RasterDEM => {
                    if let Some(raster_dem_source) = source.downcast_ref::<RasterDEMSource>() {
                        let tile_source = raster_dem_source.tile_source();
                        handle_tiled_source(tile_source.get_url_or_tileset(), tile_source.get_tile_size());
                    }
                }

                SourceType::GeoJSON => {
                    if let Some(geojson_source) = source.downcast_ref::<GeoJSONSource>()
                        && geojson_source.get_url().is_some()
                    {
                        result.required_resource_count += 1;
                    }
                }

                SourceType::Image => {
                    if let Some(image_source) = source.downcast_ref::<ImageSource>()
                        && image_source.get_url().is_some()
                    {
                        result.required_resource_count += 1;
                    }
                }

                SourceType::CustomVector | SourceType::CustomMVTVector => {}
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

    fn activate_download(self: &Rc<Self>) {
        self.traversal.set(self.traversal.get() + 1);
        {
            let mut status = self.status.borrow_mut();
            *status = OfflineRegionStatus::default();
            status.download_state = OfflineRegionDownloadState::Active;
            status.required_resource_count += 1;
        }

        let mut style_resource = Resource::style(self.definition.style_url());
        style_resource.set_priority(Priority::Low);
        style_resource.set_usage(Usage::Offline);

        let weak = Rc::downgrade(self);
        self.ensure_resource(
            style_resource,
            Some(Rc::new(move |style_response: &Response| match weak.upgrade() {
                Some(this) => this.on_style_response(style_response),
                None => Ok(()),
            })),
        );
    }

    /// The style callback of `activateDownload`.
    fn on_style_response(self: &Rc<Self>, style_response: &Response) -> Result<(), String> {
        let Some(style_data) = &style_response.data else {
            return Err("the style has no content".to_owned());
        };
        let mut parser = Parser::default();
        parser.parse(&String::from_utf8_lossy(style_data)).map_err(|error| error.to_string())?;
        self.status.borrow_mut().required_resource_count_is_precise = true;

        for source in &parser.sources {
            let type_ = source.get_type();

            let handle_tiled_source = |url_or_tileset: &UrlOrTileset, tile_size: u16| match url_or_tileset {
                UrlOrTileset::Tileset(tileset) => self.queue_tiles(type_, tile_size, tileset),
                UrlOrTileset::Url(raw_url) => {
                    let url = raw_url.clone();

                    {
                        let mut status = self.status.borrow_mut();
                        status.required_resource_count_is_precise = false;
                        status.required_resource_count += 1;
                    }
                    self.required_source_urls.borrow_mut().insert(url.clone());

                    let mut source_resource = Resource::source(&url);
                    source_resource.set_priority(Priority::Low);
                    source_resource.set_usage(Usage::Offline);

                    let weak = Rc::downgrade(self);
                    self.ensure_resource(
                        source_resource,
                        Some(Rc::new(move |source_response: &Response| {
                            let Some(this) = weak.upgrade() else {
                                return Ok(());
                            };
                            let Some(data) = &source_response.data else {
                                return Err("the TileJSON has no content".to_owned());
                            };
                            let mut error = ConversionError::default();
                            let Some(tileset) = convert_json_tileset(data, &mut error) else {
                                return Err(error.message);
                            };
                            if tileset.tiles.is_empty() {
                                return Err("the TileJSON has no tile URLs".to_owned());
                            }
                            this.queue_tiles(type_, tile_size, &tileset);

                            this.required_source_urls.borrow_mut().remove(&url);
                            if this.required_source_urls.borrow().is_empty() {
                                this.status.borrow_mut().required_resource_count_is_precise = true;
                            }
                            Ok(())
                        })),
                    );
                }
            };

            match type_ {
                SourceType::Vector => {
                    if let Some(vector_source) = source.downcast_ref::<VectorSource>() {
                        handle_tiled_source(vector_source.tile_source().get_url_or_tileset(), TILE_SIZE_I);
                    }
                }

                SourceType::Raster => {
                    if let Some(raster_source) = source.downcast_ref::<RasterSource>() {
                        let tile_source = raster_source.tile_source();
                        handle_tiled_source(tile_source.get_url_or_tileset(), tile_source.get_tile_size());
                    }
                }

                SourceType::RasterDEM => {
                    if let Some(raster_dem_source) = source.downcast_ref::<RasterDEMSource>() {
                        let tile_source = raster_dem_source.tile_source();
                        handle_tiled_source(tile_source.get_url_or_tileset(), tile_source.get_tile_size());
                    }
                }

                SourceType::GeoJSON => {
                    if let Some(geojson_source) = source.downcast_ref::<GeoJSONSource>()
                        && let Some(url) = geojson_source.get_url()
                    {
                        self.queue_resource(Resource::source(&url));
                    }
                }

                SourceType::Image => {
                    if let Some(image_source) = source.downcast_ref::<ImageSource>()
                        && let Some(image_url) = image_source.get_url()
                        && !image_url.is_empty()
                    {
                        self.queue_resource(Resource::image(&image_url));
                    }
                }

                SourceType::CustomVector | SourceType::CustomMVTVector => {}
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

        self.continue_download();
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
    fn continue_download(self: &Rc<Self>) {
        // An observer can pause the download, or restart it, from a status
        // change.
        if !self.is_active() {
            return;
        }
        let current = self.traversal.get();

        if self.resources_remaining.borrow().is_empty() {
            // Flush pending buffers.
            self.flush_resources_buffer();
            if self.interrupted(current) {
                return;
            }
            if self.status.borrow().complete() {
                self.mark_pending_used_resources();
                self.set_state(OfflineRegionDownloadState::Inactive);
                return;
            }
        }

        if self.resources_to_be_marked_as_used.borrow().len() >= K_MARK_BATCH_SIZE {
            self.mark_pending_used_resources();
        }

        let mut max_concurrent_requests = DEFAULT_MAXIMUM_CONCURRENT_REQUESTS;
        if let Value::Uint(max_requests) = self.online_file_source.get_property(MAX_CONCURRENT_REQUESTS_KEY) {
            max_concurrent_requests = max_requests as u32;
        }

        loop {
            if self.requests.borrow().len() >= max_concurrent_requests as usize {
                break;
            }
            let Some(resource) = self.resources_remaining.borrow_mut().pop_front() else {
                break;
            };
            self.ensure_resource(resource, None);
        }
    }

    fn deactivate_download(&self) {
        self.required_source_urls.borrow_mut().clear();
        self.resources_remaining.borrow_mut().clear();
        let requests = std::mem::take(&mut *self.requests.borrow_mut());
        drop(requests);
        self.buffer.borrow_mut().clear();
        self.traversal.set(self.traversal.get() + 1);
    }

    fn is_active(&self) -> bool {
        self.status.borrow().download_state == OfflineRegionDownloadState::Active
    }

    /// Whether an observer paused or restarted the download since traversal
    /// `current` began, which ends that traversal.
    fn interrupted(&self, current: u64) -> bool {
        self.traversal.get() != current || !self.is_active()
    }

    fn flush_resources_buffer(self: &Rc<Self>) {
        if self.buffer.borrow().is_empty() {
            return;
        }
        let buffer = self.buffer.borrow().clone();
        let mut status = *self.status.borrow();
        self.offline_database.borrow_mut().put_region_resources(self.id, &buffer, &mut status);
        *self.status.borrow_mut() = status;
        self.buffer.borrow_mut().clear();
        self.notify_status_changed();
    }

    fn queue_resource(&self, mut resource: Resource) {
        resource.set_priority(Priority::Low);
        resource.set_usage(Usage::Offline);
        {
            let mut status = self.status.borrow_mut();
            status.required_resource_count += 1;
            if resource.kind == Kind::Tile {
                status.required_tile_count += 1;
            }
        }
        self.resources_remaining.borrow_mut().push_front(resource);
    }

    fn queue_tiles(&self, type_: SourceType, tile_size: u16, tileset: &Tileset) {
        // A source without tile URLs has no tiles to download.
        let Some(url_template) = tileset.tiles.first() else {
            return;
        };
        tile_cover(&self.definition, type_, tile_size, tileset.zoom_range, |tile: CanonicalTileID| {
            {
                let mut status = self.status.borrow_mut();
                status.required_resource_count += 1;
                status.required_tile_count += 1;
            }

            let mut tile_resource = Resource::tile(
                url_template,
                self.definition.pixel_ratio(),
                tile.x as i32,
                tile.y as i32,
                tile.z as i8,
                tileset.scheme,
                LoadingMethod::All,
            );

            tile_resource.set_priority(Priority::Low);
            tile_resource.set_usage(Usage::Offline);

            self.resources_remaining.borrow_mut().push_back(tile_resource);
        });
    }

    fn mark_pending_used_resources(&self) {
        let resources = self.resources_to_be_marked_as_used.borrow().clone();
        self.offline_database.borrow_mut().mark_used_resources(self.id, &resources);
        self.resources_to_be_marked_as_used.borrow_mut().clear();
    }

    fn ensure_resource(self: &Rc<Self>, resource: Resource, callback: Option<ResponseCallback>) {
        debug_assert_eq!(resource.priority, Priority::Low);
        debug_assert_eq!(resource.usage, Usage::Offline);

        // An observer can pause or restart the download from any callback,
        // which ends this traversal. Nothing more of this resource happens
        // after that.
        let current = self.traversal.get();
        let work_request_id = self.insert_request();
        let weak = Rc::downgrade(self);
        let task = invoke_cancellable(move || {
            if let Some(this) = weak.upgrade() {
                this.ensure_resource_task(work_request_id, current, resource, callback);
            }
        });
        self.set_request(work_request_id, task);
    }

    /// The task of `ensureResource`.
    fn ensure_resource_task(
        self: &Rc<Self>,
        work_request_id: u64,
        current: u64,
        resource: Resource,
        callback: Option<ResponseCallback>,
    ) {
        self.remove_request(work_request_id);

        // A stored resource that can't be used without revalidating it, such
        // as one invalidate_region marked, isn't complete. Keep it to
        // revalidate it with the server.
        let stale = match &callback {
            None => {
                let stored = self.offline_database.borrow_mut().check_region_resource(&resource);
                match stored {
                    Some((size, true)) => {
                        self.complete_stored_resource(&resource, size as u64);
                        return;
                    }
                    Some((_, false)) => self.offline_database.borrow_mut().get_region_resource(&resource),
                    None => None,
                }
            }
            Some(callback) => {
                let stored = self.offline_database.borrow_mut().get_region_resource(&resource);
                match stored {
                    Some((response, size)) if response.is_usable() => {
                        if let Err(error) = callback(&response) {
                            self.report_unparsable(&resource, &error);
                        }
                        if self.interrupted(current) {
                            return;
                        }
                        self.complete_stored_resource(&resource, size);
                        return;
                    }
                    stale => stale,
                }
            }
        };

        let mut online_resource = resource.clone();
        if let Some((stored, _)) = &stale {
            // Lets the server answer 304 Not Modified.
            online_resource.prior_etag = stored.etag.clone();
            online_resource.prior_modified = stored.modified;
        }

        let file_request_id = self.insert_request();
        let weak = Rc::downgrade(self);
        let stale = stale.map(Rc::new);
        let request = self.online_file_source.request(
            &online_resource,
            Box::new(move |online_response: Response| {
                if let Some(this) = weak.upgrade() {
                    let stale = stale.as_deref();
                    this.on_online_response(
                        file_request_id,
                        current,
                        &resource,
                        callback.as_ref(),
                        stale,
                        online_response,
                    );
                }
            }),
        );
        self.set_request(file_request_id, request);
    }

    /// The online request callback of `ensureResource`. `stale` is the stored
    /// copy being revalidated, with its stored size.
    fn on_online_response(
        self: &Rc<Self>,
        file_request_id: u64,
        current: u64,
        resource: &Resource,
        callback: Option<&ResponseCallback>,
        stale: Option<&(Response, u64)>,
        online_response: Response,
    ) {
        if let Some(error) = &online_response.error {
            self.observer().response_error((**error).clone());
            if self.interrupted(current) {
                return;
            }
            if error.reason == Reason::NotFound {
                self.remove_request(file_request_id);
                if let (Some(stale), Some(callback)) = (stale, callback) {
                    self.continue_from_stored_copy(current, resource, callback, stale);
                    return;
                }

                // On error 404, we skip this request and go further.
                {
                    let mut status = self.status.borrow_mut();
                    assert!(status.required_resource_count > 0);
                    status.required_resource_count -= 1;
                }
                if resource.kind == Kind::Source {
                    let mut required_source_urls = self.required_source_urls.borrow_mut();
                    if required_source_urls.remove(&resource.url) && required_source_urls.is_empty() {
                        self.status.borrow_mut().required_resource_count_is_precise = true;
                    }
                }
                self.notify_status_changed();
                if self.interrupted(current) {
                    return;
                }
                self.continue_download();
            }
            return;
        }

        self.remove_request(file_request_id);

        if let Some(callback) = callback {
            // A 304 response has no data, so pass on the stored copy.
            let parsed = match stale {
                Some((stored, _)) if online_response.not_modified => callback(stored),
                _ => callback(&online_response),
            };
            if let Err(error) = &parsed {
                self.report_unparsable(resource, error);
            }
            if self.interrupted(current) {
                return;
            }
            if let (Err(_), Some(stale), false) = (&parsed, stale, online_response.not_modified) {
                self.continue_from_stored_copy(current, resource, callback, stale);
                return;
            }
        }

        // Queue up for batched insertion
        self.buffer.borrow_mut().push((resource.clone(), online_response));

        // Flush buffer periodically.
        // Have to keep `resourcesRemaining.empty()` as the following
        // condition would fail otherwise.
        let flush =
            self.buffer.borrow().len() == K_RESOURCES_BATCH_SIZE || self.resources_remaining.borrow().is_empty();
        if flush {
            self.flush_resources_buffer();
        }
        if self.interrupted(current) {
            return;
        }

        self.continue_download();
    }

    /// A style or TileJSON lists the region's other resources. When its
    /// refresh is missing or can't be read, the download goes on from the
    /// stored copy.
    fn continue_from_stored_copy(
        self: &Rc<Self>,
        current: u64,
        resource: &Resource,
        callback: &ResponseCallback,
        (stored, size): &(Response, u64),
    ) {
        if let Err(error) = callback(stored) {
            self.report_unparsable(resource, &error);
        }
        if self.interrupted(current) {
            return;
        }
        self.complete_stored_resource(resource, *size);
    }

    /// Counts a resource the region already holds as complete.
    fn complete_stored_resource(self: &Rc<Self>, resource: &Resource, size: u64) {
        self.resources_to_be_marked_as_used.borrow_mut().push(resource.clone());
        {
            let mut status = self.status.borrow_mut();
            status.completed_resource_count += 1;
            status.completed_resource_size = status.completed_resource_size.wrapping_add(size);
            if resource.kind == Kind::Tile {
                status.completed_tile_count += 1;
                status.completed_tile_size = status.completed_tile_size.wrapping_add(size);
            }
        }

        let current = self.traversal.get();
        self.notify_status_changed();
        if self.interrupted(current) {
            return;
        }
        self.continue_download();
    }

    fn report_unparsable(&self, resource: &Resource, error: &str) {
        let message = format!("Can't parse {}: {error}", resource.url);
        self.observer().response_error(ResourceError::new(Reason::Other, message));
    }
}
