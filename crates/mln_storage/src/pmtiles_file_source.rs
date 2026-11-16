// A request starts on a thread pool: header, metadata and directories are
// read with range requests through the file source that loads the archives,
// and each step continues on the thread that answers the one before. A
// request's sub-request replaces the one before, and goes when the request
// is cancelled.

use std::fmt::Write as _;
use std::sync::Arc;

use bytes::Bytes;
use mln_actor::SchedulerPtr;
use mln_util::compression::{decompress, is_compressed};
use mln_util::constants::{FILE_PROTOCOL, PMTILES_PROTOCOL};
use mln_vec_map::VecMap;
use parking_lot::Mutex;
use pmtiles::{Compression, DirEntry, Directory, Header, PmtError, TileCoord, TileId, TileType};
use serde_json::{Value as Json, json};

use crate::archive_tilejson::archive_tilejson;
use crate::file_source::FileSource;
use crate::request::{Request, Responder};
use crate::resource::{Kind, LoadingMethod, Resource};
use crate::response::{Reason, ResourceError, Response};

// https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md#3-header
const PMTILES_HEADER_OFFSET: u64 = 0;
const PMTILES_HEADER_LENGTH: u64 = 127;

// To avoid allocating lots of memory with PMTiles directory caching, set a
// limit so it doesn't grow unlimited
const MAX_DIRECTORY_CACHE_ENTRIES: usize = 100;

/// Parses a PMTiles header with C++'s `pmtiles::deserialize_header` checks
/// and error texts, then reads the fields through the pmtiles crate. Unlike
/// C++, the crate refuses a compression or a tile type that the spec doesn't
/// define; a compression that C++ would read and then refuse fails with
/// C++'s message.
fn deserialize_header(s: &[u8]) -> Result<Header, String> {
    if !s.starts_with(b"PMTiles") {
        return Err("pmtiles magic number exception".to_owned());
    }
    if s.len() != 127 || s.get(7) != Some(&0x3) {
        return Err("pmtiles version: must be 3".to_owned());
    }
    Header::try_from_bytes(Bytes::copy_from_slice(s)).map_err(|e| match e {
        PmtError::InvalidCompression => "Compression method not supported".to_owned(),
        e => e.to_string(),
    })
}

/// The tile id of `z/x/y`, with the C++ `pmtiles::zxy_to_tileid` error
/// texts.
fn zxy_to_tileid(z: u8, x: u32, y: u32) -> Result<TileId, &'static str> {
    TileCoord::new(z, x, y)
        .map(TileId::from)
        .map_err(|_| if z > 31 { "tile zoom exceeds 64-bit limit" } else { "tile x/y outside zoom level bounds" })
}

fn accepts_url(url: &str) -> bool {
    url.starts_with(PMTILES_PROTOCOL)
}

fn extract_url(url: &str) -> String {
    url[PMTILES_PROTOCOL.len().min(url.len())..].to_owned()
}

type ErrorPtr = Option<Box<ResourceError>>;
/// Called when an asynchronous step finishes, with its error if it failed.
type AsyncCallback = Arc<dyn Fn(ErrorPtr) + Send + Sync>;
type AsyncTileCallback = Arc<dyn Fn((u64, u32), ErrorPtr) + Send + Sync>;
/// A request's sub-request in flight.
type Task = Arc<Mutex<Option<Request>>>;

fn error(reason: Reason, message: impl Into<String>) -> ResourceError {
    ResourceError::new(reason, message)
}

/// The members of `PMTilesFileSource::Impl`.
struct State {
    /// Loads the archives' bytes.
    file_source: Arc<dyn FileSource>,
    header_cache: Mutex<VecMap<String, Arc<Header>>>,
    metadata_cache: Mutex<VecMap<String, String>>,
    directory_cache: Mutex<VecMap<String, VecMap<String, Directory>>>,
    directory_cache_control: Mutex<VecMap<String, Vec<String>>>,
}

impl State {
    /// Requests `resource` for `task`, in place of its sub-request before.
    fn request(&self, req: &Task, resource: &Resource, callback: impl FnMut(Response) + Send + 'static) {
        let (request, responder) = Request::direct(callback);
        let previous = req.lock().replace(request);
        drop(previous);
        self.file_source.load(resource, responder);
    }

