// Failures are `DatabaseError`s, returned through `Result` and handled by
// `handle_error`. `put_region_resource(s)` handle their errors themselves.
//
// Statements come from rusqlite's per-connection statement cache
// (`prepare_cached`), so they are dropped with the connection.
//
// A cache hit queues its row's `accessed` timestamp instead of writing it in
// a transaction of its own.
// The owner flushes the queue (`flush_accessed`, see `set_accessed_callback`);
// closing the database, changing its path, reopening it read-only, resizing
// the cache, deleting a region and a full queue flush it too, and `put`'s
// eviction writes it in its own transaction.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mln_style::error::{Error, ErrorKind};
use mln_util::chrono::{Timestamp, now};
use mln_util::compression::{compress, decompress};
use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;
use rusqlite::types::FromSqlError;
use rusqlite::{Connection, OpenFlags, Row, ToSql, ffi, params};

use crate::log_target;
use crate::merge_sideloaded::MERGE_SIDELOADED_DATABASE_SQL;
use crate::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionExpiry, OfflineRegionMetadata, OfflineRegionStatus,
    OfflineRegions, decode_offline_region_definition, encode_offline_region_definition,
};
use crate::offline_schema::OFFLINE_DATABASE_SCHEMA;
use crate::resource::{Kind, Resource, TileData};
use crate::response::{Caching, Response};

/// The most rows whose `accessed` timestamp waits to be written. Reads of
/// other rows while the queue is full don't update their timestamps.
pub(crate) const MAX_PENDING_ACCESSED: usize = 4096;

/// Why a database operation failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum DatabaseError {
    /// SQLite failed with the extended result `code`.
    #[error("{message}")]
    Sqlite { code: i32, message: String },
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
        let code = e.sqlite_error().map_or(ffi::SQLITE_ERROR, |error| error.extended_code);
        DatabaseError::Sqlite { code, message: e.to_string() }
    }
}

impl From<FromSqlError> for DatabaseError {
    /// A column of an unexpected type, as rusqlite reports it.
    fn from(e: FromSqlError) -> Self {
        DatabaseError::Sqlite { code: ffi::SQLITE_ERROR, message: e.to_string() }
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

/// The tile data of a tile resource. A tile resource without it is stored
/// and found by its URL, as other resources are.
fn tile_data(resource: &Resource) -> Option<&TileData> {
    resource.tile_data.as_ref().filter(|_| resource.kind == Kind::Tile)
}

fn delete_file(filename: &str) -> DbResult<()> {
    match std::fs::remove_file(filename) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            let code = e.raw_os_error().unwrap_or(0);
            let what = std::io::Error::from_raw_os_error(code).to_string();
            let what = what.split(" (os error").next().unwrap_or(&what).to_owned();
            Err(DatabaseError::Io { code, message: format!("Could not delete file {filename}: {what}") })
        }
    }
}

/// The database size (pages in use) when it was created. The database is
/// passed to the methods that query it.
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

pub struct OfflineDatabase {
    path: String,
    db: Option<Connection>,

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
    pub(crate) fn cache_key(resource: &Resource) -> String {
        let Some((first, second)) = resource.data_range else {
            return resource.url.clone();
        };
        let sep = if resource.url.contains('?') { '&' } else { '?' };
        format!("{}{}_mlnRange={}-{}", resource.url, sep, first, second)
    }

