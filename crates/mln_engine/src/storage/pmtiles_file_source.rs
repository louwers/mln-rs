// A request starts on a thread pool: header, metadata and directories are
// read with range requests through the file source that loads the archives,
// and each step continues on the thread that answers the one before. A
// request's sub-request replaces the one before, and goes when the request
// is cancelled.
//
// The first read takes the archive's first 16 KiB, which hold the header and
// the root directory. A cache-only request reads only the cache, and a miss
// at any step answers that the cache has nothing.

use std::fmt::Write as _;
use std::sync::Arc;

use bytes::Bytes;
use mln_vec_map::VecMap;
use parking_lot::Mutex;
use pmtiles::{Compression, DirEntry, Directory, Header, PmtError, TileCoord, TileId, TileType};
use serde_json::{Value as Json, json};

use crate::exec::pool::Pool;
use crate::storage::archive_tilejson::archive_tilejson;
use crate::storage::compression::{decompress, is_compressed};
use crate::storage::constants::{FILE_PROTOCOL, PMTILES_PROTOCOL};
use crate::storage::file_source::FileSource;
use crate::storage::request::{Request, Responder};
use crate::storage::resource::{Kind, LoadingMethod, Resource};
use crate::storage::response::{Reason, ResourceError, Response};

// https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md#3-header
const PMTILES_HEADER_OFFSET: u64 = 0;
const PMTILES_HEADER_LENGTH: u64 = 127;
/// The bytes that hold the header and the root directory: the spec keeps
/// both in the first 16 KiB.
const PMTILES_FIRST_READ: u64 = 16_384;

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
/// Called with a directory of the archive, or the error that kept it.
type DirectoryCallback = Arc<dyn Fn(Result<Arc<Directory>, Box<ResourceError>>) + Send + Sync>;
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
    /// Each archive's directories, by its URL.
    directory_cache: Mutex<VecMap<String, Directories>>,
}

/// The directories of an archive that the cache holds, by their place in
/// it, and the order they go in when it's full: least recently used first,
/// each once.
#[derive(Default)]
struct Directories {
    by_key: VecMap<String, Arc<Directory>>,
    order: Vec<String>,
}

impl Directories {
    /// Directory `key`, which becomes the most recently used.
    fn get(&mut self, key: &str) -> Option<Arc<Directory>> {
        let directory = Arc::clone(self.by_key.get(key)?);
        if let Some(position) = self.order.iter().position(|held| held == key) {
            let key = self.order.remove(position);
            self.order.push(key);
        }
        Some(directory)
    }

    /// Stores `directory` as `key`, unless one is there already, and
    /// returns the one held, the most recently used: the least recently
    /// used goes once the cache is full.
    fn store(&mut self, key: String, directory: Directory) -> Arc<Directory> {
        if let Some(held) = self.get(&key) {
            return held;
        }
        let directory = Arc::new(directory);
        self.by_key.insert(key.clone(), Arc::clone(&directory));
        self.order.push(key);
        if self.order.len() > MAX_DIRECTORY_CACHE_ENTRIES {
            let oldest = self.order.remove(0);
            self.by_key.remove(&oldest);
        }
        directory
    }
}

/// Answers `r` with `response`: for a cache-only request, an error at any
/// step means that the cache lacks what the request needs.
fn answer(r: &Responder, cache_only: bool, response: Response) {
    if cache_only && response.error.is_some() {
        let mut nothing = Response::new();
        nothing.no_content = true;
        r.send(nothing);
    } else {
        r.send(response);
    }
}

/// How a request's reads load: only from the cache for a cache-only
/// request, else from the cache and the network.
fn loading_method(cache_only: bool) -> LoadingMethod {
    if cache_only { LoadingMethod::CacheOnly } else { LoadingMethod::All }
}

impl State {
    /// Requests `resource` for `task`, in place of its sub-request before.
    fn request(&self, req: &Task, resource: &Resource, callback: impl FnMut(Response) + Send + 'static) {
        let (request, responder) = Request::new(callback);
        let previous = req.lock().replace(request);
        drop(previous);
        self.file_source.load(resource, responder);
    }

