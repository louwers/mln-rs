// Ports platform/default/include/mln/storage/offline_database.hpp and
// platform/default/src/mln/storage/offline_database.cpp.
//
// C++ throws and catches `mapbox::sqlite::Exception`, `util::IOException`,
// `MapboxTileLimitExceededException`, `std::runtime_error` and logic errors
// (`catch (...)`); here they are the `DatabaseError` variants, returned
// through `Result` and handled by `handle_error` as in C++. Methods that
// return a `std::exception_ptr` return `Option<DatabaseError>`; methods that
// return `expected<T, std::exception_ptr>` return `Result<T, DatabaseError>`.
// `putRegionResource(s)`, which let `MapboxTileLimitExceededException`
// escape, return it as `Err`.
//
// Prepared statements are rusqlite's per-connection statement cache
// (`prepare_cached`) instead of the `statements` map; both are dropped with
// the connection.

use std::sync::Arc;

use mln_util::chrono::{Timestamp, now};
use mln_util::compression::{compress, decompress};
use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;
use mln_util::logging::{Event, EventSeverity, Log};
use mln_util::mapbox::is_canonical_url;
use mln_util::tile_server_options::TileServerOptions;
use rusqlite::params;

use crate::merge_sideloaded::MERGE_SIDELOADED_DATABASE_SQL;
use crate::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionMetadata, OfflineRegionStatus, OfflineRegions,
    RegionDefinitionError, decode_offline_region_definition, encode_offline_region_definition,
};
use crate::offline_schema::OFFLINE_DATABASE_SCHEMA;
use crate::resource::{Kind, Resource, TileData};
use crate::response::Response;
use crate::sqlite3::{
    self, Database, Exception, OpenFlag, Transaction, TransactionMode, extended_result_code, get_bool, get_bytes,
    get_i64, get_optional_bytes, get_optional_i64, get_optional_string, result_code,
};

/// `util::mapbox::DEFAULT_OFFLINE_TILE_COUNT_LIMIT`.
pub const DEFAULT_OFFLINE_TILE_COUNT_LIMIT: u64 = 6000;

/// `MapboxTileLimitExceededException` (`util::Exception("Mapbox tile limit
/// exceeded")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapboxTileLimitExceededException;

impl std::fmt::Display for MapboxTileLimitExceededException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Mapbox tile limit exceeded")
    }
}

/// The exceptions the C++ code throws and `handleError` catches.
#[derive(Clone, Debug)]
pub enum DatabaseError {
    Sqlite(Exception),
    /// `util::IOException { code }` with its `what()`.
    Io {
        code: i32,
        message: String,
    },
    /// `MapboxTileLimitExceededException` (a `std::runtime_error`).
    MapboxTileLimitExceeded,
    /// `std::runtime_error`.
    Runtime(String),
    /// Exceptions that are not `std::runtime_error`s (`std::invalid_argument`,
    /// `std::domain_error`): `handleError`'s `catch (...)`.
    Logic(String),
}

impl From<Exception> for DatabaseError {
    fn from(e: Exception) -> Self {
        DatabaseError::Sqlite(e)
    }
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(e: rusqlite::Error) -> Self {
        DatabaseError::Sqlite(e.into())
    }
}

impl From<RegionDefinitionError> for DatabaseError {
    fn from(e: RegionDefinitionError) -> Self {
        match e {
            RegionDefinitionError::Runtime(message) => DatabaseError::Runtime(message),
            RegionDefinitionError::Logic(message) => DatabaseError::Logic(message),
        }
    }
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseError::Sqlite(e) => f.write_str(&e.message),
            DatabaseError::Io { message, .. } => f.write_str(message),
            DatabaseError::MapboxTileLimitExceeded => MapboxTileLimitExceededException.fmt(f),
            DatabaseError::Runtime(message) | DatabaseError::Logic(message) => f.write_str(message),
        }
    }
}

type DbResult<T> = Result<T, DatabaseError>;

/// The tile data of a tile resource. C++ dereferences `resource.tileData`
/// unchecked; a tile resource without it (built by hand) is stored and found
/// by its URL here, as other resources are.
fn tile_data(resource: &Resource) -> Option<&TileData> {
    resource.tile_data.as_ref().filter(|_| resource.kind == Kind::Tile)
}

/// `util::deleteFile` (src/mln/util/io.cpp).
fn delete_file(filename: &str) -> DbResult<()> {
    match std::fs::remove_file(filename) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            let code = e.raw_os_error().unwrap_or(0);
            // IOException(err, msg): msg + ": " + std::generic_category().message(err)
            let what = std::io::Error::from_raw_os_error(code).to_string();
            let what = what.split(" (os error").next().unwrap_or(&what).to_owned();
            Err(DatabaseError::Io { code, message: format!("Could not delete file {filename}: {what}") })
        }
    }
}

/// `OfflineDatabase::DatabaseSizeChangeStats`: the database size (pages in
/// use) when it was created. The C++ object keeps a pointer to the
/// database; here the database is passed to the methods that query it.
struct DatabaseSizeChangeStats {
    page_size: u64,
    page_count: u64,
    initial_size: u64,
}

impl DatabaseSizeChangeStats {
    fn new(db: &mut OfflineDatabase) -> DbResult<Self> {
        let page_size = db.get_pragma_i64("PRAGMA page_size")? as u64;
        let page_count = db.get_pragma_i64("PRAGMA page_count")? as u64;
        let freelist_count = db.get_pragma_i64("PRAGMA freelist_count")?;
        let initial_size = page_size.wrapping_mul(page_count.wrapping_sub(freelist_count as u64));
        Ok(DatabaseSizeChangeStats { page_size, page_count, initial_size })
    }

    /// Returns difference between current database size and database size at
    /// the time of creation of this object.
    fn diff(&self, db: &mut OfflineDatabase) -> DbResult<i64> {
        let page_count = db.get_pragma_i64("PRAGMA page_count")?;
        let freelist_count = db.get_pragma_i64("PRAGMA freelist_count")?;
        let current_size = (self.page_size as i64).wrapping_mul(page_count.wrapping_sub(freelist_count));
        Ok(current_size.wrapping_sub(self.initial_size as i64))
    }

    /// Returns how many bytes were released comparing to a database size at
    /// the time of creation of this object.
    fn bytes_released(&self, db: &mut OfflineDatabase) -> DbResult<u64> {
        let freelist_count = db.get_pragma_i64("PRAGMA freelist_count")?;
        let current_size = self.page_size.wrapping_mul(self.page_count.wrapping_sub(freelist_count as u64));
        Ok((self.initial_size.wrapping_sub(current_size) as i64).max(0) as u64)
    }

    /// Returns page size for the database.
    fn page_size(&self) -> u64 {
        self.page_size
    }
}

/// `OfflineDatabase`.
pub struct OfflineDatabase {
    path: String,
    db: Option<Database>,

    maximum_ambient_cache_size: u64,
    offline_mapbox_tile_count_limit: u64,
    offline_mapbox_tile_count: Option<u64>,

    tile_server_options: TileServerOptions,

    current_ambient_cache_size: Option<u64>,

    autopack: bool,
    read_only: bool,
}

