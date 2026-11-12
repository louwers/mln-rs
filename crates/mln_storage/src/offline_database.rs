// Ports platform/default/include/mln/storage/offline_database.hpp and
// platform/default/src/mln/storage/offline_database.cpp.
//
// Failures are `DatabaseError`s, returned through `Result` and handled by
// `handle_error`. `put_region_resource(s)` handle their errors themselves.
//
// Prepared statements are rusqlite's per-connection statement cache
// (`prepare_cached`) instead of the `statements` map; both are dropped with
// the connection.
//
// Port-only (maplibre/maplibre-native#4748): a cache hit queues its row's
// `accessed` timestamp instead of writing it in a transaction of its own.
// The owner flushes the queue (`flush_accessed`, see `set_accessed_callback`);
// closing the database, changing its path, reopening it read-only, resizing
// the cache, deleting a region and a full queue flush it too, and `put`'s
// eviction writes it in its own transaction.

use std::collections::BTreeMap;
use std::sync::Arc;

use mln_util::chrono::{Timestamp, now};
use mln_util::compression::{compress, decompress};
use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;
use mln_util::error::{Error, ErrorKind};
use mln_util::logging::{Event, EventSeverity, Log};
use rusqlite::params;

use crate::merge_sideloaded::MERGE_SIDELOADED_DATABASE_SQL;
use crate::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionMetadata, OfflineRegionStatus, OfflineRegions,
    decode_offline_region_definition, encode_offline_region_definition,
};
use crate::offline_schema::OFFLINE_DATABASE_SCHEMA;
use crate::resource::{Kind, Resource, TileData};
use crate::response::Response;
use crate::sqlite3::{
    self, Database, OpenFlag, SqliteError, Transaction, TransactionMode, extended_result_code, get_bool, get_bytes,
    get_i64, get_optional_bytes, get_optional_i64, get_optional_string, result_code,
};

/// The most rows whose `accessed` timestamp waits to be written. Reads of
/// other rows while the queue is full don't update their timestamps.
pub const MAX_PENDING_ACCESSED: usize = 4096;

/// Why a database operation failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error(transparent)]
    Sqlite(#[from] SqliteError),
    /// A file operation failed with the OS error `code`.
    #[error("{message}")]
    Io { code: i32, message: String },
    /// The operation failed, such as on a malformed region definition in the
    /// database.
    #[error("{0}")]
    Runtime(String),
    /// An argument was invalid, such as a region definition's zoom range.
    #[error("{0}")]
    Logic(String),
    /// The database has no offline region with this ID.
    #[error("no offline region has ID {0}")]
    RegionNotFound(i64),
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(e: rusqlite::Error) -> Self {
        DatabaseError::Sqlite(e.into())
    }
}

impl From<Error> for DatabaseError {
    fn from(e: Error) -> Self {
        match e.kind {
            ErrorKind::InvalidArgument => DatabaseError::Logic(e.message),
            _ => DatabaseError::Runtime(e.message),
        }
    }
}

impl From<DatabaseError> for Error {
    /// An invalid argument for [`DatabaseError::Logic`], not found for
    /// [`DatabaseError::RegionNotFound`], and a native error otherwise.
    fn from(error: DatabaseError) -> Self {
        let kind = match error {
            DatabaseError::Logic(_) => ErrorKind::InvalidArgument,
            DatabaseError::RegionNotFound(_) => ErrorKind::NotFound,
            _ => ErrorKind::Native,
        };
        Error::new(kind, error.to_string())
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
    current_ambient_cache_size: Option<u64>,

    autopack: bool,
    read_only: bool,

    /// `accessed` timestamps (seconds) of cache hits not yet written, by
    /// row id.
    pending_resource_accessed: BTreeMap<i64, i64>,
    pending_tile_accessed: BTreeMap<i64, i64>,
    /// Set when `put`'s eviction wrote the pending timestamps in its
    /// transaction, so they are dropped once it commits.
    pending_applied_for_eviction: bool,
    /// The last flush failed: a full queue waits for the next scheduled
    /// flush instead of retrying on every read.
    accessed_flush_failed: bool,
    accessed_callback: Option<Box<dyn FnMut() + Send>>,
}

impl Drop for OfflineDatabase {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl OfflineDatabase {
    /// Single-point transformer for offline storage keys. For sources where
    /// the URL is not enough to guarantee uniqueness (eg. PMTiles where all
    /// requests resolve to the same URL), this function creates a synthetic
    /// key based on the requested data range.
    #[must_use]
    pub fn cache_key(resource: &Resource) -> String {
        let Some((first, second)) = resource.data_range else {
            return resource.url.clone();
        };
        let sep = if resource.url.contains('?') { '&' } else { '?' };
        format!("{}{}_mlnRange={}-{}", resource.url, sep, first, second)
    }

