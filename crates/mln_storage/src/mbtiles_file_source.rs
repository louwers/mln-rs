// Ports include/mln/storage/mbtiles_file_source.hpp and
// platform/default/src/mln/storage/mbtiles_file_source.cpp.
//
// The database work runs on the "MBTilesFileSource" thread (`util::Thread`)
// as in C++. The TileJSON is built as a rapidjson document would be (members
// in insertion order, duplicates kept; json_document.rs) and written with
// //vendor/rapidjson's `Writer`. Needs the `sqlite` feature.
//
// Deviations: C++ lets the exceptions of `Database::open`, a query,
// `std::stoi`/`std::stod` and `util::decompress` escape the file source
// thread (std::terminate); the port answers the request with an `Other`
// error carrying the message instead, and `std::stoi`/`std::stod` of a
// string without a number read 0. The resource and client options are kept
// next to the thread behind a mutex (C++: in the thread object, read back
// with a blocking `ask`).
//
// TODO(port): makeThreadPrioritySetter(EXPERIMENTAL_THREAD_PRIORITY_FILE).

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use mln_actor::{ActorRef, Thread};
use mln_util::chrono::Timestamp;
use mln_util::client_options::ClientOptions;
use mln_util::compression::{decompress, is_compressed};
use mln_util::constants::MBTILES_PROTOCOL;
use mln_util::sync::MutexExt;
use mln_util::url::percent_decode;
use mln_vec_map::VecMap;
use serde_json::Value as Json;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::json_document::{Document, strtol};
use crate::local_file_request::path_from_utf8;
use crate::resource::{Kind, Resource};
use crate::resource_options::ResourceOptions;
use crate::response::{Error, Reason, Response};
use crate::sqlite3::{Database, OpenFlag, Query, Statement};

fn accepts_url(url: &str) -> bool {
    url.starts_with(MBTILES_PROTOCOL)
}

fn url_to_path(url: &str) -> Vec<u8> {
    percent_decode(&url[MBTILES_PROTOCOL.len().min(url.len())..])
}

fn json_double(d: f64) -> Json {
    serde_json::Number::from_f64(d).map_or(Json::Null, Json::Number)
}

/// `std::strtod(s, nullptr)`: the longest prefix that is a number, after
/// leading whitespace; 0 when there is none.
pub(crate) fn strtod(s: &str) -> f64 {
    let s = s.trim_start();
    let mut best = 0.0;
    for (i, _) in s.char_indices().skip(1).chain(std::iter::once((s.len(), ' '))) {
        if let Ok(v) = s[..i].parse::<f64>() {
            best = v;
        }
    }
    best
}

/// `split(s, delim)`: each `std::getline` piece through `std::strtod`.
fn split(s: &str, delim: char) -> Vec<f64> {
    let mut elems: Vec<f64> = s.split(delim).map(strtod).collect();
    // `std::getline` yields no final empty piece after a trailing delimiter
    // (nor any piece for an empty string).
    if s.is_empty() || s.ends_with(delim) {
        elems.pop();
    }
    elems
}

fn db_path(path: &[u8]) -> &[u8] {
    match path.iter().position(|&b| b == b'?') {
        Some(i) => &path[..i],
        None => path,
    }
}

fn error_response(reason: Reason, message: impl Into<String>) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(Error::new(reason, message)));
    response
}

/// `MBTilesFileSource::Impl`.
pub struct MBTilesFileSourceThread {
    // Multiple databases open simultaneously, to effectively support
    // multiple .mbtiles maps
    db_cache: RefCell<VecMap<Vec<u8>, Database>>,
}

impl MBTilesFileSourceThread {
    /// Generate a tilejson resource from .mbtiles file
    pub fn request_tilejson(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        let response = match self.tilejson(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        };
        req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
    }