impl OfflineDatabase {
    /// Single-point transformer for offline storage keys. For sources where
    /// the URL is not enough to guarantee uniqueness (eg. PMTiles where all
    /// requests resolve to the same URL), this function creates a synthetic
    /// key based on the requested data range.
    pub fn cache_key(resource: &Resource) -> String {
        let Some((first, second)) = resource.data_range else {
            return resource.url.clone();
        };
        let sep = if resource.url.contains('?') { '&' } else { '?' };
        format!("{}{}_mlnRange={}-{}", resource.url, sep, first, second)
    }

    /// `OfflineDatabase(std::string path, const TileServerOptions&)`.
    pub fn new(path: impl Into<String>, options: &TileServerOptions) -> Self {
        let mut db = OfflineDatabase {
            path: path.into(),
            db: None,
            maximum_ambient_cache_size: DEFAULT_MAX_CACHE_SIZE,
            offline_mapbox_tile_count_limit: DEFAULT_OFFLINE_TILE_COUNT_LIMIT,
            offline_mapbox_tile_count: None,
            tile_server_options: options.clone(),
            current_ambient_cache_size: None,
            autopack: true,
            read_only: false,
        };
        if let Err(e) = db.initialize() {
            db.handle_error(&e, "open database");
        }
        // Assume that we can't open the database right now and work with an
        // empty database object.
        db
    }

    fn initialize(&mut self) -> DbResult<()> {
        assert!(self.db.is_none());

        if self.read_only {
            let db = Database::open(&self.path, OpenFlag::ReadOnly)?;
            self.db = Some(db);
            let db = self.db.as_ref().unwrap();
            db.set_busy_timeout(i64::MAX)?;
            db.exec("PRAGMA foreign_keys = ON")?;
            return Ok(());
        }

        let db = Database::open(&self.path, OpenFlag::ReadWriteCreate)?;
        self.db = Some(db);
        {
            let db = self.db.as_ref().unwrap();
            db.set_busy_timeout(i64::MAX)?;
            db.exec("PRAGMA foreign_keys = ON")?;
        }

        let user_version = self.get_pragma_i64("PRAGMA user_version")?;
        match user_version {
            0 | 1 => {
                // Newly created database, or old cache-only database; remove
                // old table if it exists.
                self.remove_old_cache_table()?;
                self.create_schema()?;
                Ok(())
            }
            2..=6 => {
                if user_version == 2 {
                    self.migrate_to_version3()?;
                }
                // 3: Removed migration, see below.
                if user_version <= 4 {
                    self.migrate_to_version5()?;
                }
                if user_version <= 5 {
                    self.migrate_to_version6()?;
                }
                // 6: Happy path; we're done
                Ok(())
            }
            _ => {
                // Downgrade: delete the database and try to reinitialize.
                self.remove_existing()?;
                self.initialize()
            }
        }
    }

    /// `changePath`.
    pub fn change_path(&mut self, path: &str) {
        Log::info(Event::Database, "Changing the database path.");
        self.cleanup();
        self.path = path.to_owned();
        if let Err(e) = self.initialize() {
            // C++ lets this propagate to the caller (the database thread).
            self.handle_error(&e, "open database");
        }
    }

    fn cleanup(&mut self) {
        // Deleting these SQLite objects may result in exceptions (rusqlite
        // closes on drop and ignores the result).
        self.db = None;
    }

    fn disabled(&mut self) -> bool {
        if self.maximum_ambient_cache_size != 0 {
            return false;
        }

        let regions = self.list_regions();
        regions.map_or(true, |regions| regions.is_empty())
    }

    fn handle_error(&mut self, error: &DatabaseError, action: &str) {
        match error {
            DatabaseError::Sqlite(ex) => {
                if ex.code == result_code::NOT_A_DB
                    || ex.code == result_code::CORRUPT
                    || (ex.code == result_code::READ_ONLY
                        && ex.extended_code == extended_result_code::READ_ONLY_DB_MOVED)
                {
                    // The database was corruped, moved away, or deleted.
                    // We're going to start fresh with a clean slate for the
                    // next operation.
                    Log::record(
                        EventSeverity::Error,
                        Event::Database,
                        ex.code as i64,
                        &format!("Can't {action}: {}", ex.message),
                    );
                    if let Err(io_ex) = self.remove_existing() {
                        self.handle_error(&io_ex, action);
                    }
                } else {
                    // We treat the error as temporary, and pretend we have an
                    // inaccessible DB.
                    Log::record(
                        EventSeverity::Warning,
                        Event::Database,
                        ex.code as i64,
                        &format!("Can't {action}: {}", ex.message),
                    );
                }
            }
            DatabaseError::Io { code, message } => {
                // We failed to delete the database file.
                Log::record(EventSeverity::Error, Event::Database, *code as i64, &format!("Can't {action}: {message}"));
            }
            DatabaseError::MapboxTileLimitExceeded => {
                // This is ours and must be handled on client side (C++
                // rethrows; the callers that can see it return it).
            }
            DatabaseError::Runtime(message) => {
                Log::error(Event::Database, format!("Can't {action}: {message}"));
            }
            DatabaseError::Logic(_) => {
                // C++: assert(false) in debug builds.
                Log::error(Event::Database, format!("Can't {action}"));
            }
        }
    }

    fn remove_existing(&mut self) -> DbResult<()> {
        Log::warning(Event::Database, "Removing existing incompatible offline database");

        let filename = match &self.db {
            Some(db) => db.get_filename(),
            None => self.path.clone(),
        };
        self.db = None;

        if !filename.is_empty() {
            delete_file(&filename)?;
        }
        Ok(())
    }

    fn remove_old_cache_table(&mut self) -> DbResult<()> {
        self.check_flags()?;
        self.get_db()?.exec("DROP TABLE IF EXISTS http_cache")?;
        if self.autopack {
            self.vacuum()?;
        }
        Ok(())
    }

    fn create_schema(&mut self) -> DbResult<()> {
        self.check_flags()?;

        self.vacuum()?;
        let db = self.get_db()?;
        db.exec("PRAGMA journal_mode = DELETE")?;
        db.exec("PRAGMA synchronous = FULL")?;
        self.with_transaction(TransactionMode::Deferred, |this| {
            let db = this.get_db()?;
            db.exec(OFFLINE_DATABASE_SCHEMA)?;
            db.exec("PRAGMA user_version = 6")?;
            Ok(())
        })
    }

    fn migrate_to_version3(&mut self) -> DbResult<()> {
        self.check_flags()?;
        self.vacuum()?;
        self.get_db()?.exec("PRAGMA user_version = 3")?;
        Ok(())
    }

    // Schema version 4 was WAL journal + NORMAL sync. It was reverted during
    // pre-release development and the migration was removed entirely to
    // avoid potential conflicts from quickly (and needlessly) switching
    // journal and sync modes.
    //
    // See: https://github.com/mapbox/mapbox-gl-native/pull/6320

    fn migrate_to_version5(&mut self) -> DbResult<()> {
        self.check_flags()?;
        let db = self.get_db()?;
        db.exec("PRAGMA journal_mode = DELETE")?;
        db.exec("PRAGMA synchronous = FULL")?;
        db.exec("PRAGMA user_version = 5")?;
        Ok(())
    }

    fn migrate_to_version6(&mut self) -> DbResult<()> {
        self.check_flags()?;
        self.with_transaction(TransactionMode::Deferred, |this| {
            let db = this.get_db()?;
            db.exec("ALTER TABLE resources ADD COLUMN must_revalidate INTEGER NOT NULL DEFAULT 0")?;
            db.exec("ALTER TABLE tiles ADD COLUMN must_revalidate INTEGER NOT NULL DEFAULT 0")?;
            db.exec("PRAGMA user_version = 6")?;
            Ok(())
        })
    }