    fn get_header(self: &Arc<Self>, url: &str, req: &Task, callback: AsyncCallback) {
        if self.header_cache.lock().contains_key(url) {
            callback(None);
            return;
        }

        let mut resource = Resource::new(Kind::Source, url);
        resource.loading_method = LoadingMethod::All;

        resource.data_range = Some((PMTILES_HEADER_OFFSET, PMTILES_HEADER_OFFSET + PMTILES_HEADER_LENGTH - 1));

        let this = Arc::downgrade(self);
        let url = url.to_owned();
        self.request(req, &resource, move |response: Response| {
            let Some(this) = this.upgrade() else { return };
            if let Some(response_error) = &response.error {
                let mut message = format!("Error fetching PMTiles header: {}", response_error.message);

                if response_error.message.is_empty() && response_error.reason == Reason::NotFound {
                    if let Some(path) = url.strip_prefix(FILE_PROTOCOL) {
                        let _ = write!(message, "path not found: {path}");
                    } else {
                        let _ = write!(message, "url not found: {url}");
                    }
                }

                callback(Some(Box::new(error(response_error.reason, message))));

                return;
            }

            if response.not_modified {
                return;
            }

            let Some(data) = &response.data else {
                callback(Some(Box::new(error(Reason::Other, "PMTiles header response has no data"))));
                return;
            };

            let header = deserialize_header(&data[..data.len().min(127)]).and_then(|header| {
                let supported = |c: Compression| c == Compression::None || c == Compression::Gzip;
                if !supported(header.internal_compression()) || !supported(header.tile_compression) {
                    return Err("Compression method not supported".to_owned());
                }
                Ok(header)
            });
            match header {
                Ok(header) => {
                    this.header_cache.lock().entry(url.clone()).or_insert_with(|| Arc::new(header));
                    callback(None);
                }
                Err(e) => callback(Some(Box::new(error(Reason::Other, format!("Error parsing PMTiles header: {e}"))))),
            }
        });
    }