    fn tilejson(&self, resource: &Resource) -> Result<Response, String> {
        let path = url_to_path(&resource.url);
        let path = path_from_utf8(&path)?;

        let mut response = Response::new();

        let mut doc = Document::default();

        let mut values: VecMap<String, String> = VecMap::new();
        let db =
            Database::open(path.to_str().ok_or("Invalid UTF-8 in path")?, OpenFlag::ReadOnly).map_err(|e| e.message)?;
        let conn = db.conn();

        {
            let mut meta = Statement::new(&conn, "SELECT * from metadata").map_err(|e| e.message)?;
            let mut q = Query::new(&mut meta);
            while q.run().map_err(|e| e.message)? {
                let name = q.get_string(0).map_err(|e| e.message)?;
                let val = q.get_bytes(1).map_err(|e| e.message)?;
                if name == "json" {
                    doc = Document::parse_object(&val);
                } else {
                    values.insert(name, String::from_utf8_lossy(&val).into_owned());
                }
            }
        }

        values.insert("tilejson".to_owned(), "2.0.0".to_owned());
        values.insert("scheme".to_owned(), "xyz".to_owned());

        let format = values.get("format").cloned().unwrap_or_else(|| "png".to_owned());

        // Translate MIME type field to source encoding.
        if format == "mlt" || format == "application/vnd.maplibre-vector-tile" {
            doc.add_member("encoding", Json::String("mlt".to_owned()));
        }

        if format != "pbf" && !values.contains_key("scale") {
            values.insert("scale".to_owned(), "1".to_owned());
        }

        // We use file location with appended parameter query parameter as
        // URL for actual tile data
        let tile_url = format!("{}?file={{x}}/{{y}}/{{z}}.{format}", resource.url);
        doc.add_member("tiles", Json::Array(vec![Json::String(tile_url)]));

        let minz;
        let maxz;

        let minzoom = values.get("minzoom").cloned();
        let maxzoom = values.get("maxzoom").cloned();

        if let (Some(min), Some(max)) = (&minzoom, &maxzoom) {
            minz = min.clone();
            maxz = max.clone();
        } else {
            let mut zoom_stmt =
                Statement::new(&conn, "SELECT MIN(zoom_level),MAX(zoom_level) from tiles").map_err(|e| e.message)?;
            let mut q = Query::new(&mut zoom_stmt);
            let (mut min, mut max) = (String::new(), String::new());
            while q.run().map_err(|e| e.message)? {
                min = q.get_string(0).map_err(|e| e.message)?;
                max = q.get_string(1).map_err(|e| e.message)?;
            }
            minz = min;
            maxz = max;

            values.insert("minzoom".to_owned(), minz.clone());
            values.insert("maxzoom".to_owned(), maxz.clone());
        }

        let min_zoom = if minz.is_empty() { 0 } else { strtol(&minz) as i32 };
        let max_zoom = if maxz.is_empty() { 0 } else { strtol(&maxz) as i32 };

        for (name, value) in &values {
            if name == "scale" {
                doc.add_member("scale", json_double(strtod(value)));
            } else if name == "minzoom" || name == "maxzoom" {
                let value = if value.is_empty() { 0 } else { strtol(value) as i32 };
                doc.add_member(name.clone(), Json::from(value));
            } else if name == "bounds" {
                let bounds_values = split(value, ',');
                if bounds_values.len() != 4 {
                    response.error = Some(Box::new(Error::new(Reason::Other, "")));
                    return Ok(response);
                }
                let c_lon = (bounds_values[0] + bounds_values[2]) / 2.0;
                let c_lat = (bounds_values[1] + bounds_values[3]) / 2.0;
                let c_zoom = (min_zoom + max_zoom) / 2;

                if format != "pbf" {
                    doc.add_member("bounds", Json::Array(bounds_values.iter().map(|&v| json_double(v)).collect()));
                }

                doc.add_member("center", Json::Array(vec![json_double(c_lon), json_double(c_lat), Json::from(c_zoom)]));
            } else {
                doc.add_member(name.clone(), Json::String(value.clone()));
            }
        }

        response.data = Some(Arc::new(doc.serialize().into_bytes()));
        Ok(response)
    }

    /// Load data for specific tile
    pub fn request_tile(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        let response = match self.tile(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        };
        req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
    }

    fn tile(&self, resource: &Resource) -> Result<Response, String> {
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

/// `MBTilesFileSource`: the `Mbtiles` file source (`mbtiles://` URLs with
/// absolute paths).
pub struct MBTilesFileSource {
    thread: Thread<MBTilesFileSourceThread>,
    resource_options: Mutex<ResourceOptions>,
    client_options: Mutex<ClientOptions>,
}

impl MBTilesFileSource {
    /// `MBTilesFileSource(const ResourceOptions&, const ClientOptions&)`.
    pub fn new(resource_options: &ResourceOptions, client_options: &ClientOptions) -> Self {
        MBTilesFileSource {
            thread: Thread::new("MBTilesFileSource", |_| MBTilesFileSourceThread {
                db_cache: RefCell::new(VecMap::new()),
            }),
            resource_options: Mutex::new(resource_options.clone()),
            client_options: Mutex::new(client_options.clone()),
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
                Some(Box::new(Error::new(Reason::Other, "MBTilesFileSource only supports absolute path urls")));
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
            response.error = Some(Box::new(Error::new(
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
        *self.resource_options.lock_unpoisoned() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock_unpoisoned().clone()
    }

    fn set_client_options(&self, options: ClientOptions) {
        *self.client_options.lock_unpoisoned() = options;
    }

    fn get_client_options(&self) -> ClientOptions {
        self.client_options.lock_unpoisoned().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strtod_and_strtol_prefixes() {
        assert_eq!(1.5, strtod("1.5abc"));
        assert_eq!(-180.0, strtod(" -180"));
        assert_eq!(0.0, strtod("abc"));
        assert_eq!(12, strtol("12x"));
        assert_eq!(0, strtol(""));
        assert_eq!(vec![1.0, 2.0, 3.0, 4.0], split("1,2,3,4", ','));
        assert_eq!(vec![1.0, 2.0], split("1,2,", ','));
        assert!(split("", ',').is_empty());
    }
}
