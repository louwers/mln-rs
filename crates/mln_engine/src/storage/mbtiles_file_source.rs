// The database work runs on a thread pool. Needs the `sqlite` feature.
//
// A database that doesn't open or query and data that doesn't decompress
// answer the request with an `Other` error carrying the message, as in
// maplibre-native-ffi's patched C++.

use std::sync::Arc;

use mln_util::chrono::Timestamp;
use mln_util::compression::{decompress, is_compressed};
use mln_util::constants::MBTILES_PROTOCOL;
use mln_util::pool::Pool;
use mln_vec_map::VecMap;
use parking_lot::Mutex;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};

use crate::storage::archive_tilejson::archive_tilejson;
use crate::storage::file_source::FileSource;
use crate::storage::local_file_request::path_from_utf8;
use crate::storage::request::Responder;
use crate::storage::resource::{Kind, Resource};
use crate::storage::response::{Reason, ResourceError, Response};

fn accepts_url(url: &str) -> bool {
    url.starts_with(MBTILES_PROTOCOL)
}

fn url_to_path(url: &str) -> Vec<u8> {
    percent_encoding::percent_decode_str(&url[MBTILES_PROTOCOL.len().min(url.len())..]).collect()
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

fn open_read_only(path: &std::path::Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
        .map_err(|e| e.to_string())
}

/// A metadata value as text. The MBTiles spec makes values TEXT, but a
/// file's metadata table may store numbers.
fn metadata_text(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(r) => r.to_string(),
        ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t).into_owned(),
    }
}

fn error_response(reason: Reason, message: impl Into<String>) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(reason, message)));
    response
}

/// The open databases, so that several `.mbtiles` maps load at once.
#[derive(Default)]
struct Databases {
    db_cache: Mutex<VecMap<Vec<u8>, Connection>>,
}

impl Databases {
    /// The TileJSON of an `.mbtiles` file.
    fn request_tilejson(resource: &Resource) -> Response {
        match Self::tilejson(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        }
    }

    fn tilejson(resource: &Resource) -> Result<Response, String> {
        let path = url_to_path(&resource.url);
        let path = path_from_utf8(&path)?;
        let conn = open_read_only(&path)?;

        // The metadata table's rows as strings, and the members of its `json`
        // row (such as `vector_layers`) on their own.
        let mut rows = Map::new();
        let mut json = Map::new();
        {
            let mut meta = conn.prepare("SELECT name, value FROM metadata").map_err(|e| e.to_string())?;
            let mut query = meta.query([]).map_err(|e| e.to_string())?;
            while let Some(row) = query.next().map_err(|e| e.to_string())? {
                let name = metadata_text(row.get_ref(0).map_err(|e| e.to_string())?);
                let value = metadata_text(row.get_ref(1).map_err(|e| e.to_string())?);
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
            let (min, max) = conn
                .query_row("SELECT MIN(zoom_level), MAX(zoom_level) FROM tiles", [], |row| {
                    Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
                })
                .map_err(|e| e.to_string())?;
            let zoom = |z: Option<i64>| z.and_then(|z| u8::try_from(z).ok());
            minzoom = minzoom.or(zoom(min));
            maxzoom = maxzoom.or(zoom(max));
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
        if matches!(format, "mlt" | "application/vnd.maplibre-tile" | "application/vnd.maplibre-vector-tile") {
            members.push(("encoding", json!("mlt")));
        }
        let tilejson = archive_tilejson(members, rows.into_iter().chain(json));

        let mut response = Response::new();
        response.data = Some(Arc::new(tilejson.into_bytes()));
        Ok(response)
    }

    /// A tile's data.
    fn request_tile(&self, resource: &Resource) -> Response {
        match self.tile(resource) {
            Ok(response) => response,
            Err(message) => error_response(Reason::Other, message),
        }
    }

    fn tile(&self, resource: &Resource) -> Result<Response, String> {
        // The etag is the tile's URL, so a matching one means the loader
        // already has this tile: skip the re-read and its re-decode.
        if resource.prior_etag.as_deref() == Some(resource.url.as_str()) {
            let mut response = Response::new();
            response.not_modified = true;
            response.caching.expires = Some(Timestamp::from_secs(i64::MAX));
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

        let mut db_cache = self.db_cache.lock();
        if !db_cache.contains_key(&path) {
            let file = path_from_utf8(&path)?;
            db_cache.insert(path.clone(), open_read_only(&file)?);
        }
        let conn = &db_cache[&path];
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let data = row.get_ref(0).and_then(|value| Ok(value.as_bytes_or_null()?)).map_err(|e| e.to_string())?;
            if let Some(data) = data {
                let mut data = data.to_vec();
                response.no_content = false;
                // `Timestamp::max()`.
                response.caching.expires = Some(Timestamp::from_secs(i64::MAX));
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

/// The `Mbtiles` file source: `mbtiles://` URLs with absolute paths, read
/// on a thread pool.
pub(crate) struct MBTilesFileSource {
    databases: Arc<Databases>,
    pool: Pool,
}

impl MBTilesFileSource {
    /// Reads the databases on `pool`.
    #[must_use]
    pub(crate) fn new(pool: Pool) -> Self {
        MBTilesFileSource { databases: Arc::default(), pool }
    }

    /// Answers `responder` with `read`'s response, on the pool.
    fn read(&self, responder: Responder, read: impl FnOnce(&Databases) -> Response + Send + 'static) {
        let databases = Arc::clone(&self.databases);
        self.pool.spawn(move || {
            if !responder.is_cancelled() {
                responder.send(read(&databases));
            }
        });
    }
}

impl FileSource for MBTilesFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let resource = resource.clone();
        // assume if there is a tile request, that the mbtiles file has been
        // validated
        if resource.kind == Kind::Tile {
            self.read(responder, move |databases| databases.request_tile(&resource));
            return;
        }

        let path = resource.url.split_once("://").map(|(_, path)| path);
        if !path.is_some_and(|path| std::path::Path::new(path).is_absolute()) {
            let mut response = Response::new();
            response.no_content = true;
            response.error =
                Some(Box::new(ResourceError::new(Reason::Other, "MBTilesFileSource only supports absolute path urls")));
            responder.send(response);
            return;
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
            responder.send(response);
            return;
        }

        // return TileJSON
        self.read(responder, move |_| Databases::request_tilejson(&resource));
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}