    fn get_metadata(self: &Arc<Self>, url: &str, req: &Task, callback: AsyncCallback) {
        if self.metadata_cache.lock().contains_key(url) {
            callback(None);
            return;
        }

        let this = Arc::downgrade(self);
        let url = url.to_owned();
        let task = Arc::clone(req);
        self.get_header(
            &url.clone(),
            req,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if error_ptr.is_some() {
                    callback(error_ptr);
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url]);

                let parse_callback = {
                    let (this, url, callback, header) =
                        (Arc::downgrade(&this), url.clone(), Arc::clone(&callback), Arc::clone(&header));
                    move |data: &[u8]| {
                        let Some(this) = this.upgrade() else { return };
                        // The header's members win over the metadata's, which
                        // may be stale or name another tile URL.
                        let members = [
                            ("tiles", json!([format!("{PMTILES_PROTOCOL}{url}")])),
                            ("minzoom", json!(header.min_zoom)),
                            ("maxzoom", json!(header.max_zoom)),
                            (
                                "bounds",
                                json!([
                                    header.min_longitude,
                                    header.min_latitude,
                                    header.max_longitude,
                                    header.max_latitude
                                ]),
                            ),
                            ("center", json!([header.center_longitude, header.center_latitude, header.center_zoom])),
                        ];
                        let encoding = (header.tile_type == TileType::Mlt).then(|| ("encoding", json!("mlt")));
                        let metadata = match serde_json::from_slice(data) {
                            Ok(Json::Object(metadata)) => metadata,
                            _ => serde_json::Map::new(),
                        };
                        let tilejson = archive_tilejson(members.into_iter().chain(encoding), metadata);
                        this.metadata_cache.lock().entry(url.clone()).or_insert(tilejson);

                        callback(None);
                    }
                };

                if header.metadata_length() > 0 {
                    let mut resource = Resource::new(Kind::Source, url.clone());
                    resource.loading_method = LoadingMethod::All;
                    resource.data_range = Some((
                        header.metadata_offset(),
                        header.metadata_offset().wrapping_add(header.metadata_length()).wrapping_sub(1),
                    ));

                    let callback = Arc::clone(&callback);
                    this.request(&task, &resource, move |response_metadata: Response| {
                        if let Some(e) = &response_metadata.error {
                            callback(Some(Box::new(error(
                                e.reason,
                                format!("Error fetching PMTiles metadata: {}", e.message),
                            ))));

                            return;
                        }

                        if response_metadata.not_modified {
                            return;
                        }

                        let Some(data) = &response_metadata.data else {
                            callback(Some(Box::new(error(
                                Reason::Other,
                                format!(
                                    "PMTiles metadata response has no data (range={}-{})",
                                    header.metadata_offset(),
                                    header.metadata_offset().wrapping_add(header.metadata_length()).wrapping_sub(1)
                                ),
                            ))));
                            return;
                        };

                        let mut data: Vec<u8> = data.to_vec();

                        if header.internal_compression() == Compression::Gzip {
                            match decompress(&data) {
                                Ok(decompressed) => data = decompressed,
                                Err(e) => {
                                    callback(Some(Box::new(error(
                                        Reason::Other,
                                        format!("Error decompressing PMTiles metadata: {e}"),
                                    ))));
                                    return;
                                }
                            }
                        }

                        parse_callback(&data);
                    });

                    return;
                }

                parse_callback(&[]);
            }),
        );
    }

    fn store_directory(
        &self,
        url: &str,
        directory_offset: u64,
        directory_length: u64,
        directory_data: Vec<u8>,
    ) -> Result<(), String> {
        let directory_cache_key = format!("{url}|{directory_offset}|{directory_length}");
        // Unlike C++, the crate refuses a directory with an entry of length 0
        // (C++ reads it, and the entry finds no tile), a first entry whose
        // offset continues a previous one, or a run past the last tile ID,
        // and it reads a directory with bytes after its entries, which C++
        // refuses. Its errors have the crate's texts.
        let directory = Directory::try_from(Bytes::from(directory_data)).map_err(|e| e.to_string())?;

        let mut cache = self.directory_cache.lock();
        let mut control = self.directory_cache_control.lock();
        let cache = cache.entry(url.to_owned()).or_default();
        let control = control.entry(url.to_owned()).or_default();

        cache.entry(directory_cache_key.clone()).or_insert(directory);
        control.push(directory_cache_key);

        if control.len() > MAX_DIRECTORY_CACHE_ENTRIES {
            cache.remove(&control[0]);
            control.remove(0);
        }
        Ok(())
    }

    fn get_directory(
        self: &Arc<Self>,
        url: &str,
        req: &Task,
        directory_offset: u64,
        directory_length: u32,
        callback: AsyncCallback,
    ) {
        let directory_cache_key = format!("{url}|{directory_offset}|{directory_length}");

        let cached = self.directory_cache.lock().get(url).is_some_and(|cache| cache.contains_key(&directory_cache_key));
        if cached {
            if let Some(control) = self.directory_cache_control.lock().get_mut(url)
                && control.last() != Some(&directory_cache_key)
            {
                control.push(directory_cache_key.clone());

                if let Some(position) = control.iter().position(|key| *key == directory_cache_key) {
                    control.remove(position);
                }
            }

            callback(None);
            return;
        }

        let this = Arc::downgrade(self);
        let url = url.to_owned();
        let task = Arc::clone(req);
        self.get_header(
            &url.clone(),
            req,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if error_ptr.is_some() {
                    callback(error_ptr);
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url]);

                let mut resource = Resource::new(Kind::Source, url.clone());
                resource.loading_method = LoadingMethod::All;
                resource.data_range = Some((
                    directory_offset,
                    directory_offset.wrapping_add(u64::from(directory_length)).wrapping_sub(1),
                ));

                let (this2, url, callback) = (Arc::downgrade(&this), url.clone(), Arc::clone(&callback));
                this.request(&task, &resource, move |response: Response| {
                    let Some(this2) = this2.upgrade() else { return };
                    if let Some(e) = &response.error {
                        callback(Some(Box::new(error(
                            e.reason,
                            format!("Error fetching PMTiles directory: {}", e.message),
                        ))));

                        return;
                    }

                    if response.not_modified {
                        return;
                    }

                    let Some(data) = &response.data else {
                        callback(Some(Box::new(error(Reason::Other, "PMTiles directory response has no data"))));
                        return;
                    };

                    let result = (|| -> Result<(), String> {
                        let mut directory_data: Vec<u8> = data.to_vec();

                        if header.internal_compression() == Compression::Gzip {
                            directory_data = decompress(&directory_data)?;
                        }

                        this2.store_directory(&url, directory_offset, u64::from(directory_length), directory_data)
                    })();
                    match result {
                        Ok(()) => callback(None),
                        Err(e) => callback(Some(Box::new(error(
                            Reason::Other,
                            format!("Error parsing PMTiles directory: {e}"),
                        )))),
                    }
                });
            }),
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the request, tile and directory location of one step of the directory walk, and its callback"
    )]
    fn get_tile_address(
        self: &Arc<Self>,
        url: &str,
        req: &Task,
        tile_id: TileId,
        directory_offset: u64,
        directory_length: u32,
        directory_depth: u32,
        callback: AsyncTileCallback,
    ) {
        if directory_depth > 3 {
            callback(
                (0, 0),
                Some(Box::new(error(
                    Reason::Other,
                    "Error fetching PMTiles tile address: Maximum directory depth exceeded",
                ))),
            );

            return;
        }

        let this = Arc::downgrade(self);
        let url = url.to_owned();
        let task = Arc::clone(req);
        self.get_directory(
            &url.clone(),
            req,
            directory_offset,
            directory_length,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if error_ptr.is_some() {
                    callback((0, 0), error_ptr);
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url]);
                // `find_tile`'s null entry, of length 0, is `None`: the crate
                // refuses a directory with an entry of length 0.
                let entry: Option<DirEntry> = {
                    let cache = this.directory_cache.lock();
                    let directory = &cache[&url][&format!("{url}|{directory_offset}|{directory_length}")];
                    directory.find_tile_id(tile_id).copied()
                };

                if let Some(entry) = entry {
                    if entry.run_length() > 0 {
                        callback((header.data_offset().wrapping_add(entry.offset()), entry.length()), None);
                        return;
                    }

                    this.get_tile_address(
                        &url,
                        &task,
                        tile_id,
                        header.leaf_offset().wrapping_add(entry.offset()),
                        entry.length(),
                        directory_depth + 1,
                        Arc::clone(&callback),
                    );
                    return;
                }

                callback((0, 0), None);
            }),
        );
    }
}