    fn get_header(self: &Arc<Self>, url: &str, req: &Task, cache_only: bool, callback: AsyncCallback) {
        if self.header_cache.lock().contains_key(url) {
            callback(None);
            return;
        }

        let mut resource = Resource::new(Kind::Source, url);
        resource.loading_method = loading_method(cache_only);
        resource.data_range = Some((PMTILES_HEADER_OFFSET, PMTILES_HEADER_OFFSET + PMTILES_FIRST_READ - 1));

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

            let header =
                deserialize_header(&data[..data.len().min(PMTILES_HEADER_LENGTH as usize)]).and_then(|header| {
                    let supported = |c: Compression| c == Compression::None || c == Compression::Gzip;
                    if !supported(header.internal_compression()) || !supported(header.tile_compression) {
                        return Err("Compression method not supported".to_owned());
                    }
                    Ok(header)
                });
            match header {
                Ok(header) => {
                    this.store_root_directory(&url, &header, data);
                    this.header_cache.lock().entry(url.clone()).or_insert_with(|| Arc::new(header));
                    callback(None);
                }
                Err(e) => callback(Some(Box::new(error(Reason::Other, format!("Error parsing PMTiles header: {e}"))))),
            }
        });
    }

    /// Stores the root directory of the archive at `url`, whose header is
    /// `header`, from `first`, the archive's first bytes, if they hold it.
    /// One that doesn't parse is left for its own read, which reports why.
    fn store_root_directory(&self, url: &str, header: &Header, first: &[u8]) {
        let (offset, length) = (header.root_offset(), header.root_length());
        let Some(end) = offset.checked_add(length).filter(|&end| end <= first.len() as u64) else { return };
        let mut directory = first[offset as usize..end as usize].to_vec();
        if header.internal_compression() == Compression::Gzip {
            let Ok(decompressed) = decompress(&directory) else { return };
            directory = decompressed;
        }
        let _ = self.store_directory(url, offset, length, directory);
    }

    fn get_metadata(self: &Arc<Self>, url: &str, req: &Task, cache_only: bool, callback: AsyncCallback) {
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
            cache_only,
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
                    resource.loading_method = loading_method(cache_only);
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

    /// Stores the directory of the archive at `url` that `directory_data`
    /// holds, and returns the one the cache holds there.
    fn store_directory(
        &self,
        url: &str,
        directory_offset: u64,
        directory_length: u64,
        directory_data: Vec<u8>,
    ) -> Result<Arc<Directory>, String> {
        let directory_cache_key = format!("{url}|{directory_offset}|{directory_length}");
        // Unlike C++, the crate refuses a directory with an entry of length 0
        // (C++ reads it, and the entry finds no tile), a first entry whose
        // offset continues a previous one, or a run past the last tile ID,
        // and it reads a directory with bytes after its entries, which C++
        // refuses. Its errors have the crate's texts.
        let directory = Directory::try_from(Bytes::from(directory_data)).map_err(|e| e.to_string())?;

        Ok(self.directory_cache.lock().entry(url.to_owned()).or_default().store(directory_cache_key, directory))
    }

    fn get_directory(
        self: &Arc<Self>,
        url: &str,
        req: &Task,
        directory_offset: u64,
        directory_length: u32,
        cache_only: bool,
        callback: DirectoryCallback,
    ) {
        let directory_cache_key = format!("{url}|{directory_offset}|{directory_length}");

        let cached = self.directory_cache.lock().get_mut(url).and_then(|cache| cache.get(&directory_cache_key));
        if let Some(directory) = cached {
            callback(Ok(directory));
            return;
        }

        let this = Arc::downgrade(self);
        let url = url.to_owned();
        let task = Arc::clone(req);
        self.get_header(
            &url.clone(),
            req,
            cache_only,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if let Some(e) = error_ptr {
                    callback(Err(e));
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url]);

                let mut resource = Resource::new(Kind::Source, url.clone());
                resource.loading_method = loading_method(cache_only);
                resource.data_range = Some((
                    directory_offset,
                    directory_offset.wrapping_add(u64::from(directory_length)).wrapping_sub(1),
                ));

                let (this2, url, callback) = (Arc::downgrade(&this), url.clone(), Arc::clone(&callback));
                this.request(&task, &resource, move |response: Response| {
                    let Some(this2) = this2.upgrade() else { return };
                    if let Some(e) = &response.error {
                        callback(Err(Box::new(error(
                            e.reason,
                            format!("Error fetching PMTiles directory: {}", e.message),
                        ))));

                        return;
                    }

                    if response.not_modified {
                        return;
                    }

                    let Some(data) = &response.data else {
                        callback(Err(Box::new(error(Reason::Other, "PMTiles directory response has no data"))));
                        return;
                    };

                    let result = (|| -> Result<Arc<Directory>, String> {
                        let mut directory_data: Vec<u8> = data.to_vec();

                        if header.internal_compression() == Compression::Gzip {
                            directory_data = decompress(&directory_data)?;
                        }

                        this2.store_directory(&url, directory_offset, u64::from(directory_length), directory_data)
                    })();
                    callback(
                        result.map_err(|e| {
                            Box::new(error(Reason::Other, format!("Error parsing PMTiles directory: {e}")))
                        }),
                    );
                });
            }),
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the request, tile and directory location of one step of the directory walk, how to read it, and its callback"
    )]
    fn get_tile_address(
        self: &Arc<Self>,
        url: &str,
        req: &Task,
        tile_id: TileId,
        directory_offset: u64,
        directory_length: u32,
        directory_depth: u32,
        cache_only: bool,
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
            cache_only,
            Arc::new(move |directory: Result<Arc<Directory>, Box<ResourceError>>| {
                let Some(this) = this.upgrade() else { return };
                let directory = match directory {
                    Ok(directory) => directory,
                    Err(e) => {
                        callback((0, 0), Some(e));
                        return;
                    }
                };

                let header = Arc::clone(&this.header_cache.lock()[&url]);
                // `find_tile`'s null entry, of length 0, is `None`: the crate
                // refuses a directory with an entry of length 0.
                let entry: Option<DirEntry> = directory.find_tile_id(tile_id).copied();

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
                        cache_only,
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
        let cache_only = resource.loading_method == LoadingMethod::CacheOnly;

        let (this, r, url2) = (Arc::downgrade(self), r.clone(), url.clone());
        self.get_metadata(
            &url,
            req,
            cache_only,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                let mut response = Response::new();

                if error_ptr.is_some() {
                    response.error = error_ptr;
                    answer(&r, cache_only, response);
                    return;
                }

                let metadata = this.metadata_cache.lock()[&url2].clone();
                response.data = Some(Arc::new(metadata.into_bytes()));
                answer(&r, cache_only, response);
            }),
        );
    }

    /// Load data for specific tile
    fn request_tile(self: &Arc<Self>, req: &Task, resource: &Resource, r: &Responder) {
        let url = extract_url(&resource.url);
        let cache_only = resource.loading_method == LoadingMethod::CacheOnly;
        // C++ dereferences `resource.tileData` unchecked.
        let Some(tile_data) = resource.tile_data.clone() else {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "tile resource without tile data")));
            answer(r, cache_only, response);
            return;
        };

        let (this, r, url2) = (Arc::downgrade(self), r.clone(), url.clone());
        let task = Arc::clone(req);
        self.get_header(
            &url,
            req,
            cache_only,
            Arc::new(move |error_ptr: ErrorPtr| {
                let Some(this) = this.upgrade() else { return };
                if error_ptr.is_some() {
                    let mut response = Response::new();
                    response.no_content = true;
                    response.error = error_ptr;
                    answer(&r, cache_only, response);
                    return;
                }

                let header = Arc::clone(&this.header_cache.lock()[&url2]);

                // `std::cmp_less(z, min_zoom) || std::cmp_greater(z, max_zoom)`.
                let z = i32::from(tile_data.z);
                if z < i32::from(header.min_zoom) || z > i32::from(header.max_zoom) {
                    let mut response = Response::new();
                    response.no_content = true;
                    answer(&r, cache_only, response);
                    return;
                }

                let tile_id = match zxy_to_tileid(tile_data.z as u8, tile_data.x as u32, tile_data.y as u32) {
                    Ok(tile_id) => tile_id,
                    Err(e) => {
                        let mut response = Response::new();
                        response.no_content = true;
                        response.error = Some(Box::new(error(Reason::Other, format!("Invalid tile: {e}"))));
                        answer(&r, cache_only, response);
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
                    cache_only,
                    Arc::new(move |tile_address: (u64, u32), tile_error: ErrorPtr| {
                        let Some(this2) = this2.upgrade() else { return };
                        if tile_error.is_some() {
                            let mut response = Response::new();
                            response.no_content = true;
                            response.error = tile_error;
                            answer(&r, cache_only, response);
                            return;
                        }

                        if tile_address.0 == 0 && tile_address.1 == 0 {
                            let mut response = Response::new();
                            response.no_content = true;
                            answer(&r, cache_only, response);
                            return;
                        }

                        let mut tile_resource = Resource::new(Kind::Source, url.clone());
                        tile_resource.loading_method = loading_method(cache_only);
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
                                answer(&r, cache_only, response);
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
                                answer(&r, cache_only, response);
                                return;
                            };

                            response.data = Some(Arc::clone(data));
                            response.no_content = false;
                            response.modified = tile_response.modified;
                            response.caching.expires = tile_response.caching.expires;
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

                            answer(&r, cache_only, response);
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
    pool: Pool,
}

impl PMTilesFileSource {
    /// A PMTiles file source that reads archives through `archives`, and
    /// starts its requests on `pool`.
    #[must_use]
    pub(crate) fn new(archives: Arc<dyn FileSource>, pool: Pool) -> Self {
        PMTilesFileSource {
            state: Arc::new(State {
                file_source: archives,
                header_cache: Mutex::new(VecMap::new()),
                metadata_cache: Mutex::new(VecMap::new()),
                directory_cache: Mutex::new(VecMap::new()),
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
        self.pool.spawn(move || {
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
        });
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}

#[cfg(test)]
mod tests {
    use mln_style::tileset::Scheme;

    use super::*;
    use crate::storage::local_file_request::read_file;
    use crate::storage::resource::TileFormat;

    /// Holds the archive's reads until the test answers them.
    #[derive(Default)]
    struct Held(Mutex<Vec<(Resource, Responder)>>);

    impl FileSource for Held {
        fn load(&self, resource: &Resource, responder: Responder) {
            self.0.lock().push((resource.clone(), responder));
        }

        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }

    impl Held {
        /// The reads waiting, as their ranges and loading methods.
        fn waiting(&self) -> Vec<((u64, u64), LoadingMethod)> {
            let held = self.0.lock();
            held.iter()
                .filter(|(_, r)| !r.is_cancelled())
                .map(|(res, _)| (res.data_range.unwrap(), res.loading_method))
                .collect()
        }

        /// Answers the reads waiting from the archive, or with `error`.
        fn answer(&self, error: Option<Reason>) {
            let held = std::mem::take(&mut *self.0.lock());
            for (resource, responder) in held {
                let mut response = Response::new();
                match error {
                    Some(reason) => response.error = Some(Box::new(ResourceError::new(reason, "down"))),
                    None => response.data = read_file(&path(), resource.data_range).map(Arc::new),
                }
                responder.send(response);
            }
        }
    }

    fn path() -> std::path::PathBuf {
        crate::storage::tests::cpp_path("test/fixtures/storage/pmtiles/geography-class-png.pmtiles")
    }

    type Answers = Arc<Mutex<Vec<Response>>>;

    /// Requests tile `x`/`y` of zoom 1, as `method`, of a source over `held`.
    fn request(source: &PMTilesFileSource, x: i32, y: i32, method: LoadingMethod) -> (Task, Request, Answers) {
        let url = format!("{PMTILES_PROTOCOL}{FILE_PROTOCOL}{}", path().display());
        let answers = Answers::default();
        let saved = Arc::clone(&answers);
        let (request, responder) = Request::new(move |response| saved.lock().push(response));
        let task = Task::default();
        source.state.request_tile(
            &task,
            &Resource::tile(&url, 1.0, x, y, 1, Scheme::XYZ, TileFormat::Image, method),
            &responder,
        );
        (task, request, answers)
    }

    fn source() -> (Arc<Held>, PMTilesFileSource) {
        let held = Arc::new(Held::default());
        (Arc::clone(&held), PMTilesFileSource::new(held, crate::storage::tests::pool()))
    }

    /// A tile reads the header and the root directory in one read of the
    /// archive's first 16 KiB, then its own range.
    #[test]
    fn a_tile_reads_the_header_and_root_directory_at_once() {
        let (held, source) = source();
        let (_task, _request, answers) = request(&source, 0, 0, LoadingMethod::All);
        assert_eq!(held.waiting(), [((0, 16_383), LoadingMethod::All)]);
        held.answer(None);
        let tile = held.waiting();
        assert!(matches!(tile[..], [((first, _), _)] if first > 16_383), "the tile's range, not a directory: {tile:?}");
        held.answer(None);
        assert!(answers.lock()[0].data.is_some());
    }

    /// A cache-only request reads only the cache, at every step, and a miss
    /// there answers that the cache has nothing, not an error.
    #[test]
    fn a_cache_only_request_reads_only_the_cache() {
        let (held, archive) = source();
        let (_task, _request, answers) = request(&archive, 0, 0, LoadingMethod::CacheOnly);
        assert_eq!(held.waiting(), [((0, 16_383), LoadingMethod::CacheOnly)]);
        held.answer(None);
        let tile = held.waiting();
        assert!(matches!(tile[..], [((first, _), LoadingMethod::CacheOnly)] if first > 16_383), "{tile:?}");
        held.answer(Some(Reason::NotFound));
        let answers = answers.lock();
        assert!(answers[0].no_content && answers[0].error.is_none());

        let (held, archive) = source();
        let (_task, _request, answers) = request(&archive, 0, 0, LoadingMethod::CacheOnly);
        let (_, responder) = held.0.lock().pop().unwrap();
        let mut miss = Response::new();
        miss.no_content = true;
        responder.send(miss);
        assert!(answers.lock()[0].no_content, "a miss of the header");
    }

    /// A directory stored again keeps its one place in the order, and the
    /// directory used most recently goes last.
    #[test]
    fn a_directory_goes_once_the_cache_has_used_the_others_since() {
        let (_, source) = source();
        let archive = std::fs::read(path()).unwrap();
        let header = deserialize_header(&archive[..PMTILES_HEADER_LENGTH as usize]).unwrap();
        let (offset, length) = (header.root_offset() as usize, header.root_length() as usize);
        let mut root = archive[offset..offset + length].to_vec();
        if header.internal_compression() == Compression::Gzip {
            root = decompress(&root).unwrap();
        }
        let state = &source.state;
        let store = |at: u64| state.store_directory("archive", at, 1, root.clone()).unwrap();
        let first = store(0);
        assert!(Arc::ptr_eq(&first, &store(0)), "stored again, it's the one held");
        store(1);
        for at in 2..MAX_DIRECTORY_CACHE_ENTRIES as u64 {
            store(at);
        }
        let mut cache = state.directory_cache.lock();
        let directories = cache.get_mut("archive").unwrap();
        assert!(directories.get("archive|0|1").is_some(), "stored twice, listed once");
        drop(cache);
        store(MAX_DIRECTORY_CACHE_ENTRIES as u64);
        let mut cache = state.directory_cache.lock();
        let directories = cache.get_mut("archive").unwrap();
        assert!(directories.get("archive|1|1").is_none(), "the least recently used went");
        assert!(directories.get("archive|0|1").is_some(), "used since, it stays");
    }
}