    /// The database at `path`, which it creates or migrates to the current
    /// schema. A database it can't open, it opens again at its next use.
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
        // Assume that we can't open the database right now and work with an
        // empty database object.
        if let Err(e) = db.initialize() {
            db.handle_error(&e, "open database");
        }
        db
    }

    fn initialize(&mut self) -> DbResult<()> {
        assert!(self.db.is_none());

        let flags = if self.read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
        } else {
            OpenFlags::default()
        };
        let db = self.db.insert(Connection::open_with_flags(&self.path, flags)?);
        db.busy_timeout(Duration::from_millis(i32::MAX as u64))?;
        exec(db, "PRAGMA foreign_keys = ON")?;
        if self.read_only {
            return Ok(());
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

    #[cfg(test)]
    pub(crate) fn change_path(&mut self, path: &str) {
        log::info!(target: log_target::DATABASE, "Changing the database path.");
        self.cleanup();
        self.path = path.to_owned();
        if let Err(e) = self.initialize() {
            self.handle_error(&e, "open database");
        }
    }

    fn cleanup(&mut self) {
        self.flush_accessed();
        self.discard_pending_accessed();
        self.db = None;
    }

    /// Sets the callback that runs after a cache hit leaves `accessed`
    /// timestamps waiting to be written; the owner then calls
    /// [`flush_accessed`](Self::flush_accessed) later. It runs inside `get`
    /// and `get_region_resource`, so it must not use this database.
    pub(crate) fn set_accessed_callback(&mut self, callback: Box<dyn FnMut() + Send>) {
        self.accessed_callback = Some(callback);
    }

    /// The number of rows whose `accessed` timestamp waits to be written.
    #[must_use]
    pub(crate) fn pending_accessed_count(&self) -> usize {
        self.pending_resource_accessed.len() + self.pending_tile_accessed.len()
    }

    /// Writes the waiting `accessed` timestamps in one transaction, never
    /// moving a row's timestamp back. Returns false and keeps them waiting
    /// when that fails or the database is read-only or closed.
    pub(crate) fn flush_accessed(&mut self) -> bool {
        if self.pending_accessed_count() == 0 {
            return true;
        }
        if self.read_only || self.db.is_none() {
            return false;
        }
        match self.with_transaction(Begin::Immediate, Self::apply_pending_accessed) {
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
        let Some(conn) = &self.db else {
            return Ok(());
        };
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
            DatabaseError::Sqlite { code, message } => {
                let primary_code = code & 0xff;
                if primary_code == ffi::SQLITE_NOTADB
                    || primary_code == ffi::SQLITE_CORRUPT
                    || *code == ffi::SQLITE_READONLY_DBMOVED
                {
                    // The database was corruped, moved away, or deleted.
                    // We're going to start fresh with a clean slate for the
                    // next operation.
                    log::error!(
                        target: log_target::DATABASE,
                        code = i64::from(primary_code);
                        "Can't {action}: {message}"
                    );
                    if let Err(io_ex) = self.remove_existing() {
                        self.handle_error(&io_ex, action);
                    }
                } else {
                    // We treat the error as temporary, and pretend we have an
                    // inaccessible DB.
                    log::warn!(
                        target: log_target::DATABASE,
                        code = i64::from(primary_code);
                        "Can't {action}: {message}"
                    );
                }
            }
            DatabaseError::Io { code, message } => {
                // We failed to delete the database file.
                log::error!(target: log_target::DATABASE, code = i64::from(*code); "Can't {action}: {message}");
            }
            DatabaseError::Runtime(message) => {
                log::error!(target: log_target::DATABASE, "Can't {action}: {message}");
            }
            DatabaseError::Logic(_) => {
                log::error!(target: log_target::DATABASE, "Can't {action}");
            }
            // `RegionNotFound` is the caller's to report.
            DatabaseError::RegionNotFound(_) => {}
        }
    }

    fn remove_existing(&mut self) -> DbResult<()> {
        log::warn!(target: log_target::DATABASE, "Removing existing incompatible offline database");

        self.discard_pending_accessed();
        let filename = match &self.db {
            Some(db) => db.path().unwrap_or("").to_owned(),
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
        exec(self.get_db()?, "DROP TABLE IF EXISTS http_cache")?;
        if self.autopack {
            self.vacuum()?;
        }
        Ok(())
    }

    fn create_schema(&mut self) -> DbResult<()> {
        self.check_flags()?;

        self.vacuum()?;
        let db = self.get_db()?;
        exec(db, "PRAGMA journal_mode = DELETE")?;
        exec(db, "PRAGMA synchronous = FULL")?;
        self.with_transaction(Begin::Deferred, |this| {
            let db = this.get_db()?;
            exec(db, OFFLINE_DATABASE_SCHEMA)?;
            exec(db, "PRAGMA user_version = 6")?;
            Ok(())
        })
    }

    fn migrate_to_version3(&mut self) -> DbResult<()> {
        self.check_flags()?;
        self.vacuum()?;
        exec(self.get_db()?, "PRAGMA user_version = 3")?;
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
        exec(db, "PRAGMA journal_mode = DELETE")?;
        exec(db, "PRAGMA synchronous = FULL")?;
        exec(db, "PRAGMA user_version = 5")?;
        Ok(())
    }

    fn migrate_to_version6(&mut self) -> DbResult<()> {
        self.check_flags()?;
        self.with_transaction(Begin::Deferred, |this| {
            let db = this.get_db()?;
            exec(db, "ALTER TABLE resources ADD COLUMN must_revalidate INTEGER NOT NULL DEFAULT 0")?;
            exec(db, "ALTER TABLE tiles ADD COLUMN must_revalidate INTEGER NOT NULL DEFAULT 0")?;
            exec(db, "PRAGMA user_version = 6")?;
            Ok(())
        })
    }

    fn vacuum(&mut self) -> DbResult<()> {
        self.check_flags()?;
        if self.get_pragma_i64("PRAGMA auto_vacuum")? == 2 {
            exec(self.get_db()?, "PRAGMA incremental_vacuum")?;
        } else {
            // A database without pages, such as a new one in memory, takes
            // the setting with its first table, and has nothing to vacuum.
            let empty = self.get_pragma_i64("PRAGMA page_count")? == 0;
            let db = self.get_db()?;
            exec(db, "PRAGMA auto_vacuum = INCREMENTAL")?;
            if !empty {
                exec(db, "VACUUM")?;
            }
        }
        Ok(())
    }

    fn check_flags(&self) -> DbResult<()> {
        if self.read_only {
            return Err(DatabaseError::Runtime("Cannot modify database in read-only mode".to_owned()));
        }
        Ok(())
    }

    /// The connection, which opens the database if it isn't open.
    fn get_db(&mut self) -> DbResult<&Connection> {
        if self.db.is_none() {
            self.initialize()?;
        }
        Ok(self.db.as_ref().expect("initialized database"))
    }

    /// Runs `sql`, a statement that returns no rows, with `params` bound;
    /// returns the number of rows changed.
    fn execute(&mut self, sql: &str, params: impl rusqlite::Params) -> DbResult<u64> {
        let conn = self.get_db()?;
        let mut stmt = conn.prepare_cached(sql)?;
        Ok(stmt.execute(params)? as u64)
    }

    /// Runs `sql` with `params` bound and steps once: `f` reads the row,
    /// `None` when there is none.
    fn query_row<T>(
        &mut self,
        sql: &str,
        params: impl rusqlite::Params,
        f: impl FnOnce(&Row<'_>) -> DbResult<T>,
    ) -> DbResult<Option<T>> {
        let conn = self.get_db()?;
        let mut stmt = conn.prepare_cached(sql)?;
        let mut rows = stmt.query(params)?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => f(row).map(Some),
        }
    }

    /// The row id of the last statement's insert.
    fn last_insert_row_id(&mut self) -> DbResult<i64> {
        let conn = self.get_db()?;
        Ok(conn.last_insert_rowid())
    }

    /// Runs `f` in a transaction that commits after `f` succeeds and rolls
    /// back when `f` fails.
    fn with_transaction<T>(&mut self, begin: Begin, f: impl FnOnce(&mut Self) -> DbResult<T>) -> DbResult<T> {
        exec(self.get_db()?, begin.sql())?;
        match f(self) {
            Ok(value) => {
                exec(self.get_db()?, "COMMIT TRANSACTION")?;
                Ok(value)
            }
            Err(e) => {
                // `f`'s error is the one to report, not a failed rollback's.
                if let Some(db) = &self.db {
                    let _ = exec(db, "ROLLBACK TRANSACTION");
                }
                Err(e)
            }
        }
    }

    fn get_pragma_i64(&mut self, sql: &str) -> DbResult<i64> {
        Ok(self.query_row(sql, [], |row| Ok(row.get(0)?))?.unwrap_or(0))
    }

    /// The stored copy of `resource`, if any.
    pub fn get(&mut self, resource: &Resource) -> Option<Response> {
        let result = if self.disabled() { Ok(None) } else { self.get_internal(resource) };
        self.report(result, "read resource").ok().flatten().map(|(response, _size)| response)
    }

    fn get_internal(&mut self, resource: &Resource) -> DbResult<Option<(Response, u64)>> {
        let stored = Stored::of(resource);
        let sql = format!(
            "SELECT etag, modified, data, compressed, id, {} FROM {} WHERE {}",
            CACHING_COLUMNS,
            stored.table().name(),
            stored.condition()
        );
        let Some((response, size, id)) = self.query_row(&sql, &*stored.params(), read_response)? else {
            return Ok(None);
        };
        self.queue_accessed(stored.is_tile(), id);
        if self.pending_accessed_count() >= MAX_PENDING_ACCESSED && !self.accessed_flush_failed {
            self.flush_accessed();
        }
        if self.pending_accessed_count() > 0
            && let Some(callback) = &mut self.accessed_callback
        {
            callback();
        }
        Ok(Some((response, size)))
    }

    /// The stored size and caching rules of the copy of `resource`; `None`
    /// without one, or for a copy without data.
    fn has_internal(&mut self, resource: &Resource) -> DbResult<Option<(u64, Caching)>> {
        let stored = Stored::of(resource);
        let sql = format!(
            "SELECT length(data), {} FROM {} WHERE {}",
            CACHING_COLUMNS,
            stored.table().name(),
            stored.condition()
        );
        let row = self.query_row(&sql, &*stored.params(), |row| {
            let size: Option<i64> = row.get(0)?;
            size.map(|size| Ok((size as u64, read_caching(row, 1)?))).transpose()
        })?;
        Ok(row.flatten())
    }

    /// `put(const Resource&, const Response&)`: (inserted, stored size). The ambient cache doesn't
    /// keep a tile the tileset doesn't have (`Response::tile_not_found`), since it would read back as
    /// an empty tile.
    pub fn put(&mut self, resource: &Resource, response: &Response) -> (bool, u64) {
        if self.read_only || response.tile_not_found {
            return (false, 0);
        }

        let result = (|| -> DbResult<(bool, u64)> {
            if self.disabled() {
                return Ok((false, 0));
            }

            self.pending_applied_for_eviction = false;
            let result = self.with_transaction(Begin::Immediate, |this| this.put_internal(resource, response, true))?;
            if self.pending_applied_for_eviction {
                self.discard_pending_accessed();
            }
            Ok(result)
        })();
        self.report(result, "write resource").unwrap_or((false, 0))
    }

    /// Stores `response` for `resource`, first making room in the ambient
    /// cache when `evict` is set.
    fn put_internal(&mut self, resource: &Resource, response: &Response, evict: bool) -> DbResult<(bool, u64)> {
        self.check_flags()?;

        // An error isn't stored, nor what the server forbids storing.
        if response.error.is_some() || response.no_store {
            return Ok((false, 0));
        }

        // An ambient cache with no room can't store anything, so skip compressing.
        if evict && self.maximum_ambient_cache_size == 0 {
            return Ok((false, 0));
        }

        // A copy the database has, as for a 304, only renews its caching
        // rules.
        let renews = renews(response);
        let compressed = match &response.data {
            Some(data) if !renews => {
                Some(compress(data).map_err(DatabaseError::Runtime)?).filter(|c| c.len() < data.len())
            }
            _ => None,
        };
        let data = if renews {
            &[]
        } else {
            compressed.as_deref().or(response.data.as_deref().map(Vec::as_slice)).unwrap_or_default()
        };
        let size = data.len() as u64;

        let stats = if evict {
            let stats = DatabaseSizeChangeStats::new(self)?;
            if !self.evict(size, &stats, true)? {
                log::info!(target: log_target::DATABASE, "Unable to make space for entry");
                return Ok((false, 0));
            }
            Some(stats)
        } else {
            None
        };

        let inserted = self.write(&Stored::of(resource), response, data, compressed.is_some())?;

        if let Some(stats) = stats {
            self.update_ambient_cache_size(&stats)?;
        }

        Ok((inserted, size))
    }

    /// Writes `response`, with `data` as its stored bytes, to the row of
    /// `stored`, and gives whether it added the row. A copy the database has
    /// (`renews`) updates only its caching rules.
    fn write(&mut self, stored: &Stored<'_>, response: &Response, data: &[u8], compressed: bool) -> DbResult<bool> {
        let secs = |time: Option<Timestamp>| time.map(Timestamp::secs_since_epoch);
        let (expires, modified) = (secs(response.caching.expires), secs(response.modified));
        let accessed = now().secs_since_epoch();
        // An empty body is an empty blob, and no content is NULL.
        let data = (!response.no_content).then_some(data);
        let compressed = compressed && data.is_some();

        let mut columns: Vec<(&str, &dyn ToSql)> = vec![
            ("accessed", &accessed),
            ("expires", &expires),
            ("must_revalidate", &response.caching.must_revalidate),
        ];
        let renews = renews(response);
        if !renews {
            columns.extend::<[(&str, &dyn ToSql); 4]>([
                ("etag", &response.etag),
                ("modified", &modified),
                ("data", &data),
                ("compressed", &compressed),
            ]);
            columns.extend(stored.content());
        }
        let key = stored.key();
        let params: Vec<&dyn ToSql> = columns.iter().chain(&key).map(|(_, value)| *value).collect();

        // Not REPLACE, which would change the row's id.
        let assignments: Vec<String> = columns.iter().map(|(column, _)| format!("{column} = ?")).collect();
        let table = stored.table().name();
        let sql = format!("UPDATE {table} SET {} WHERE {}", assignments.join(", "), stored.condition());
        if self.execute(&sql, &*params)? != 0 || renews {
            return Ok(false);
        }

        let names: Vec<&str> = columns.iter().chain(&key).map(|(column, _)| *column).collect();
        let sql = format!("INSERT INTO {table} ({}) VALUES ({})", names.join(", "), vec!["?"; names.len()].join(", "));
        self.execute(&sql, &*params)?;
        Ok(true)
    }

    /// Makes the ambient cache's copies stale, so maps revalidate them when
    /// they next use them. See `invalidate`.
    pub(crate) fn invalidate_ambient_cache(&mut self) -> DbResult<()> {
        let result = self.invalidate(
            "id NOT IN (SELECT tile_id FROM region_tiles)",
            "id NOT IN (SELECT resource_id FROM region_resources)",
            [],
        );
        self.report(result, "invalidate ambient cache")
    }

    /// Makes the region's copies stale, so maps revalidate them when they
    /// next use them and its next download refreshes them. See `invalidate`.
    pub fn invalidate_region(&mut self, region_id: i64) -> DbResult<()> {
        let result = self.invalidate(
            "id IN (SELECT tile_id FROM region_tiles WHERE region_id = ?1)",
            "id IN (SELECT resource_id FROM region_resources WHERE region_id = ?1)",
            params![region_id],
        );
        self.report(result, "invalidate region")
    }

    /// Makes the tiles and resources whose ids match the conditions stale
    /// now. A copy that must revalidate then can't be used until it has.
    fn invalidate(&mut self, tiles: &str, resources: &str, params: impl rusqlite::Params + Copy) -> DbResult<()> {
        self.check_flags()?;
        let now = now().secs_since_epoch();
        for (table, ids) in [("tiles", tiles), ("resources", resources)] {
            self.execute(
                &format!("UPDATE {table} SET expires = MIN(IFNULL(expires, {now}), {now}) WHERE {ids}"),
                params,
            )?;
        }
        Ok(())
    }

    /// Clears the ambient cache, freeing space.
    pub(crate) fn clear_ambient_cache(&mut self) -> DbResult<()> {
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
        self.report(result, "clear ambient cache")
    }

    /// Handles `result`'s error, if any (see `handle_error`), and passes
    /// the result on.
    fn report<T>(&mut self, result: DbResult<T>, action: &str) -> DbResult<T> {
        if let Err(e) = &result {
            self.handle_error(e, action);
        }
        result
    }

    /// The regions that `sql` selects: their id, definition and description. A region whose
    /// definition is malformed fails, or, with `skip_malformed`, is left out.
    fn read_regions(
        &mut self,
        sql: &str,
        params: impl rusqlite::Params,
        skip_malformed: bool,
    ) -> DbResult<OfflineRegions> {
        let rows = {
            let mut stmt = self.get_db()?.prepare_cached(sql)?;
            let mut rows = stmt.query(params)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                result.push((row.get(0)?, text_or_blob(row, 1)?, text_or_blob(row, 2)?));
            }
            result
        };
        let mut regions = OfflineRegions::new();
        for (id, definition, metadata) in rows {
            let definition = match decode_offline_region_definition(&definition) {
                Ok(definition) => definition,
                Err(e) if skip_malformed => {
                    log::error!(target: log_target::GENERAL, "{}", e.message);
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            regions.push(OfflineRegion { id, definition, metadata });
        }
        Ok(regions)
    }

    /// Every region whose definition can be read.
    pub fn list_regions(&mut self) -> DbResult<OfflineRegions> {
        let result = self.read_regions("SELECT id, definition, description FROM regions", [], true);
        self.report(result, "list regions")
    }

    /// The region `region_id`, or `None` when the database has none.
    pub fn get_region(&mut self, region_id: i64) -> DbResult<Option<OfflineRegion>> {
        let sql = "SELECT id, definition, description FROM regions WHERE id = ?1";
        let result = self.read_regions(sql, params![region_id], false).map(|regions| regions.into_iter().next());
        self.report(result, "get region")
    }

    /// Creates a region without resources.
    pub fn create_region(
        &mut self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
    ) -> DbResult<OfflineRegion> {
        let result = (|| -> DbResult<OfflineRegion> {
            self.check_flags()?;
            self.execute(
                "INSERT INTO regions (definition, description) VALUES (?1, ?2)",
                params![encode_offline_region_definition(definition), metadata_blob(metadata)],
            )?;
            Ok(OfflineRegion {
                id: self.last_insert_row_id()?,
                definition: definition.clone(),
                metadata: metadata.clone(),
            })
        })();
        self.report(result, "create region")
    }

    /// How long the region's stored resources stay usable.
    pub fn get_region_expiry(&mut self, region_id: i64) -> DbResult<OfflineRegionExpiry> {
        let result = (|| -> DbResult<OfflineRegionExpiry> {
            let mut sql = Vec::new();
            for table in [Table::Tiles, Table::Resources] {
                let (name, (regions, id)) = (table.name(), table.regions());
                sql.push(format!(
                    "SELECT {CACHING_COLUMNS} FROM {regions} AS held JOIN {name} ON {name}.id = held.{id} \
                     WHERE held.region_id = ?1"
                ));
            }
            let now = now();
            let mut expiry = OfflineRegionExpiry::default();
            let mut stmt = self.get_db()?.prepare_cached(&sql.join(" UNION ALL "))?;
            let mut rows = stmt.query(params![region_id])?;
            while let Some(row) = rows.next()? {
                let caching = read_caching(row, 0)?;
                let earliest = |a: Option<Timestamp>, b: Option<Timestamp>| a.into_iter().chain(b).min();
                expiry.stale_after = earliest(expiry.stale_after, caching.expires);
                expiry.usable_offline_until = earliest(expiry.usable_offline_until, caching.usable_until());
                expiry.stale_resource_count += u64::from(!caching.is_fresh(now));
                expiry.unusable_resource_count += u64::from(!caching.is_usable(now));
            }
            Ok(expiry)
        })();
        self.report(result, "get region expiry")
    }

    pub fn merge_database(&mut self, side_database_path: &str) -> Result<OfflineRegions, DatabaseError> {
        self.check_flags()?;

        match self.execute("ATTACH DATABASE ?1 AS side", params![side_database_path]) {
            Ok(_) => {}
            Err(e @ DatabaseError::Sqlite { code, .. }) => {
                log::error!(
                    target: log_target::DATABASE,
                    code = i64::from(code & 0xff);
                    "Can't attach database ({side_database_path}) for merge: {e}"
                );
                return Err(e);
            }
            Err(e) => return Err(e),
        }

        let result = (|| -> DbResult<OfflineRegions> {
            // A sideloaded database has the main database's version, 6.
            let side_user_version = self.get_pragma_i64("PRAGMA side.user_version")?;
            let main_user_version = self.get_pragma_i64("PRAGMA user_version")?;
            if side_user_version != main_user_version {
                return Err(DatabaseError::Runtime("Merge database has incorrect user_version".to_owned()));
            }

            self.with_transaction(Begin::Deferred, |this| {
                exec(this.get_db()?, MERGE_SIDELOADED_DATABASE_SQL)?;
                Ok(())
            })?;

            let sql = "SELECT DISTINCT r.id, r.definition, r.description FROM side.regions sr \
                       JOIN regions r ON sr.definition = r.definition AND sr.description IS r.description \
                       ORDER BY r.id";
            let result = self.read_regions(sql, [], false)?;
            exec(self.get_db()?, "DETACH DATABASE side")?;
            Ok(result)
        })();

        match result {
            // `catch (const std::runtime_error& ex)`: everything but the
            // logic errors, which propagate (side stays attached).
            Err(e) if !matches!(e, DatabaseError::Logic(_)) => {
                exec(self.get_db()?, "DETACH DATABASE side")?;
                log::error!(target: log_target::DATABASE, "{e}");
                Err(e)
            }
            result => result,
        }
    }

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
        self.report(result, "update region metadata")
    }

    pub fn delete_region(&mut self, region: &OfflineRegion) -> DbResult<()> {
        let result = (|| -> DbResult<()> {
            self.check_flags()?;
            // Eviction uses the stored timestamps if this fails.
            self.flush_accessed();

            self.execute("DELETE FROM regions WHERE id = ?", params![region.id])?;

            let stats = DatabaseSizeChangeStats::new(self)?;
            self.evict(0, &stats, false)?;
            if self.autopack {
                self.vacuum()?;
            }
            self.update_ambient_cache_size(&stats)?;
            Ok(())
        })();
        self.report(result, "delete region")
    }

    /// The stored copy of `resource`, with its stored size, whether or not
    /// the ambient cache is enabled.
    pub fn get_region_resource(&mut self, resource: &Resource) -> Option<(Response, u64)> {
        let result = self.get_internal(resource);
        self.report(result, "read region resource").ok().flatten()
    }

    /// The stored size and caching rules of the copy of `resource`; `None`
    /// without one, or for a copy without data.
    pub fn check_region_resource(&mut self, resource: &Resource) -> Option<(u64, Caching)> {
        let result = self.has_internal(resource);
        self.report(result, "query region resource").ok().flatten()
    }

    pub fn put_region_resource(&mut self, region_id: i64, resource: &Resource, response: &Response) -> u64 {
        let result = (|| -> DbResult<u64> {
            self.check_flags()?;

            if self.db.is_none() {
                self.initialize()?;
            }
            self.with_transaction(Begin::Deferred, |this| {
                this.put_region_resource_internal(region_id, resource, response)
            })
        })();
        result.unwrap_or_else(|e| {
            self.handle_error(&e, "write region resource");
            0
        })
    }

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
            // Accumulate all statistics locally first before adding them to
            // the OfflineRegionStatus object to ensure correctness when the
            // transaction fails.
            let mut completed_resource_count = 0u64;
            let mut completed_resource_size = 0u64;
            let mut completed_tile_count = 0u64;
            let mut completed_tile_size = 0u64;

            self.with_transaction(Begin::Deferred, |this| {
                for (resource, response) in resources {
                    let resource_size = this.put_region_resource_internal(region_id, resource, response)?;
                    completed_resource_count += 1;
                    completed_resource_size += resource_size;
                    if resource.kind == Kind::Tile {
                        completed_tile_count += 1;
                        completed_tile_size += resource_size;
                    }
                }
                Ok(())
            })?;

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
            size = self.has_internal(resource)?.map_or(0, |(stored, _)| stored);
        }
        self.mark_used(region_id, resource)?;
        Ok(size)
    }

    /// Adds `resource`, which the database holds, to the region.
    fn mark_used(&mut self, region_id: i64, resource: &Resource) -> DbResult<()> {
        self.check_flags()?;
        let stored = Stored::of(resource);
        let table = stored.table();
        let (name, (regions, id)) = (table.name(), table.regions());
        let mut params: Vec<&dyn ToSql> = vec![&region_id];
        params.extend(stored.params());
        self.execute(
            &format!(
                "INSERT OR IGNORE INTO {regions} (region_id, {id}) SELECT ?, id FROM {name} WHERE {}",
                stored.condition()
            ),
            &*params,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get_region_definition(&mut self, region_id: i64) -> Result<OfflineRegionDefinition, DatabaseError> {
        let result = (|| -> DbResult<OfflineRegionDefinition> {
            // A missing region reads as an empty, malformed definition.
            let definition = self
                .query_row("SELECT definition FROM regions WHERE id = ?1", params![region_id], |row| {
                    text_or_blob(row, 0)
                })?
                .unwrap_or_default();

            Ok(decode_offline_region_definition(&definition)?)
        })();
        self.report(result, "load region")
    }

    /// The number and size of the resources that the region holds, tiles
    /// included, and of its tiles.
    pub fn get_region_completed_status(&mut self, region_id: i64) -> DbResult<OfflineRegionStatus> {
        let result = (|| -> DbResult<OfflineRegionStatus> {
            let mut status = OfflineRegionStatus::default();
            for table in [Table::Tiles, Table::Resources] {
                let (name, (regions, id)) = (table.name(), table.regions());
                let (count, size) = self
                    .query_row(
                        &format!(
                            "SELECT COUNT(*), IFNULL(SUM(LENGTH(data)), 0) FROM {regions} \
                             JOIN {name} ON {name}.id = {regions}.{id} WHERE region_id = ?1"
                        ),
                        params![region_id],
                        |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
                    )?
                    .unwrap_or_default();
                status.completed_resource_count += count;
                status.completed_resource_size += size;
                if let Table::Tiles = table {
                    status.completed_tile_count = count;
                    status.completed_tile_size = size;
                }
            }
            Ok(status)
        })();
        self.report(result, "get region status")
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
        let mut new_ambient_cache_size =
            self.ambient_cache_size()?.wrapping_add(needed_free_size).wrapping_add(stats.page_size());

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
                |row| Ok(row.get::<_, Option<i64>>(0)?),
            )?;
            let Some(accessed) = accessed.flatten() else {
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

            if resource_changes == 0 && tile_changes == 0 {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// The size of the ambient cache's rows, which it reads once and then
    /// keeps up to date.
    fn ambient_cache_size(&mut self) -> DbResult<u64> {
        if let Some(size) = self.current_ambient_cache_size {
            return Ok(size);
        }
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
            |row| Ok(row.get::<_, Option<i64>>(0)?),
        )?;
        let size = size.flatten().unwrap_or(0) as u64;
        self.current_ambient_cache_size = Some(size);
        Ok(size)
    }

    pub(crate) fn set_maximum_ambient_cache_size(&mut self, size: u64) -> DbResult<()> {
        // Eviction uses the stored timestamps if this fails.
        self.flush_accessed();
        let previous_maximum_ambient_cache_size = self.maximum_ambient_cache_size;

        let result = (|| -> DbResult<()> {
            let current = self.ambient_cache_size()?;
            self.maximum_ambient_cache_size = size;
            if current > size {
                let stats = DatabaseSizeChangeStats::new(self)?;
                self.evict(0, &stats, false)?;
                if self.autopack {
                    self.vacuum()?;
                }
                self.update_ambient_cache_size(&stats)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.maximum_ambient_cache_size = previous_maximum_ambient_cache_size;
        }
        self.report(result, "set maximum ambient cache size")
    }

    pub fn mark_used_resources(&mut self, region_id: i64, resources: &[Resource]) {
        let result = (|| -> DbResult<()> {
            if self.db.is_none() {
                self.initialize()?;
            }
            self.with_transaction(Begin::Deferred, |this| {
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

    pub(crate) fn pack(&mut self) -> DbResult<()> {
        let result = (|| -> DbResult<()> {
            if self.db.is_none() {
                self.initialize()?;
            }
            self.vacuum()
        })();
        self.report(result, "pack storage")
    }

    #[cfg(test)]
    pub(crate) fn run_pack_database_automatically(&mut self, autopack: bool) {
        self.autopack = autopack;
    }

    pub(crate) fn reopen_database_read_only(&mut self, read_only: bool) {
        if self.read_only == read_only {
            return;
        }
        self.cleanup();
        self.read_only = read_only;
        if let Err(e) = self.initialize() {
            self.handle_error(&e, "reopen database read-only");
        }
    }

    /// Deletes the database file and starts an empty one. Returns the error,
    /// if any.
    pub(crate) fn reset_database(&mut self) -> DbResult<()> {
        let result = self.remove_existing().and_then(|()| self.initialize());
        self.report(result, "reset database")
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

/// How a transaction takes its locks.
#[derive(Clone, Copy)]
enum Begin {
    Deferred,
    Immediate,
}

impl Begin {
    fn sql(self) -> &'static str {
        match self {
            Begin::Deferred => "BEGIN DEFERRED TRANSACTION",
            Begin::Immediate => "BEGIN IMMEDIATE TRANSACTION",
        }
    }
}

/// Runs every statement of `sql` to completion, as `sqlite3_exec` does.
/// rusqlite's `execute_batch` steps each statement only once, which leaves
/// e.g. `PRAGMA incremental_vacuum` after freeing one page.
fn exec(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    use rusqlite::fallible_iterator::FallibleIterator as _;

    let mut batch = rusqlite::Batch::new(conn, sql);
    while let Some(mut stmt) = batch.next()? {
        let mut rows = stmt.raw_query();
        while rows.next()?.is_some() {}
    }
    Ok(())
}

/// The bytes of a TEXT or BLOB column, such as a region's definition and
/// metadata; empty for NULL.
fn text_or_blob(row: &Row<'_>, index: usize) -> DbResult<Vec<u8>> {
    Ok(row.get_ref(index)?.as_bytes_or_null()?.unwrap_or_default().to_vec())
}

/// The metadata as a blob to bind. Empty metadata is stored as NULL.
fn metadata_blob(metadata: &OfflineRegionMetadata) -> Option<&[u8]> {
    if metadata.is_empty() { None } else { Some(metadata.as_slice()) }
}

/// A table of copies.
#[derive(Clone, Copy)]
enum Table {
    Tiles,
    Resources,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Tiles => "tiles",
            Table::Resources => "resources",
        }
    }

    /// The table that links the copies to the regions that hold them, and
    /// its column for a copy's id.
    fn regions(self) -> (&'static str, &'static str) {
        match self {
            Table::Tiles => ("region_tiles", "tile_id"),
            Table::Resources => ("region_resources", "resource_id"),
        }
    }
}

/// Where the database stores a resource's copy: a tile by its tileset and
/// coordinates in `tiles`, another resource by its cache key in `resources`.
enum Stored<'a> {
    Tile(&'a TileData),
    Resource { url: String, kind: i32 },
}

impl<'a> Stored<'a> {
    fn of(resource: &'a Resource) -> Self {
        match tile_data(resource) {
            Some(tile) => Stored::Tile(tile),
            None => Stored::Resource { url: OfflineDatabase::cache_key(resource), kind: resource.kind as i32 },
        }
    }

    fn is_tile(&self) -> bool {
        matches!(self, Stored::Tile(_))
    }

    fn table(&self) -> Table {
        if self.is_tile() { Table::Tiles } else { Table::Resources }
    }

    /// The columns that identify the row, with their values, in the order of
    /// `condition`'s parameters.
    fn key(&self) -> Vec<(&'static str, &dyn ToSql)> {
        match self {
            Stored::Tile(tile) => vec![
                ("url_template", &tile.url_template),
                ("pixel_ratio", &tile.pixel_ratio),
                ("x", &tile.x),
                ("y", &tile.y),
                ("z", &tile.z),
            ],
            Stored::Resource { url, .. } => vec![("url", url)],
        }
    }

    /// The condition that selects the row, over `params`.
    fn condition(&self) -> &'static str {
        if self.is_tile() { "url_template = ? AND pixel_ratio = ? AND x = ? AND y = ? AND z = ?" } else { "url = ?" }
    }

    fn params(&self) -> Vec<&dyn ToSql> {
        self.key().into_iter().map(|(_, value)| value).collect()
    }

    /// The columns besides the response's that a write of new content sets.
    fn content(&self) -> Vec<(&'static str, &dyn ToSql)> {
        match self {
            Stored::Tile(_) => Vec::new(),
            Stored::Resource { kind, .. } => vec![("kind", kind)],
        }
    }
}

/// Whether `response` brings a copy the database has: a 304, or the copy
/// the request carried, which a 304 delivered.
fn renews(response: &Response) -> bool {
    response.not_modified || response.prior
}

/// Reads `etag, modified, data, compressed, id`, then the caching columns:
/// the response, its stored size, and its row id.
fn read_response(row: &Row<'_>) -> DbResult<(Response, u64, i64)> {
    let mut response = Response::new();
    let mut size = 0u64;
    response.etag = row.get(0)?;
    response.modified = row.get::<_, Option<i64>>(1)?.map(Timestamp::from_secs);
    match row.get_ref(2)?.as_bytes_or_null()? {
        None => response.no_content = true,
        Some(data) => {
            size = data.len() as u64;
            let data = if row.get(3)? { decompress(data).map_err(DatabaseError::Runtime)? } else { data.to_vec() };
            response.data = Some(Arc::new(data));
        }
    }
    response.caching = read_caching(row, 5)?;
    Ok((response, size, row.get(4)?))
}

/// The columns that `read_caching` reads.
const CACHING_COLUMNS: &str = "expires, must_revalidate";

/// Reads `CACHING_COLUMNS` from column `start` on.
fn read_caching(row: &Row<'_>, start: usize) -> DbResult<Caching> {
    Ok(Caching {
        expires: row.get::<_, Option<i64>>(start)?.map(Timestamp::from_secs),
        must_revalidate: row.get(start + 1)?,
    })
}
// The cached value of offlineTileCount does not need to be
// updated here because only non-offline tiles can be removed by
// eviction.