impl State {
    /// Generate a tilejson resource from .pmtiles file
    fn request_tilejson(self: &Arc<Self>, req: &Task, resource: &Resource, r: &Responder) {
        let url = extract_url(&resource.url);

        let (this, r, url2) = (Arc::downgrade(self), r.clone(), url.clone());
        self.get_metadata(
            &url,
            req,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                let mut response = Response::new();

                if error_ptr.is_some() {
                    response.error = error_ptr;
                    r.send(response);
                    return;
                }

                let metadata = this.metadata_cache.lock()[&url2].clone();
                response.data = Some(Arc::new(metadata.into_bytes()));
                r.send(response);
            }),
        );
    }

    /// Load data for specific tile
    fn request_tile(self: &Arc<Self>, req: &Task, resource: &Resource, r: &Responder) {
        let url = extract_url(&resource.url);
        // C++ dereferences `resource.tileData` unchecked.
        let Some(tile_data) = resource.tile_data.clone() else {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "tile resource without tile data")));
            r.send(response);
            return;
        };

        let (this, r, url2) = (Arc::downgrade(self), r.clone(), url.clone());
        let task = Arc::clone(req);
        self.get_header(
            &url,
            req,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if error_ptr.is_some() {
                    let mut response = Response::new();
                    response.no_content = true;
                    response.error = error_ptr;
                    r.send(response);
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url2]);

                // `std::cmp_less(z, min_zoom) || std::cmp_greater(z, max_zoom)`.
                let z = i32::from(tile_data.z);
                if z < i32::from(header.min_zoom) || z > i32::from(header.max_zoom) {
                    let mut response = Response::new();
                    response.no_content = true;
                    r.send(response);
                    return;
                }

                let tile_id = match zxy_to_tileid(tile_data.z as u8, tile_data.x as u32, tile_data.y as u32) {
                    Ok(tile_id) => tile_id,
                    Err(e) => {
                        let mut response = Response::new();
                        response.no_content = true;
                        response.error = Some(Box::new(error(Reason::Other, format!("Invalid tile: {e}"))));
                        r.send(response);
                        return;
                    }
                };

                let (this2, r, url, tile_task) = (Arc::downgrade(&this), r.clone(), url2.clone(), Arc::clone(&task));
                this.get_tile_address(
                    &url2,
                    &task,
                    tile_id,
                    header.root_offset(),
                    header.root_length() as u32,
                    0,
                    Arc::new(move |tile_address: (u64, u32), tile_error: ErrorPtr| {
                        let Some(this2) = this2.upgrade() else { return };
                        if tile_error.is_some() {
                            let mut response = Response::new();
                            response.no_content = true;
                            response.error = tile_error;
                            r.send(response);
                            return;
                        }

                        if tile_address.0 == 0 && tile_address.1 == 0 {
                            let mut response = Response::new();
                            response.no_content = true;
                            r.send(response);
                            return;
                        }

                        let mut tile_resource = Resource::new(Kind::Source, url.clone());
                        tile_resource.loading_method = LoadingMethod::All;
                        tile_resource.data_range = Some((
                            tile_address.0,
                            tile_address.0.wrapping_add(u64::from(tile_address.1)).wrapping_sub(1),
                        ));

                        let (r, header) = (r.clone(), Arc::clone(&header));
                        this2.request(&tile_task, &tile_resource, move |tile_response: Response| {
                            let mut response = Response::new();
                            response.no_content = true;

                            if let Some(e) = &tile_response.error {
                                response.error = Some(Box::new(error(
                                    e.reason,
                                    format!("Error fetching PMTiles tile: {}", e.message),
                                )));
                                r.send(response);
                                return;
                            }

                            if tile_response.not_modified {
                                return;
                            }

                            let Some(data) = &tile_response.data else {
                                response.error = Some(Box::new(error(
                                    Reason::Other,
                                    "Error fetching PMTiles tile: response has no data",
                                )));
                                r.send(response);
                                return;
                            };

                            response.data = Some(Arc::clone(data));
                            response.no_content = false;
                            response.modified = tile_response.modified;
                            response.expires = tile_response.expires;
                            response.etag = tile_response.etag.clone();

                            if header.tile_compression == Compression::Gzip {
                                // Support uncompressed tiles
                                if is_compressed(data) {
                                    match decompress(data) {
                                        Ok(decompressed) => response.data = Some(Arc::new(decompressed)),
                                        Err(e) => {
                                            response.error = Some(Box::new(error(
                                                Reason::Other,
                                                format!("Error decompressing PMTiles tile: {e}"),
                                            )));
                                        }
                                    }
                                } else {
                                    response.data = Some(Arc::clone(data));
                                }
                            }

                            r.send(response);
                        });
                    }),
                );
            }),
        );
    }
}