    fn vacuum(&mut self) -> DbResult<()> {
        self.check_flags()?;
        if self.get_pragma_i64("PRAGMA auto_vacuum")? != 2
        /*INCREMENTAL*/
        {
            let db = self.get_db()?;
            db.exec("PRAGMA auto_vacuum = INCREMENTAL")?;
            db.exec("VACUUM")?;
        } else {
            self.get_db()?.exec("PRAGMA incremental_vacuum")?;
        }
        Ok(())
    }

    fn check_flags(&self) -> DbResult<()> {
        if self.read_only {
            return Err(DatabaseError::Runtime("Cannot modify database in read-only mode".to_owned()));
        }
        Ok(())
    }

    /// `getStatement`'s `if (!db) initialize();`.
    fn get_db(&mut self) -> DbResult<&Database> {
        if self.db.is_none() {
            self.initialize()?;
        }
        Ok(self.db.as_ref().expect("initialized database"))
    }

    /// `Query{getStatement(sql)}` with the parameters bound, then `run()`
    /// on a statement that returns no rows; returns `changes()`.
    fn execute(&mut self, sql: &str, params: impl rusqlite::Params) -> DbResult<u64> {
        let conn = self.get_db()?.conn();
        let mut stmt = conn.prepare_cached(sql)?;
        Ok(stmt.execute(params)? as u64)
    }

    /// `Query{getStatement(sql)}` with the parameters bound and one `run()`:
    /// `f` reads the row, `None` when there is none.
    fn query_row<T>(
        &mut self,
        sql: &str,
        params: impl rusqlite::Params,
        f: impl FnOnce(&rusqlite::Row<'_>) -> DbResult<T>,
    ) -> DbResult<Option<T>> {
        let conn = self.get_db()?.conn();
        let mut stmt = conn.prepare_cached(sql)?;
        let mut rows = stmt.query(params)?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => f(row).map(Some),
        }
    }

    /// `query.lastInsertRowId()` of the last statement.
    fn last_insert_row_id(&mut self) -> DbResult<i64> {
        let conn = self.get_db()?.conn();
        Ok(conn.last_insert_rowid())
    }

    /// `mapbox::sqlite::Transaction transaction(*db, mode)` around `f`: the
    /// transaction commits after `f` succeeds and rolls back (in its
    /// destructor) when `f` fails.
    fn with_transaction<T>(&mut self, mode: TransactionMode, f: impl FnOnce(&mut Self) -> DbResult<T>) -> DbResult<T> {
        let mut transaction = Transaction::new(self.get_db()?, mode)?;
        let value = f(self)?;
        transaction.commit()?;
        Ok(value)
    }

    /// `getPragma<int64_t>(sql)`.
    fn get_pragma_i64(&mut self, sql: &str) -> DbResult<i64> {
        Ok(self.query_row(sql, [], |row| Ok(sqlite3::get_i64(row, 0)?))?.unwrap_or(0))
    }

    /// `get(const Resource&)`.
    pub fn get(&mut self, resource: &Resource) -> Option<Response> {
        let result = (|| -> DbResult<Option<Response>> {
            if self.disabled() {
                return Ok(None);
            }

            let result = self.get_internal(resource)?;
            Ok(result.map(|(response, _size)| response))
        })();
        match result {
            Ok(response) => response,
            Err(e) => {
                self.handle_error(&e, "read resource");
                None
            }
        }
    }

    fn get_internal(&mut self, resource: &Resource) -> DbResult<Option<(Response, u64)>> {
        if let Some(tile_data) = tile_data(resource) { self.get_tile(tile_data) } else { self.get_resource(resource) }
    }

    fn has_internal(&mut self, resource: &Resource) -> DbResult<Option<i64>> {
        if let Some(tile_data) = tile_data(resource) { self.has_tile(tile_data) } else { self.has_resource(resource) }
    }

    /// `put(const Resource&, const Response&)`: (inserted, stored size).
    pub fn put(&mut self, resource: &Resource, response: &Response) -> (bool, u64) {
        if self.read_only {
            return (false, 0);
        }

        let result = (|| -> DbResult<(bool, u64)> {
            if self.db.is_none() {
                self.initialize()?;
            }

            if self.disabled() {
                return Ok((false, 0));
            }

            self.with_transaction(TransactionMode::Immediate, |this| this.put_internal(resource, response, true))
        })();
        match result {
            Ok(result) => result,
            Err(e) => {
                self.handle_error(&e, "write resource");
                (false, 0)
            }
        }
    }

    fn put_internal(&mut self, resource: &Resource, response: &Response, evict: bool) -> DbResult<(bool, u64)> {
        self.check_flags()?;

        if response.error.is_some() {
            return Ok((false, 0));
        }

        // An ambient cache with no room can't store anything, so skip compressing.
        if evict && self.maximum_ambient_cache_size == 0 {
            return Ok((false, 0));
        }

        let mut compressed_data = Vec::new();
        let mut compressed = false;
        let mut size = 0u64;

        if let Some(data) = &response.data {
            compressed_data = compress(data).map_err(DatabaseError::Runtime)?;
            compressed = compressed_data.len() < data.len();
            size = if compressed { compressed_data.len() } else { data.len() } as u64;
        }

        let mut stats = None;
        if evict {
            let s = DatabaseSizeChangeStats::new(self)?;
            if !self.evict(size, &s)? {
                Log::info(Event::Database, "Unable to make space for entry");
                return Ok((false, 0));
            }
            stats = Some(s);
        }

        let data: &[u8] = if compressed {
            &compressed_data
        } else if let Some(data) = &response.data {
            data
        } else {
            b""
        };

        let inserted = if let Some(tile_data) = tile_data(resource) {
            self.put_tile(tile_data, response, data, compressed)?
        } else {
            self.put_resource(resource, response, data, compressed)?
        };

        if let Some(stats) = stats {
            self.update_ambient_cache_size(&stats)?;
        }

        Ok((inserted, size))
    }