    /// `OfflineDatabase(std::string path)`.
    pub fn new(path: impl Into<String>) -> Self {
        let mut db = OfflineDatabase {
            path: path.into(),
            db: None,
            maximum_ambient_cache_size: DEFAULT_MAX_CACHE_SIZE,
            current_ambient_cache_size: None,
            autopack: true,
            read_only: false,
            pending_resource_accessed: BTreeMap::new(),
            pending_tile_accessed: BTreeMap::new(),
            pending_applied_for_eviction: false,
            accessed_flush_failed: false,
            accessed_callback: None,
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
        self.flush_accessed();
        self.discard_pending_accessed();
        // Deleting these SQLite objects may result in exceptions (rusqlite
        // closes on drop and ignores the result).
        self.db = None;
    }

    /// Sets the callback that runs after a cache hit leaves `accessed`
    /// timestamps waiting to be written; the owner then calls
    /// [`flush_accessed`](Self::flush_accessed) later. It runs inside `get`
    /// and `get_region_resource`, so it must not use this database.
    pub fn set_accessed_callback(&mut self, callback: Box<dyn FnMut() + Send>) {
        self.accessed_callback = Some(callback);
    }

    /// The number of rows whose `accessed` timestamp waits to be written.
    #[must_use]
    pub fn pending_accessed_count(&self) -> usize {
        self.pending_resource_accessed.len() + self.pending_tile_accessed.len()
    }

    /// Writes the waiting `accessed` timestamps in one transaction, never
    /// moving a row's timestamp back. Returns false and keeps them waiting
    /// when that fails or the database is read-only or closed.
    pub fn flush_accessed(&mut self) -> bool {
        if self.pending_accessed_count() == 0 {
            return true;
        }
        if self.read_only || self.db.is_none() {
            return false;
        }
        match self.with_transaction(TransactionMode::Immediate, Self::apply_pending_accessed) {
            Ok(()) => {
                self.discard_pending_accessed();
                true
            }
            Err(e) => {
                self.accessed_flush_failed = true;
                self.handle_error(&e, "flush accessed timestamps");
                false
            }
        }
    }

    fn discard_pending_accessed(&mut self) {
        self.pending_resource_accessed.clear();
        self.pending_tile_accessed.clear();
        self.accessed_flush_failed = false;
    }

    /// Runs the `accessed` updates in the open transaction.
    fn apply_pending_accessed(&mut self) -> DbResult<()> {
        self.get_db()?;
        let Some(db) = &self.db else {
            return Ok(());
        };
        let conn = db.conn();
        for (sql, pending) in [
            ("UPDATE resources SET accessed = MAX(accessed, ?1) WHERE id = ?2", &self.pending_resource_accessed),
            ("UPDATE tiles SET accessed = MAX(accessed, ?1) WHERE id = ?2", &self.pending_tile_accessed),
        ] {
            if pending.is_empty() {
                continue;
            }
            let mut stmt = conn.prepare_cached(sql)?;
            for (id, accessed) in pending {
                stmt.execute(params![accessed, id])?;
            }
        }
        Ok(())
    }

    /// Queues the `accessed` timestamp of a cache hit, used for LRU
    /// eviction.
    fn queue_accessed(&mut self, tile: bool, id: i64) {
        if self.read_only {
            return;
        }
        let accessed = now().secs_since_epoch();
        let full = self.pending_accessed_count() >= MAX_PENDING_ACCESSED;
        let pending = if tile { &mut self.pending_tile_accessed } else { &mut self.pending_resource_accessed };
        if let Some(existing) = pending.get_mut(&id) {
            *existing = (*existing).max(accessed);
        } else if !full {
            pending.insert(id, accessed);
        }
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
                        i64::from(ex.code),
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
                        i64::from(ex.code),
                        &format!("Can't {action}: {}", ex.message),
                    );
                }
            }
            DatabaseError::Io { code, message } => {
                // We failed to delete the database file.
                Log::record(
                    EventSeverity::Error,
                    Event::Database,
                    i64::from(*code),
                    &format!("Can't {action}: {message}"),
                );
            }
            DatabaseError::Runtime(message) => {
                Log::error(Event::Database, format!("Can't {action}: {message}"));
            }
            DatabaseError::Logic(_) => {
                // C++: assert(false) in debug builds.
                Log::error(Event::Database, format!("Can't {action}"));
            }
            // `RegionNotFound` is the caller's to report.
            DatabaseError::RegionNotFound(_) => {}
        }
    }

    fn remove_existing(&mut self) -> DbResult<()> {
        Log::warning(Event::Database, "Removing existing incompatible offline database");

        self.discard_pending_accessed();
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
        if self.get_pragma_i64("PRAGMA auto_vacuum")? == 2 {
            self.get_db()?.exec("PRAGMA incremental_vacuum")?;
        } else {
            let db = self.get_db()?;
            db.exec("PRAGMA auto_vacuum = INCREMENTAL")?;
            db.exec("VACUUM")?;
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
        let result = if let Some(tile_data) = tile_data(resource) {
            self.get_tile(tile_data)
        } else {
            self.get_resource(resource)
        }?;
        if self.pending_accessed_count() >= MAX_PENDING_ACCESSED && !self.accessed_flush_failed {
            self.flush_accessed();
        }
        if result.is_some()
            && self.pending_accessed_count() > 0
            && let Some(callback) = &mut self.accessed_callback
        {
            callback();
        }
        Ok(result)
    }

    /// (stored size, whether the stored copy can be used without
    /// revalidating it; see `Response::is_usable`).
    fn has_internal(&mut self, resource: &Resource) -> DbResult<Option<(i64, bool)>> {
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

            self.pending_applied_for_eviction = false;
            let result =
                self.with_transaction(TransactionMode::Immediate, |this| this.put_internal(resource, response, true))?;
            if self.pending_applied_for_eviction {
                self.discard_pending_accessed();
            }
            Ok(result)
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
            if !self.evict(size, &s, true)? {
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

    /// Reads `etag, expires, must_revalidate, modified, data, compressed,
    /// id` of the first row, as getResource/getTile do.
    fn read_response(row: &rusqlite::Row<'_>) -> DbResult<(Response, u64, i64)> {
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

        Ok((response, size, get_i64(row, 6)?))
    }

    fn get_resource(&mut self, resource: &Resource) -> DbResult<Option<(Response, u64)>> {
        let key = Self::cache_key(resource);
        let row = self.query_row(
            //        0      1            2            3       4      5          6
            "SELECT etag, expires, must_revalidate, modified, data, compressed, id \
             FROM resources \
             WHERE url = ?",
            params![key],
            Self::read_response,
        )?;
        Ok(row.map(|(response, size, id)| {
            self.queue_accessed(false, id);
            (response, size)
        }))
    }

    fn has_resource(&mut self, resource: &Resource) -> DbResult<Option<(i64, bool)>> {
        let key = Self::cache_key(resource);
        Ok(self
            .query_row(
                "SELECT length(data), must_revalidate, expires FROM resources WHERE url = ?",
                params![key],
                Self::read_size_and_usable,
            )?
            .flatten())
    }

    /// Reads `length(data), must_revalidate, expires`, as hasResource/hasTile
    /// do.
    fn read_size_and_usable(row: &rusqlite::Row<'_>) -> DbResult<Option<(i64, bool)>> {
        let Some(size) = get_optional_i64(row, 0)? else {
            return Ok(None);
        };
        let mut response = Response::new();
        response.must_revalidate = get_bool(row, 1)?;
        response.expires = get_optional_i64(row, 2)?.map(Timestamp::from_secs);
        Ok(Some((size, response.is_usable())))
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
        let row = self.query_row(
            //        0      1           2,            3,      4,      5,         6
            "SELECT etag, expires, must_revalidate, modified, data, compressed, id \
             FROM tiles \
             WHERE url_template = ?1 \
               AND pixel_ratio  = ?2 \
               AND x            = ?3 \
               AND y            = ?4 \
               AND z            = ?5 ",
            params![tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
            Self::read_response,
        )?;
        Ok(row.map(|(response, size, id)| {
            self.queue_accessed(true, id);
            (response, size)
        }))
    }

    fn has_tile(&mut self, tile: &TileData) -> DbResult<Option<(i64, bool)>> {
        Ok(self
            .query_row(
                "SELECT length(data), must_revalidate, expires \
                 FROM tiles \
                 WHERE url_template = ?1 \
                   AND pixel_ratio  = ?2 \
                   AND x            = ?3 \
                   AND y            = ?4 \
                   AND z            = ?5 ",
                params![tile.url_template, tile.pixel_ratio, tile.x, tile.y, tile.z],
                Self::read_size_and_usable,
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
            // The deleted rows' ids may be reused.
            self.discard_pending_accessed();

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
                        Log::error(Event::General, &ex.message);
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
                    i64::from(ex.code),
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
    pub fn delete_region(&mut self, region: &OfflineRegion) -> Option<DatabaseError> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;
            // Eviction uses the stored timestamps if this fails.
            self.flush_accessed();

            self.execute("DELETE FROM regions WHERE id = ?", params![region.get_id()])?;

            let stats = DatabaseSizeChangeStats::new(self)?;
            self.evict(0, &stats, false)?;
            if self.autopack {
                self.vacuum()?;
            }
            self.update_ambient_cache_size(&stats)?;
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
        self.check_region_resource(resource).map(|(size, _)| size)
    }

    /// `checkRegionResource(const Resource&)`: (stored size, whether the
    /// stored copy can be used without revalidating it; see
    /// `Response::is_usable`).
    pub fn check_region_resource(&mut self, resource: &Resource) -> Option<(i64, bool)> {
        match self.has_internal(resource) {
            Ok(result) => result,
            Err(e) => {
                self.handle_error(&e, "query region resource");
                None
            }
        }
    }

    /// `putRegionResource(int64_t, const Resource&, const Response&)`.
    pub fn put_region_resource(&mut self, region_id: i64, resource: &Resource, response: &Response) -> u64 {
        let result = (|| -> DbResult<u64> {
            self.check_flags()?;

            if self.db.is_none() {
                self.initialize()?;
            }
            self.with_transaction(TransactionMode::Deferred, |this| {
                this.put_region_resource_internal(region_id, resource, response)
            })
        })();
        result.unwrap_or_else(|e| {
            self.handle_error(&e, "write region resource");
            0
        })
    }

    /// `putRegionResources(int64_t, const std::list<std::tuple<Resource,
    /// Response>>&, OfflineRegionStatus&)`.
    pub fn put_region_resources(
        &mut self,
        region_id: i64,
        resources: &[(Resource, Response)],
        status: &mut OfflineRegionStatus,
    ) {
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
                let resource_size = self.put_region_resource_internal(region_id, resource, response)?;
                completed_resource_count += 1;
                completed_resource_size += resource_size;
                if resource.kind == Kind::Tile {
                    completed_tile_count += 1;
                    completed_tile_size += resource_size;
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
        if let Err(e) = result {
            self.handle_error(&e, "write region resources");
        }
    }

    fn put_region_resource_internal(
        &mut self,
        region_id: i64,
        resource: &Resource,
        response: &Response,
    ) -> DbResult<u64> {
        self.check_flags()?;

        let mut size = self.put_internal(resource, response, false)?.1;
        if response.not_modified {
            // A 304 keeps the stored data, so report the size it takes up.
            size = self.has_internal(resource)?.map_or(0, |(stored, _)| stored as u64);
        }
        self.mark_used(region_id, resource)?;
        Ok(size)
    }

    /// Adds `resource`, which the database holds, to the region.
    fn mark_used(&mut self, region_id: i64, resource: &Resource) -> DbResult<()> {
        self.check_flags()?;

        if let Some(tile) = tile_data(resource) {
            self.execute(
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
        } else {
            self.execute(
                "INSERT OR IGNORE INTO region_resources (region_id, resource_id) \
                 SELECT                                  ?1,        resources.id \
                 FROM resources \
                 WHERE resources.url = ?2 ",
                params![region_id, Self::cache_key(resource)],
            )?;
        }
        Ok(())
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
    //
    // `apply_accessed` (put only) writes the pending `accessed` timestamps in
    // the caller's transaction before choosing what to evict.
    fn evict(
        &mut self,
        needed_free_size: u64,
        stats: &DatabaseSizeChangeStats,
        apply_accessed: bool,
    ) -> DbResult<bool> {
        self.check_flags()?;
        let ambient_cache_size = if self.init_ambient_cache_size()?.is_none() {
            self.current_ambient_cache_size.expect("initialized ambient cache size")
        } else {
            self.maximum_ambient_cache_size
        };
        let mut new_ambient_cache_size =
            ambient_cache_size.wrapping_add(needed_free_size).wrapping_add(stats.page_size());

        if apply_accessed
            && new_ambient_cache_size > self.maximum_ambient_cache_size
            && self.pending_accessed_count() > 0
        {
            self.apply_pending_accessed()?;
            self.pending_applied_for_eviction = true;
        }
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
        // Eviction uses the stored timestamps if this fails.
        self.flush_accessed();
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
                self.evict(0, &stats, false)?;
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