/// The `Pmtiles` file source: `pmtiles://` URLs of local or remote archives.
pub(crate) struct PMTilesFileSource {
    state: Arc<State>,
    pool: SchedulerPtr,
}

impl PMTilesFileSource {
    /// A PMTiles file source that reads archives through `archives`, and
    /// starts its requests on `pool`.
    #[must_use]
    pub(crate) fn new(archives: Arc<dyn FileSource>, pool: SchedulerPtr) -> Self {
        PMTilesFileSource {
            state: Arc::new(State {
                file_source: archives,
                header_cache: Mutex::new(VecMap::new()),
                metadata_cache: Mutex::new(VecMap::new()),
                directory_cache: Mutex::new(VecMap::new()),
                directory_cache_control: Mutex::new(VecMap::new()),
            }),
            pool,
        }
    }
}

impl FileSource for PMTilesFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let task: Task = Arc::default();
        let cancelled = Arc::clone(&task);
        responder.on_cancel(move || drop(cancelled.lock().take()));
        let (state, resource) = (Arc::clone(&self.state), resource.clone());
        self.pool.schedule(Box::new(move || {
            if responder.is_cancelled() {
                return;
            }
            // assume if there is a tile request, that the pmtiles file has
            // been validated
            if resource.kind == Kind::Tile {
                state.request_tile(&task, &resource, &responder);
            } else {
                // return TileJSON
                state.request_tilejson(&task, &resource, &responder);
            }
        }));
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}
