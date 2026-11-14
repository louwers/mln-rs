// The database work runs on the "MBTilesFileSource" thread (`util::Thread`)
// as in C++. Needs the `sqlite` feature.
//
// A database that doesn't open or query and data that doesn't decompress
// answer the request with an `Other` error carrying the message, as in
// maplibre-native-ffi's patched C++. The resource options are kept
// next to the thread behind a mutex (C++: in the thread object, read back
// with a blocking `ask`).

use std::cell::RefCell;
use std::sync::Arc;

use mln_actor::{ActorRef, Thread};
use mln_util::chrono::Timestamp;
use mln_util::compression::{decompress, is_compressed};
use mln_util::constants::MBTILES_PROTOCOL;
use mln_util::url::percent_decode;
use mln_vec_map::VecMap;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::archive_tilejson::archive_tilejson;
use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::local_file_request::path_from_utf8;
use crate::resource::{Kind, Resource};
use crate::resource_options::ResourceOptions;
use crate::response::{Reason, ResourceError, Response};
use crate::sqlite3::{Database, OpenFlag, Query, Statement};

fn accepts_url(url: &str) -> bool {
    url.starts_with(MBTILES_PROTOCOL)
}

fn url_to_path(url: &str) -> Vec<u8> {
    percent_decode(&url[MBTILES_PROTOCOL.len().min(url.len())..])
}

/// The `N` comma-separated numbers of a metadata value such as `bounds`.
fn numbers<const N: usize>(value: &str) -> Option<[f64; N]> {
    let numbers: Vec<f64> = value
        .split(',')
        .map(|number| number.trim().parse().ok().filter(|number: &f64| number.is_finite()))
        .collect::<Option<_>>()?;
    numbers.try_into().ok()
}

fn db_path(path: &[u8]) -> &[u8] {
    match path.iter().position(|&b| b == b'?') {
        Some(i) => &path[..i],
        None => path,
    }
}

fn error_response(reason: Reason, message: impl Into<String>) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(reason, message)));
    response
}

pub(crate) struct MBTilesFileSourceThread {
    // Multiple databases open simultaneously, to effectively support
    // multiple .mbtiles maps
    db_cache: RefCell<VecMap<Vec<u8>, Database>>,
}

impl MBTilesFileSourceThread {
    /// Generate a tilejson resource from .mbtiles file
    #[expect(clippy::unused_self, reason = "the file source's actor calls it on its thread's object")]
    pub(crate) fn request_tilejson(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        let response = match Self::tilejson(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        };
        req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
    }

    fn tilejson(resource: &Resource) -> Result<Response, String> {
        let path = url_to_path(&resource.url);
        let path = path_from_utf8(&path)?;
        let db =
            Database::open(path.to_str().ok_or("Invalid UTF-8 in path")?, OpenFlag::ReadOnly).map_err(|e| e.message)?;
        let conn = db.conn();

        // The metadata table's rows as strings, and the members of its `json`
        // row (such as `vector_layers`) on their own.
        let mut rows = Map::new();
        let mut json = Map::new();
        {
            let mut meta = Statement::new(&conn, "SELECT name, value FROM metadata").map_err(|e| e.message)?;
            let mut q = Query::new(&mut meta);
            while q.run().map_err(|e| e.message)? {
                let name = q.get_string(0).map_err(|e| e.message)?;
                let value = q.get_string(1).map_err(|e| e.message)?;
                if name == "json" {
                    if let Ok(Value::Object(members)) = serde_json::from_str(&value) {
                        json = members;
                    }
                } else {
                    rows.insert(name, Value::String(value));
                }
            }
        }
        let row = |name: &str| rows.get(name).and_then(Value::as_str);

        let format = row("format").unwrap_or("png");
        let zoom = |name: &str| row(name).and_then(|zoom| zoom.trim().parse::<u8>().ok());
        let (mut minzoom, mut maxzoom) = (zoom("minzoom"), zoom("maxzoom"));
        if minzoom.is_none() || maxzoom.is_none() {
            // The tiles' own zoom range, where the metadata has no valid one.
            let mut zooms =
                Statement::new(&conn, "SELECT MIN(zoom_level), MAX(zoom_level) FROM tiles").map_err(|e| e.message)?;
            let mut q = Query::new(&mut zooms);
            if q.run().map_err(|e| e.message)? {
                let zoom = |i| {
                    Ok::<_, String>(q.get_optional_i64(i).map_err(|e| e.message)?.and_then(|z| u8::try_from(z).ok()))
                };
                minzoom = minzoom.or(zoom(0)?);
                maxzoom = maxzoom.or(zoom(1)?);
            }
        }

        // The tile request carries the tile's coordinates in a query parameter
        // of the file's URL.
        let mut members = vec![("tiles", json!([format!("{}?file={{x}}/{{y}}/{{z}}.{format}", resource.url)]))];
        members.extend(minzoom.map(|zoom| ("minzoom", json!(zoom))));
        members.extend(maxzoom.map(|zoom| ("maxzoom", json!(zoom))));
        members.extend(row("bounds").and_then(numbers::<4>).map(|bounds| ("bounds", json!(bounds))));
        // TileJSON's center zoom is an integer.
        let center = row("center").and_then(numbers::<3>);
        members.extend(center.map(|[longitude, latitude, zoom]| ("center", json!([longitude, latitude, zoom as u8]))));
        if format == "mlt" || format == "application/vnd.maplibre-vector-tile" {
            members.push(("encoding", json!("mlt")));
        }
        let tilejson = archive_tilejson(members, rows.into_iter().chain(json));

        let mut response = Response::new();
        response.data = Some(Arc::new(tilejson.into_bytes()));
        Ok(response)
    }