    fn update_accessed(&mut self, sql: &str, params: impl rusqlite::Params) -> DbResult<()> {
        // Update accessed timestamp used for LRU eviction.
        if self.read_only {
            return Ok(());
        }
        match self.execute(sql, params) {
            Ok(_) => Ok(()),
            Err(DatabaseError::Sqlite(ex)) => {
                if ex.code == result_code::NOT_A_DB || ex.code == result_code::CORRUPT {
                    return Err(DatabaseError::Sqlite(ex));
                }
                // If we don't have any indication that the database is
                // corrupt, continue as usual.
                Log::record(
                    EventSeverity::Warning,
                    Event::Database,
                    ex.code as i64,
                    &format!("Can't update timestamp: {}", ex.message),
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Reads `etag, expires, must_revalidate, modified, data, compressed` of
    /// the first row, as getResource/getTile do.
    fn read_response(row: &rusqlite::Row<'_>) -> DbResult<(Response, u64)> {
        let mut response = Response::new();
        let mut size = 0u64;

        response.etag = get_optional_string(row, 0)?;
        response.expires = get_optional_i64(row, 1)?.map(Timestamp::from_secs);
        response.must_revalidate = get_bool(row, 2)?;
        response.modified = get_optional_i64(row, 3)?.map(Timestamp::from_secs);

        let data = get_optional_bytes(row, 4)?;
        match data {
            None => response.no_content = true,
            Some(data) => {
                if get_bool(row, 5)? {
                    let decompressed = decompress(&data).map_err(DatabaseError::Runtime)?;
                    response.data = Some(Arc::new(decompressed));
                } else {
                    response.data = Some(Arc::new(data.clone()));
                }
                size = data.len() as u64;
            }
        }

        Ok((response, size))
    }

    fn get_resource(&mut self, resource: &Resource) -> DbResult<Option<(Response, u64)>> {
        let key = Self::cache_key(resource);
        self.update_accessed(
            "UPDATE resources SET accessed = ?1 WHERE url = ?2",
            params![now().secs_since_epoch(), key],
        )?;

        self.query_row(
            //        0      1            2            3       4      5
            "SELECT etag, expires, must_revalidate, modified, data, compressed \
             FROM resources \
             WHERE url = ?",
            params![key],
            Self::read_response,
        )
    }

    fn has_resource(&mut self, resource: &Resource) -> DbResult<Option<i64>> {
        let key = Self::cache_key(resource);
        Ok(self
            .query_row("SELECT length(data) FROM resources WHERE url = ?", params![key], |row| {
                Ok(get_optional_i64(row, 0)?)
            })?
            .flatten())
    }

    fn put_resource(
        &mut self,
        resource: &Resource,
        response: &Response,
        data: &[u8],
        compressed: bool,
    ) -> DbResult<bool> {
        self.check_flags()?;

        let key = Self::cache_key(resource);
        let expires = response.expires.map(Timestamp::secs_since_epoch);
        let modified = response.modified.map(Timestamp::secs_since_epoch);

        if response.not_modified {
            self.execute(
                "UPDATE resources \
                 SET accessed         = ?1, \
                     expires          = ?2, \
                     must_revalidate  = ?3 \
                 WHERE url    = ?4 ",
                params![now().secs_since_epoch(), expires, response.must_revalidate, key],
            )?;
            return Ok(false);
        }

        // `bindBlob(data.data(), data.size())`: an empty string is an empty
        // blob, noContent is NULL.
        let (data, compressed): (Option<&[u8]>, bool) =
            if response.no_content { (None, false) } else { (Some(data), compressed) };

        // We can't use REPLACE because it would change the id value.
        let changes = self.execute(
            "UPDATE resources \
             SET kind            = ?1, \
                 etag            = ?2, \
                 expires         = ?3, \
                 must_revalidate = ?4, \
                 modified        = ?5, \
                 accessed        = ?6, \
                 data            = ?7, \
                 compressed      = ?8 \
             WHERE url           = ?9 ",
            params![
                resource.kind as i32,
                response.etag,
                expires,
                response.must_revalidate,
                modified,
                now().secs_since_epoch(),
                data,
                compressed,
                key
            ],
        )?;
        if changes != 0 {
            return Ok(false);
        }

        self.execute(
            "INSERT INTO resources (url, kind, etag, expires, must_revalidate, modified, accessed, data, compressed) \
             VALUES                (?1,  ?2,   ?3,   ?4,      ?5,              ?6,       ?7,       ?8,   ?9) ",
            params![
                key,
                resource.kind as i32,
                response.etag,
                expires,
                response.must_revalidate,
                modified,
                now().secs_since_epoch(),
                data,
                compressed
            ],
        )?;

        Ok(true)
    }

    fn get_tile(&mut self, tile: &TileData) -> DbResult<Option<(Response, u64)>> {
        self.update_accessed(
            "UPDATE tiles \
             SET accessed       = ?1 \
             WHERE url_template = ?2 \
               AND pixel_ratio  = ?3 \
               AND x            = ?4 \
               AND y            = ?5 \
               AND z            = ?6 ",
            params![now().secs_since_epoch(), tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
        )?;

        self.query_row(
            //        0      1           2,            3,      4,      5
            "SELECT etag, expires, must_revalidate, modified, data, compressed \
             FROM tiles \
             WHERE url_template = ?1 \
               AND pixel_ratio  = ?2 \
               AND x            = ?3 \
               AND y            = ?4 \
               AND z            = ?5 ",
            params![tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
            Self::read_response,
        )
    }

    fn has_tile(&mut self, tile: &TileData) -> DbResult<Option<i64>> {
        Ok(self
            .query_row(
                "SELECT length(data) \
                 FROM tiles \
                 WHERE url_template = ?1 \
                   AND pixel_ratio  = ?2 \
                   AND x            = ?3 \
                   AND y            = ?4 \
                   AND z            = ?5 ",
                params![tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
                |row| Ok(get_optional_i64(row, 0)?),
            )?
            .flatten())
    }

    fn put_tile(&mut self, tile: &TileData, response: &Response, data: &[u8], compressed: bool) -> DbResult<bool> {
        self.check_flags()?;

        let expires = response.expires.map(Timestamp::secs_since_epoch);
        let modified = response.modified.map(Timestamp::secs_since_epoch);

        if response.not_modified {
            self.execute(
                "UPDATE tiles \
                 SET accessed        = ?1, \
                     expires         = ?2, \
                     must_revalidate = ?3 \
                 WHERE url_template  = ?4 \
                   AND pixel_ratio   = ?5 \
                   AND x             = ?6 \
                   AND y             = ?7 \
                   AND z             = ?8 ",
                params![
                    now().secs_since_epoch(),
                    expires,
                    response.must_revalidate,
                    tile.url_template,
                    tile.pixel_ratio,
                    tile.x,
                    tile.y,
                    tile.z
                ],
            )?;
            return Ok(false);
        }

        let (data, compressed): (Option<&[u8]>, bool) =
            if response.no_content { (None, false) } else { (Some(data), compressed) };

        // We can't use REPLACE because it would change the id value.
        let changes = self.execute(
            "UPDATE tiles \
             SET modified        = ?1, \
                 etag            = ?2, \
                 expires         = ?3, \
                 must_revalidate = ?4, \
                 accessed        = ?5, \
                 data            = ?6, \
                 compressed      = ?7 \
             WHERE url_template  = ?8 \
               AND pixel_ratio   = ?9 \
               AND x             = ?10 \
               AND y             = ?11 \
               AND z             = ?12 ",
            params![
                modified,
                response.etag,
                expires,
                response.must_revalidate,
                now().secs_since_epoch(),
                data,
                compressed,
                tile.url_template,
                tile.pixel_ratio,
                tile.x,
                tile.y,
                tile.z
            ],
        )?;
        if changes != 0 {
            return Ok(false);
        }

        self.execute(
            "INSERT INTO tiles (url_template, pixel_ratio, x,  y,  z,  modified, must_revalidate, etag, expires, accessed,  data, compressed) \
             VALUES            (?1,           ?2,          ?3, ?4, ?5, ?6,       ?7,              ?8,   ?9,      ?10,       ?11,  ?12)",
            params![
                tile.url_template,
                tile.pixel_ratio,
                tile.x,
                tile.y,
                tile.z,
                modified,
                response.must_revalidate,
                response.etag,
                expires,
                now().secs_since_epoch(),
                data,
                compressed
            ],
        )?;

        Ok(true)
    }

    /// `invalidateAmbientCache()`: force revalidation of the ambient cache's
    /// tiles and resources with the server before using them.
    pub fn invalidate_ambient_cache(&mut self) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;

            self.execute(
                "UPDATE tiles \
                 SET expires = 0, must_revalidate = 1 \
                 WHERE id NOT IN (\
                     SELECT tile_id FROM region_tiles\
                 )",
                [],
            )?;

            self.execute(
                "UPDATE resources \
                 SET expires = 0, must_revalidate = 1 \
                 WHERE id NOT IN (\
                     SELECT resource_id FROM region_resources\
                 )",
                [],
            )?;
            Ok(())
        })();
        self.exception_ptr(result, "invalidate ambient cache")
    }

    /// `clearAmbientCache()`: clear the tile cache, freeing resources.
    pub fn clear_ambient_cache(&mut self) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;

            self.execute(
                "DELETE FROM tiles \
                 WHERE id NOT IN (\
                     SELECT tile_id FROM region_tiles\
                 )",
                [],
            )?;

            self.execute(
                "DELETE FROM resources \
                 WHERE id NOT IN (\
                     SELECT resource_id FROM region_resources\
                 )",
                [],
            )?;

            if self.autopack {
                self.vacuum()?;
            }
            Ok(())
        })();
        self.exception_ptr(result, "clear ambient cache")
    }

    /// `invalidateRegion(int64_t)`.
    pub fn invalidate_region(&mut self, region_id: i64) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;

            self.execute(
                "UPDATE tiles \
                 SET expires = 0, must_revalidate = 1 \
                 WHERE id IN (\
                     SELECT tile_id FROM region_tiles WHERE region_id = ?\
                 )",
                params![region_id],
            )?;

            self.execute(
                "UPDATE resources \
                 SET expires = 0, must_revalidate = 1 \
                 WHERE id IN (\
                     SELECT resource_id FROM region_resources WHERE region_id = ?\
                 )",
                params![region_id],
            )?;
            Ok(())
        })();
        self.exception_ptr(result, "invalidate region")
    }

    /// The `try { ...; return nullptr; } catch (...) { handleError(action);
    /// return std::current_exception(); }` pattern.
    fn exception_ptr(&mut self, result: DbResult<()>, action: &str) -> Option<DatabaseError> {
        match result {
            Ok(()) => None,
            Err(e) => {
                self.handle_error(&e, action);
                Some(e)
            }
        }
    }

    /// The `try { return ...; } catch (...) { handleError(action); return
    /// unexpected(std::current_exception()); }` pattern.
    fn expected<T>(&mut self, result: DbResult<T>, action: &str) -> DbResult<T> {
        if let Err(e) = &result {
            self.handle_error(e, action);
        }
        result
    }

    /// `listRegions()`.
    pub fn list_regions(&mut self) -> Result<OfflineRegions, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegions> {
            let rows = {
                let conn = self.get_db()?.conn();
                let mut stmt = conn.prepare_cached("SELECT id, definition, description FROM regions")?;
                let mut rows = stmt.query([])?;
                let mut result = Vec::new();
                while let Some(row) = rows.next()? {
                    result.push((get_i64(row, 0)?, get_bytes(row, 1)?, get_bytes(row, 2)?));
                }
                result
            };
            let mut result = OfflineRegions::new();
            for (id, definition, description) in rows {
                match decode_offline_region_definition(&definition) {
                    Ok(definition) => result.push(OfflineRegion::new(id, definition, description)),
                    Err(ex) => {
                        // Catch errors from malformed offline region
                        // definitions and skip them.
                        Log::error(Event::General, ex.what());
                    }
                }
            }
            Ok(result)
        })();
        self.expected(result, "list regions")
    }

    /// `getRegion(int64_t)`.
    pub fn get_region(&mut self, region_id: i64) -> Result<Option<OfflineRegion>, DatabaseError> {
        let result = (|| -> DbResult<Option<OfflineRegion>> {
            let row = self.query_row(
                "SELECT definition, description FROM regions WHERE id = ?1",
                params![region_id],
                |row| Ok((get_bytes(row, 0)?, get_bytes(row, 1)?)),
            )?;
            let Some((definition, description)) = row else {
                return Ok(None);
            };

            if definition.is_empty() && description.is_empty() {
                return Err(DatabaseError::Runtime(format!("Region {region_id} not found in database")));
            }

            Ok(Some(OfflineRegion::new(region_id, decode_offline_region_definition(&definition)?, description)))
        })();
        self.expected(result, "get region")
    }

    /// `createRegion(const OfflineRegionDefinition&, const OfflineRegionMetadata&)`.
    pub fn create_region(
        &mut self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
    ) -> Result<OfflineRegion, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegion> {
            self.check_flags()?;

            self.execute(
                "INSERT INTO regions (definition, description) \
                 VALUES              (?1,         ?2) ",
                params![encode_offline_region_definition(definition), metadata_blob(metadata)],
            )?;
            Ok(OfflineRegion::new(self.last_insert_row_id()?, definition.clone(), metadata.clone()))
        })();
        self.expected(result, "create region")
    }

    /// `mergeDatabase(const std::string& sideDatabasePath)`.
    pub fn merge_database(&mut self, side_database_path: &str) -> Result<OfflineRegions, DatabaseError> {
        self.check_flags()?;

        match self.execute("ATTACH DATABASE ?1 AS side", params![side_database_path]) {
            Ok(_) => {}
            Err(DatabaseError::Sqlite(ex)) => {
                Log::record(
                    EventSeverity::Error,
                    Event::Database,
                    ex.code as i64,
                    &format!("Can't attach database ({side_database_path}) for merge: {}", ex.message),
                );
                return Err(DatabaseError::Sqlite(ex));
            }
            Err(e) => return Err(e),
        }

        let result = (|| -> DbResult<OfflineRegions> {
            // Support sideloaded databases at user_version = 6. Future schema
            // version changes will need to implement migration paths for
            // sideloaded databases at version 6.
            let side_user_version = self.get_pragma_i64("PRAGMA side.user_version")? as i32;
            let main_user_version = self.get_pragma_i64("PRAGMA user_version")?;
            if side_user_version < 6 || i64::from(side_user_version) != main_user_version {
                return Err(DatabaseError::Runtime("Merge database has incorrect user_version".to_owned()));
            }

            let current_tile_count = self.get_offline_mapbox_tile_count();
            let alias = format!("{}://", self.tile_server_options.uri_scheme_alias());
            let count_of_tiles_to_merge = self
                .query_row(
                    "SELECT COUNT(DISTINCT st.id) \
                     FROM side.tiles st \
                     JOIN side.region_tiles srt ON srt.tile_id = st.id \
                     LEFT JOIN tiles t ON st.url_template = t.url_template AND \
                     st.pixel_ratio = t.pixel_ratio AND \
                     st.z = t.z AND \
                     st.x = t.x AND \
                     st.y = t.y \
                     WHERE t.id IS NULL \
                     AND st.url_template LIKE ?1 || '%'",
                    params![alias],
                    |row| Ok(get_i64(row, 0)?),
                )?
                .unwrap_or(0);
            // `(countOfTilesToMerge + currentTileCount) > offlineMapboxTileCountLimit`
            // (int64_t + uint64_t: uint64_t arithmetic).
            if (count_of_tiles_to_merge as u64).wrapping_add(current_tile_count) > self.offline_mapbox_tile_count_limit
            {
                return Err(DatabaseError::MapboxTileLimitExceeded);
            }

            self.with_transaction(TransactionMode::Deferred, |this| {
                this.get_db()?.exec(MERGE_SIDELOADED_DATABASE_SQL)?;
                Ok(())
            })?;

            let rows = {
                let conn = self.get_db()?.conn();
                let mut stmt = conn.prepare_cached(
                    "SELECT DISTINCT r.id, r.definition, r.description \
                     FROM side.regions sr \
                     JOIN regions r ON sr.definition = r.definition  AND sr.description IS r.description",
                )?;
                let mut rows = stmt.query([])?;
                let mut result = Vec::new();
                while let Some(row) = rows.next()? {
                    result.push((get_i64(row, 0)?, get_bytes(row, 1)?, get_bytes(row, 2)?));
                }
                result
            };
            let mut result = OfflineRegions::new();
            for (id, definition, description) in rows {
                result.push(OfflineRegion::new(id, decode_offline_region_definition(&definition)?, description));
            }
            self.get_db()?.exec("DETACH DATABASE side")?;
            Ok(result)
        })();

        match result {
            // `catch (const std::runtime_error& ex)`: everything but the
            // logic errors, which propagate (side stays attached).
            Err(e) if !matches!(e, DatabaseError::Logic(_)) => {
                self.get_db()?.exec("DETACH DATABASE side")?;
                Log::error(Event::Database, e.to_string());
                Err(e)
            }
            result => result,
        }
    }

    /// `updateMetadata(int64_t, const OfflineRegionMetadata&)`.
    pub fn update_metadata(
        &mut self,
        region_id: i64,
        metadata: &OfflineRegionMetadata,
    ) -> Result<OfflineRegionMetadata, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegionMetadata> {
            self.check_flags()?;

            self.execute(
                "UPDATE regions SET description = ?1 \
                 WHERE id = ?2",
                params![metadata_blob(metadata), region_id],
            )?;

            Ok(metadata.clone())
        })();
        self.expected(result, "update region metadata")
    }

    /// `deleteRegion(OfflineRegion&&)`.
    pub fn delete_region(&mut self, region: OfflineRegion) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;

            self.execute("DELETE FROM regions WHERE id = ?", params![region.get_id()])?;

            let stats = DatabaseSizeChangeStats::new(self)?;
            self.evict(0, &stats)?;
            if self.autopack {
                self.vacuum()?;
            }
            self.update_ambient_cache_size(&stats)?;

            // Ensure that the cached offlineTileCount value is recalculated.
            self.offline_mapbox_tile_count = None;
            Ok(())
        })();
        self.exception_ptr(result, "delete region")
    }

    /// `getRegionResource(const Resource&)`: (response, stored size).
    pub fn get_region_resource(&mut self, resource: &Resource) -> Option<(Response, u64)> {
        match self.get_internal(resource) {
            Ok(result) => result,
            Err(e) => {
                self.handle_error(&e, "read region resource");
                None
            }
        }
    }

    /// `hasRegionResource(const Resource&)`.
    pub fn has_region_resource(&mut self, resource: &Resource) -> Option<i64> {
        match self.has_internal(resource) {
            Ok(result) => result,
            Err(e) => {
                self.handle_error(&e, "query region resource");
                None
            }
        }
    }

    /// `putRegionResource(int64_t, const Resource&, const Response&)`.
    pub fn put_region_resource(
        &mut self,
        region_id: i64,
        resource: &Resource,
        response: &Response,
    ) -> Result<u64, MapboxTileLimitExceededException> {
        let result = (|| -> DbResult<u64> {
            self.check_flags()?;

            if self.db.is_none() {
                self.initialize()?;
            }
            self.with_transaction(TransactionMode::Deferred, |this| {
                this.put_region_resource_internal(region_id, resource, response)
            })
        })();
        match result {
            Ok(size) => Ok(size),
            Err(DatabaseError::MapboxTileLimitExceeded) => Err(MapboxTileLimitExceededException),
            Err(e) => {
                self.handle_error(&e, "write region resource");
                Ok(0)
            }
        }
    }

    /// `putRegionResources(int64_t, const std::list<std::tuple<Resource,
    /// Response>>&, OfflineRegionStatus&)`.
    pub fn put_region_resources(
        &mut self,
        region_id: i64,
        resources: &[(Resource, Response)],
        status: &mut OfflineRegionStatus,
    ) -> Result<(), MapboxTileLimitExceededException> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;

            if self.db.is_none() {
                self.initialize()?;
            }
            let mut transaction = Transaction::new(self.get_db()?, TransactionMode::Deferred)?;

            // Accumulate all statistics locally first before adding them to
            // the OfflineRegionStatus object to ensure correctness when the
            // transaction fails.
            let mut completed_resource_count = 0u64;
            let mut completed_resource_size = 0u64;
            let mut completed_tile_count = 0u64;
            let mut completed_tile_size = 0u64;

            for (resource, response) in resources {
                match self.put_region_resource_internal(region_id, resource, response) {
                    Ok(resource_size) => {
                        completed_resource_count += 1;
                        completed_resource_size += resource_size;
                        if resource.kind == Kind::Tile {
                            completed_tile_count += 1;
                            completed_tile_size += resource_size;
                        }
                    }
                    Err(DatabaseError::MapboxTileLimitExceeded) => {
                        // Commit the rest of the batch and rethrow
                        transaction.commit()?;
                        return Err(DatabaseError::MapboxTileLimitExceeded);
                    }
                    Err(e) => return Err(e),
                }
            }

            // Commit the completed batch
            transaction.commit()?;

            status.completed_resource_count += completed_resource_count;
            status.completed_resource_size += completed_resource_size;
            status.completed_tile_count += completed_tile_count;
            status.completed_tile_size += completed_tile_size;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(DatabaseError::MapboxTileLimitExceeded) => Err(MapboxTileLimitExceededException),
            Err(e) => {
                self.handle_error(&e, "write region resources");
                Ok(())
            }
        }
    }

    fn put_region_resource_internal(
        &mut self,
        region_id: i64,
        resource: &Resource,
        response: &Response,
    ) -> DbResult<u64> {
        self.check_flags()?;

        let size = self.put_internal(resource, response, false)?.1;
        let previously_unused = self.mark_used(region_id, resource)?;

        if previously_unused && self.exceeds_offline_mapbox_tile_count_limit(resource) {
            return Err(DatabaseError::MapboxTileLimitExceeded);
        }

        if let Some(count) = self.offline_mapbox_tile_count.as_mut()
            && resource.kind == Kind::Tile
            && is_canonical_url(&self.tile_server_options, &resource.url)
            && previously_unused
        {
            *count += 1;
        }

        Ok(size)
    }

    /// Return value is true iff the resource was previously unused by any
    /// other regions.
    fn mark_used(&mut self, region_id: i64, resource: &Resource) -> DbResult<bool> {
        self.check_flags()?;

        if let Some(tile) = tile_data(resource) {
            let changes = self.execute(
                "INSERT OR IGNORE INTO region_tiles (region_id, tile_id) \
                 SELECT                              ?1,        tiles.id \
                 FROM tiles \
                 WHERE url_template = ?2 \
                   AND pixel_ratio  = ?3 \
                   AND x            = ?4 \
                   AND y            = ?5 \
                   AND z            = ?6 ",
                params![region_id, tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
            )?;

            let not_on_this_region = changes != 0;

            let not_on_other_region = self
                .query_row(
                    "SELECT region_id \
                     FROM region_tiles, tiles \
                     WHERE region_id   != ?1 \
                       AND tile_id      = id \
                       AND url_template = ?2 \
                       AND pixel_ratio  = ?3 \
                       AND x            = ?4 \
                       AND y            = ?5 \
                       AND z            = ?6 \
                     LIMIT 1 ",
                    params![region_id, tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
                    |_| Ok(()),
                )?
                .is_none();

            Ok(not_on_this_region && not_on_other_region)
        } else {
            let key = Self::cache_key(resource);
            let changes = self.execute(
                "INSERT OR IGNORE INTO region_resources (region_id, resource_id) \
                 SELECT                                  ?1,        resources.id \
                 FROM resources \
                 WHERE resources.url = ?2 ",
                params![region_id, key],
            )?;

            if changes == 0 {
                return Ok(false);
            }

            let found = self.query_row(
                "SELECT region_id \
                 FROM region_resources, resources \
                 WHERE region_id    !=  ?1 \
                   AND resources.url = ?2 \
                 LIMIT 1 ",
                params![region_id, key],
                |_| Ok(()),
            )?;
            Ok(found.is_none())
        }
    }

    /// `getRegionDefinition(int64_t)`.
    pub fn get_region_definition(&mut self, region_id: i64) -> Result<OfflineRegionDefinition, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegionDefinition> {
            // `query.run(); query.get<std::string>(0)`: no row reads as "".
            let definition = self
                .query_row("SELECT definition FROM regions WHERE id = ?1", params![region_id], |row| {
                    Ok(get_bytes(row, 0)?)
                })?
                .unwrap_or_default();

            Ok(decode_offline_region_definition(&definition)?)
        })();
        self.expected(result, "load region")
    }

    /// `getRegionCompletedStatus(int64_t)`.
    pub fn get_region_completed_status(&mut self, region_id: i64) -> Result<OfflineRegionStatus, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegionStatus> {
            let mut result = OfflineRegionStatus::default();

            let (count, size) = self.get_completed_resource_count_and_size(region_id)?;
            result.completed_resource_count = count as u64;
            result.completed_resource_size = size as u64;
            let (count, size) = self.get_completed_tile_count_and_size(region_id)?;
            result.completed_tile_count = count as u64;
            result.completed_tile_size = size as u64;

            result.completed_resource_count += result.completed_tile_count;
            result.completed_resource_size += result.completed_tile_size;

            Ok(result)
        })();
        self.expected(result, "get region status")
    }

    fn get_completed_resource_count_and_size(&mut self, region_id: i64) -> DbResult<(i64, i64)> {
        Ok(self
            .query_row(
                "SELECT COUNT(*), SUM(LENGTH(data)) \
                 FROM region_resources, resources \
                 WHERE region_id = ?1 \
                 AND resource_id = resources.id ",
                params![region_id],
                |row| Ok((get_i64(row, 0)?, get_i64(row, 1)?)),
            )?
            .unwrap_or_default())
    }

    fn get_completed_tile_count_and_size(&mut self, region_id: i64) -> DbResult<(i64, i64)> {
        Ok(self
            .query_row(
                "SELECT COUNT(*), SUM(LENGTH(data)) \
                 FROM region_tiles, tiles \
                 WHERE region_id = ?1 \
                 AND tile_id = tiles.id ",
                params![region_id],
                |row| Ok((get_i64(row, 0)?, get_i64(row, 1)?)),
            )?
            .unwrap_or_default())
    }

    // Remove least-recently used resources and tiles until the used database
    // size, as calculated by multiplying the number of in-use pages by the
    // page size, is less than the maximum cache size. Returns false if this
    // condition cannot be satisfied.
    //
    // SQLite database never shrinks in size unless we call VACUUM. We here
    // are monitoring the soft limit (i.e. number of free pages in the file)
    // and as it approaches to the hard limit (i.e. the actual file size) we
    // delete an arbitrary number of old cache entries. The free pages
    // approach saves us from calling VACUUM or keeping a running total,
    // which can be costly.
    fn evict(&mut self, needed_free_size: u64, stats: &DatabaseSizeChangeStats) -> DbResult<bool> {
        self.check_flags()?;
        let ambient_cache_size = if self.init_ambient_cache_size()?.is_none() {
            self.current_ambient_cache_size.expect("initialized ambient cache size")
        } else {
            self.maximum_ambient_cache_size
        };
        let mut new_ambient_cache_size =
            ambient_cache_size.wrapping_add(needed_free_size).wrapping_add(stats.page_size());

        while new_ambient_cache_size > self.maximum_ambient_cache_size {
            let accessed = self.query_row(
                "SELECT max(accessed) \
                 FROM ( \
                     SELECT accessed \
                     FROM resources \
                     LEFT JOIN region_resources \
                     ON resource_id = resources.id \
                     WHERE resource_id IS NULL \
                   UNION ALL \
                     SELECT accessed \
                     FROM tiles \
                     LEFT JOIN region_tiles \
                     ON tile_id = tiles.id \
                     WHERE tile_id IS NULL \
                   ORDER BY accessed ASC LIMIT ?1 \
                 ) ",
                params![50],
                |row| Ok(get_i64(row, 0)?),
            )?;
            let Some(accessed) = accessed else {
                return Ok(false);
            };

            let resource_changes = self.execute(
                "DELETE FROM resources \
                 WHERE id IN ( \
                   SELECT id FROM resources \
                   LEFT JOIN region_resources \
                   ON resource_id = resources.id \
                   WHERE resource_id IS NULL \
                   AND accessed <= ?1 \
                 ) ",
                params![accessed],
            )?;

            let tile_changes = self.execute(
                "DELETE FROM tiles \
                 WHERE id IN ( \
                   SELECT id FROM tiles \
                   LEFT JOIN region_tiles \
                   ON tile_id = tiles.id \
                   WHERE tile_id IS NULL \
                   AND accessed <= ?1 \
                 ) ",
                params![accessed],
            )?;

            // Update current ambient cache size, based on how many bytes were
            // released.
            let released = stats.bytes_released(self)?;
            new_ambient_cache_size = (new_ambient_cache_size as i64).wrapping_sub(released as i64).max(0) as u64;

            // The cached value of offlineTileCount does not need to be
            // updated here because only non-offline tiles can be removed by
            // eviction.
            if resource_changes == 0 && tile_changes == 0 {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Lazily initializes currentAmbientCacheSize. `Ok(Some(e))` is the
    /// returned `exception_ptr` (SQLite errors, handled); `Err` propagates.
    fn init_ambient_cache_size(&mut self) -> DbResult<Option<DatabaseError>> {
        if self.current_ambient_cache_size.is_none() {
            let size = self.query_row(
                "SELECT SUM(data) \
                 FROM ( \
                     SELECT SUM(IFNULL(LENGTH(data), 0) \
                                + IFNULL(LENGTH(id), 0) \
                                + IFNULL(LENGTH(url_template), 0) \
                                + IFNULL(LENGTH(pixel_ratio), 0) \
                                + IFNULL(LENGTH(x), 0) \
                                + IFNULL(LENGTH(y), 0) \
                                + IFNULL(LENGTH(z), 0) \
                                + IFNULL(LENGTH(expires), 0) \
                                + IFNULL(LENGTH(modified), 0) \
                                + IFNULL(LENGTH(etag), 0) \
                                + IFNULL(LENGTH(compressed), 0) \
                                + IFNULL(LENGTH(accessed), 0) \
                                + IFNULL(LENGTH(must_revalidate), 0) \
                                ) as data \
                     FROM tiles \
                     LEFT JOIN region_tiles \
                     ON tile_id = tiles.id \
                     WHERE tile_id IS NULL \
                   UNION ALL \
                     SELECT SUM(IFNULL(LENGTH(data), 0) \
                                + IFNULL(LENGTH(id), 0) \
                                + IFNULL(LENGTH(url), 0) \
                                + IFNULL(LENGTH(kind), 0) \
                                + IFNULL(LENGTH(expires), 0) \
                                + IFNULL(LENGTH(modified), 0) \
                                + IFNULL(LENGTH(etag), 0) \
                                + IFNULL(LENGTH(compressed), 0) \
                                + IFNULL(LENGTH(accessed), 0) \
                                + IFNULL(LENGTH(must_revalidate), 0) \
                                ) as data \
                     FROM resources \
                     LEFT JOIN region_resources \
                     ON resource_id = resources.id \
                     WHERE resource_id IS NULL \
                 ) ",
                [],
                |row| Ok(get_i64(row, 0)?),
            );
            match size {
                Ok(size) => self.current_ambient_cache_size = Some(size.unwrap_or(0) as u64),
                Err(DatabaseError::Sqlite(ex)) => {
                    let e = DatabaseError::Sqlite(ex);
                    self.handle_error(&e, "cannot get current ambient cache size");
                    return Ok(Some(e));
                }
                Err(e) => return Err(e),
            }
        }

        Ok(None)
    }

    /// `setMaximumAmbientCacheSize(uint64_t)`.
    pub fn set_maximum_ambient_cache_size(&mut self, size: u64) -> Option<DatabaseError> {
        let previous_maximum_ambient_cache_size = self.maximum_ambient_cache_size;

        // C++ lets errors other than SQLite's escape here; they are returned.
        match self.init_ambient_cache_size() {
            Ok(Some(exception)) | Err(exception) => return Some(exception),
            Ok(None) => {}
        }

        let result = (|| -> DbResult<()> {
            self.maximum_ambient_cache_size = size;

            if self.current_ambient_cache_size.expect("initialized ambient cache size")
                > self.maximum_ambient_cache_size
            {
                let stats = DatabaseSizeChangeStats::new(self)?;
                self.evict(0, &stats)?;
                if self.autopack {
                    self.vacuum()?;
                }
                self.update_ambient_cache_size(&stats)?;
            }

            Ok(())
        })();
        match result {
            Ok(()) => None,
            Err(e) => {
                self.maximum_ambient_cache_size = previous_maximum_ambient_cache_size;
                self.handle_error(&e, "set maximum ambient cache size");
                Some(e)
            }
        }
    }

    /// `setOfflineMapboxTileCountLimit`.
    pub fn set_offline_mapbox_tile_count_limit(&mut self, limit: u64) {
        self.offline_mapbox_tile_count_limit = limit;
    }

    /// `getOfflineMapboxTileCountLimit`.
    pub fn get_offline_mapbox_tile_count_limit(&self) -> u64 {
        self.offline_mapbox_tile_count_limit
    }

    /// `offlineMapboxTileCountLimitExceeded`.
    pub fn offline_mapbox_tile_count_limit_exceeded(&mut self) -> bool {
        self.get_offline_mapbox_tile_count() >= self.offline_mapbox_tile_count_limit
    }

    /// `getOfflineMapboxTileCount`.
    pub fn get_offline_mapbox_tile_count(&mut self) -> u64 {
        // Calculating this on every call would be much simpler than caching
        // and manually updating the value, but it would make offline
        // downloads an O(n²) operation, because the database query below
        // involves an index scan of region_tiles.

        if let Some(count) = self.offline_mapbox_tile_count {
            return count;
        }

        let alias = format!("{}://", self.tile_server_options.uri_scheme_alias());
        let result = self.query_row(
            "SELECT COUNT(DISTINCT id) \
             FROM region_tiles, tiles \
             WHERE tile_id = tiles.id \
             AND url_template LIKE ?1 || '%'",
            params![alias],
            |row| Ok(get_i64(row, 0)?),
        );
        match result {
            Ok(count) => {
                let count = count.unwrap_or(0) as u64;
                self.offline_mapbox_tile_count = Some(count);
                count
            }
            Err(e) => {
                self.handle_error(&e, "get offline Mapbox tile count");
                u64::MAX
            }
        }
    }

    /// `exceedsOfflineMapboxTileCountLimit(const Resource&)`.
    pub fn exceeds_offline_mapbox_tile_count_limit(&mut self, resource: &Resource) -> bool {
        resource.kind == Kind::Tile
            && is_canonical_url(&self.tile_server_options, &resource.url)
            && self.offline_mapbox_tile_count_limit_exceeded()
    }

    /// `markUsedResources(int64_t, const std::list<Resource>&)`.
    pub fn mark_used_resources(&mut self, region_id: i64, resources: &[Resource]) {
        let result = (|| -> DbResult<()> {
            if self.db.is_none() {
                self.initialize()?;
            }
            self.with_transaction(TransactionMode::Deferred, |this| {
                for resource in resources {
                    this.mark_used(region_id, resource)?;
                }
                Ok(())
            })
        })();
        if let Err(e) = result {
            self.handle_error(&e, "mark resources as used");
        }
    }

    /// `pack()`.
    pub fn pack(&mut self) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            if self.db.is_none() {
                self.initialize()?;
            }
            self.vacuum()
        })();
        self.exception_ptr(result, "pack storage")
    }

    /// `runPackDatabaseAutomatically`.
    pub fn run_pack_database_automatically(&mut self, autopack: bool) {
        self.autopack = autopack;
    }

    pub fn tile_server_options(&self) -> &TileServerOptions {
        &self.tile_server_options
    }

    /// `reopenDatabaseReadOnly(bool)`.
    pub fn reopen_database_read_only(&mut self, read_only: bool) {
        if self.read_only == read_only {
            return;
        }
        self.cleanup();
        self.read_only = read_only;
        if let Err(e) = self.initialize() {
            self.handle_error(&e, "reopen database read-only");
        }
    }

    /// `resetDatabase()`: C++ returns the exception (or null).
    pub fn reset_database(&mut self) -> Option<DatabaseError> {
        let result = self.remove_existing().and_then(|()| self.initialize());
        self.exception_ptr(result, "reset database")
    }

    fn update_ambient_cache_size(&mut self, stats: &DatabaseSizeChangeStats) -> DbResult<()> {
        debug_assert!(self.current_ambient_cache_size.is_some());
        if let Some(current) = self.current_ambient_cache_size {
            let diff = stats.diff(self)?;
            self.current_ambient_cache_size = Some((current as i64).wrapping_add(diff).max(0) as u64);
        }
        Ok(())
    }
}

/// `query.bindBlob(n, metadata)`: `sqlite3_bind_blob` with the vector's
/// `data()`, which is null for an empty vector, so empty metadata is stored
/// as NULL.
fn metadata_blob(metadata: &OfflineRegionMetadata) -> Option<&[u8]> {
    if metadata.is_empty() { None } else { Some(metadata.as_slice()) }
}