    /// Load data for specific tile
    pub(crate) fn request_tile(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        let response = match self.tile(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        };
        req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
    }

    fn tile(&self, resource: &Resource) -> Result<Response, String> {
        // The etag is the tile's URL, so a matching one means the loader
        // already has this tile: skip the re-read and its re-decode.
        if resource.prior_etag.as_deref() == Some(resource.url.as_str()) {
            let mut response = Response::new();
            response.not_modified = true;
            response.expires = Some(Timestamp::from_secs(i64::MAX));
            response.etag = Some(resource.url.clone());
            return Ok(response);
        }

        let base_path = url_to_path(&resource.url);
        let path = db_path(&base_path).to_vec();

        // C++ dereferences `resource.tileData` unchecked.
        let tile_data = resource.tile_data.as_ref().ok_or("tile resource without tile data")?;
        let iy = tile_data.y;
        let iz = i32::from(tile_data.z as u8);

        let x = tile_data.x.to_string();
        let y = ((2f64.powi(iz) - 1.0) as i32 - iy).to_string();
        let z = iz.to_string();

        let sql =
            format!("SELECT tile_data FROM tiles where zoom_level = {z} AND tile_column = {x} AND tile_row = {y}");

        let mut response = Response::new();
        response.no_content = true;

        let mut db_cache = self.db_cache.borrow_mut();
        if !db_cache.contains_key(&path) {
            let file = path_from_utf8(&path)?;
            let db = Database::open(file.to_str().ok_or("Invalid UTF-8 in path")?, OpenFlag::ReadOnly)
                .map_err(|e| e.message)?;
            db_cache.insert(path.clone(), db);
        }
        let db = &db_cache[&path];
        let conn = db.conn();
        let mut stmt = Statement::new(&conn, &sql).map_err(|e| e.message)?;
        let mut q = Query::new(&mut stmt);
        while q.run().map_err(|e| e.message)? {
            // `q.get<std::optional<std::string>>(0)`.
            let data = q.get_optional_bytes(0).map_err(|e| e.message)?;
            if let Some(data) = data {
                let mut data = data;
                response.no_content = false;
                // `Timestamp::max()`.
                response.expires = Some(Timestamp::from_secs(i64::MAX));
                response.etag = Some(resource.url.clone());

                if is_compressed(&data) {
                    data = decompress(&data)?;
                }
                response.data = Some(Arc::new(data));
            }
        }
        Ok(response)
    }
}

/// The `Mbtiles` file source: `mbtiles://` URLs with absolute paths.
pub(crate) struct MBTilesFileSource {
    thread: Thread<MBTilesFileSourceThread>,
    resource_options: Mutex<ResourceOptions>,
}

impl MBTilesFileSource {
    #[must_use]
    pub(crate) fn new(resource_options: &ResourceOptions) -> Self {
        MBTilesFileSource {
            thread: Thread::new("MBTilesFileSource", |_| MBTilesFileSourceThread {
                db_cache: RefCell::new(VecMap::new()),
            }),
            resource_options: Mutex::new(resource_options.clone()),
        }
    }
}

impl FileSource for MBTilesFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let req = FileSourceRequest::new(callback);
        let req_ref = req.actor();

        // assume if there is a tile request, that the mbtiles file has been
        // validated
        if resource.kind == Kind::Tile {
            let resource = resource.clone();
            self.thread.actor().invoke(move |t: &MBTilesFileSourceThread| t.request_tile(&resource, &req_ref));
            return Box::new(req);
        }

        let scheme_end = resource.url.find("://");
        if scheme_end.is_none() || !std::path::Path::new(&resource.url[scheme_end.unwrap() + 3..]).is_absolute() {
            let mut response = Response::new();
            response.no_content = true;
            response.error =
                Some(Box::new(ResourceError::new(Reason::Other, "MBTilesFileSource only supports absolute path urls")));
            req_ref.invoke(move |request: &FileSourceRequestState| request.set_response(response));
            return Box::new(req);
        }

        // file must exist
        let path = url_to_path(&resource.url);
        let not_found = match path_from_utf8(&path) {
            Ok(file) => matches!(std::fs::metadata(file), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
            Err(_) => false,
        };
        if not_found {
            let mut response = Response::new();
            response.no_content = true;
            response.error = Some(Box::new(ResourceError::new(
                Reason::NotFound,
                format!("path not found: {}", String::from_utf8_lossy(&path)),
            )));
            req_ref.invoke(move |request: &FileSourceRequestState| request.set_response(response));
            return Box::new(req);
        }

        // return TileJSON
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &MBTilesFileSourceThread| t.request_tilejson(&resource, &req_ref));
        Box::new(req)
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.resource_options.lock() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock().clone()
    }
}
