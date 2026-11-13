// `Database` holds its connection the way C++ holds its `DatabaseImpl`
// (`std::unique_ptr`): behind a pointer that a `Transaction` shares, so a
// transaction keeps working when the `Database` object moves. The pointer
// is an `Arc<Mutex<Connection>>` (the mutex makes it `Send`; it is never
// contended). `conn()` gives the connection for rusqlite calls.
//
// `Statement`/`Query` are rusqlite statements; the typed `Query::get<T>`
// overloads are the `get_*` functions below, which apply SQLite's own
// column conversions (`sqlite3_column_int64` of a text value parses it,
// `sqlite3_column_blob` of a number formats it, ...). The offline database
// uses rusqlite's statement cache (`prepare_cached`) directly.

use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
#[cfg(test)]
use rusqlite::types::ToSql;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row, Rows};

/// SQLite result codes (`SQLITE_*`).
pub(crate) mod result_code {
    pub(crate) const ERROR: i32 = 1;
    pub(crate) const READ_ONLY: i32 = 8;
    pub(crate) const CORRUPT: i32 = 11;
    #[cfg(test)]
    pub(crate) const CANT_OPEN: i32 = 14;
    #[cfg(test)]
    pub(crate) const CONSTRAINT: i32 = 19;
    pub(crate) const NOT_A_DB: i32 = 26;
}

/// SQLite extended result codes, without the primary code (`err >> 8`).
pub(crate) mod extended_result_code {
    /// `SQLITE_READONLY_DBMOVED >> 8`.
    pub(crate) const READ_ONLY_DB_MOVED: i32 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenFlag {
    ReadOnly = 0x0000_0001,
    ReadWriteCreate = 0x0000_0006,
}

/// An SQLite error. `code` is the low byte of the (extended) result code;
/// `extended_code` is `err >> 8`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SqliteError {
    pub(crate) code: i32,
    pub(crate) extended_code: i32,
    pub(crate) message: String,
}

impl SqliteError {
    pub(crate) fn new(err: i32, message: impl Into<String>) -> Self {
        SqliteError { code: err & 0xff, extended_code: (err >> 8) & 0xff, message: message.into() }
    }
}

impl From<rusqlite::Error> for SqliteError {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(error, message) => {
                SqliteError::new(error.extended_code, message.clone().unwrap_or_else(|| error.to_string()))
            }
            _ => SqliteError::new(result_code::ERROR, e.to_string()),
        }
    }
}

pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    /// Opens `filename` with `flags` and `SQLITE_OPEN_URI`, with extended
    /// result codes on.
    pub fn try_open(filename: &str, flags: OpenFlag) -> Result<Database, SqliteError> {
        let flags = OpenFlags::from_bits_retain(flags as i32) | OpenFlags::SQLITE_OPEN_URI;
        let conn = Connection::open_with_flags(filename, flags)?;
        Ok(Database { connection: Arc::new(Mutex::new(conn)) })
    }

    /// The same as `try_open`.
    pub(crate) fn open(filename: &str, flags: OpenFlag) -> Result<Database, SqliteError> {
        Self::try_open(filename, flags)
    }

    #[must_use]
    pub(crate) fn get_filename(&self) -> String {
        self.conn().path().unwrap_or("").to_owned()
    }

    /// Sets the busy timeout, clamped to `0..=i32::MAX` milliseconds.
    pub(crate) fn set_busy_timeout(&self, timeout_ms: i64) -> Result<(), SqliteError> {
        let ms = timeout_ms.clamp(0, i64::from(i32::MAX)) as u64;
        self.conn().busy_timeout(std::time::Duration::from_millis(ms))?;
        Ok(())
    }

    /// `exec(sql)`.
    pub(crate) fn exec(&self, sql: &str) -> Result<(), SqliteError> {
        exec(&self.conn(), sql)
    }

    /// The connection.
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock()
    }
}

/// Runs every statement of `sql` to completion, as `sqlite3_exec` does.
/// rusqlite's `execute_batch` steps each statement only once, which leaves
/// e.g. `PRAGMA incremental_vacuum` after freeing one page.
fn exec(conn: &Connection, sql: &str) -> Result<(), SqliteError> {
    let mut batch = rusqlite::Batch::new(conn, sql);
    while let Some(mut stmt) = batch.next()? {
        let mut rows = stmt.raw_query();
        while rows.next()?.is_some() {}
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransactionMode {
    Deferred,
    Immediate,
}

/// A transaction that rolls back on drop unless committed or rolled back.
pub(crate) struct Transaction {
    connection: Arc<Mutex<Connection>>,
    need_rollback: bool,
}

impl Transaction {
    /// `Transaction(Database&, Mode = Deferred)`.
    pub(crate) fn new(db: &Database, mode: TransactionMode) -> Result<Transaction, SqliteError> {
        let sql = match mode {
            TransactionMode::Deferred => "BEGIN DEFERRED TRANSACTION",
            TransactionMode::Immediate => "BEGIN IMMEDIATE TRANSACTION",
        };
        // C++ throws from the constructor: no destructor, no rollback.
        db.exec(sql)?;
        Ok(Transaction { connection: Arc::clone(&db.connection), need_rollback: true })
    }

    fn exec(&self, sql: &str) -> Result<(), SqliteError> {
        exec(&self.connection.lock(), sql)
    }

    pub(crate) fn commit(&mut self) -> Result<(), SqliteError> {
        self.need_rollback = false;
        self.exec("COMMIT TRANSACTION")
    }

    pub(crate) fn rollback(&mut self) -> Result<(), SqliteError> {
        self.need_rollback = false;
        self.exec("ROLLBACK TRANSACTION")
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.need_rollback {
            // Ignore failed rollbacks in destructor.
            let _ = self.rollback();
        }
    }
}

/// A prepared statement that can be run repeatedly with a `Query`. Borrows
/// the connection guard it was prepared on (`db.conn()`).
pub(crate) struct Statement<'conn> {
    stmt: rusqlite::Statement<'conn>,
    conn: &'conn Connection,
    last_insert_row_id: i64,
    changes: i64,
}

impl<'conn> Statement<'conn> {
    /// `Statement(Database&, const char* sql)` (`sqlite3_prepare_v2`).
    pub(crate) fn new(conn: &'conn Connection, sql: &str) -> Result<Statement<'conn>, SqliteError> {
        Ok(Statement { stmt: conn.prepare(sql)?, conn, last_insert_row_id: 0, changes: 0 })
    }
}

/// Binds and runs a `Statement` one row at a time.
pub(crate) struct Query<'s, 'conn> {
    stmt: Option<&'s mut rusqlite::Statement<'conn>>,
    rows: Option<Rows<'s>>,
    conn: &'conn Connection,
    last_insert_row_id: &'s mut i64,
    changes: &'s mut i64,
}

impl<'s, 'conn> Query<'s, 'conn> {
    /// `Query(Statement&)`.
    pub(crate) fn new(statement: &'s mut Statement<'conn>) -> Self {
        let Statement { stmt, conn, last_insert_row_id, changes } = statement;
        Query { stmt: Some(stmt), rows: None, conn, last_insert_row_id, changes }
    }

    /// `bind(offset, value)` (before the first `run()`).
    #[cfg(test)]
    pub(crate) fn bind<T: ToSql>(&mut self, offset: usize, value: T) -> Result<(), SqliteError> {
        let stmt = self.stmt.as_mut().expect("Query::bind after run()");
        stmt.raw_bind_parameter(offset, value)?;
        Ok(())
    }

    /// Steps once; true when a row is available.
    pub(crate) fn run(&mut self) -> Result<bool, SqliteError> {
        if self.rows.is_none() {
            let stmt = self.stmt.take().expect("statement");
            self.rows = Some(stmt.raw_query());
        }
        let result = self.rows.as_mut().expect("rows").next().map(|row| row.is_some());
        *self.last_insert_row_id = self.conn.last_insert_rowid();
        *self.changes = self.conn.changes() as i64;
        Ok(result?)
    }

    fn row(&self) -> &Row<'s> {
        self.rows.as_ref().and_then(|rows| rows.get()).expect("Query::get without a row")
    }

    #[cfg(test)]
    pub(crate) fn get_i64(&self, offset: usize) -> Result<i64, SqliteError> {
        get_i64(self.row(), offset)
    }

    #[cfg(test)]
    pub(crate) fn get_i32(&self, offset: usize) -> Result<i32, SqliteError> {
        get_i32(self.row(), offset)
    }

    /// The column at `offset` as bytes (see the free `get_bytes`).
    pub(crate) fn get_bytes(&self, offset: usize) -> Result<Vec<u8>, SqliteError> {
        get_bytes(self.row(), offset)
    }

    /// The column at `offset` as bytes; `None` for NULL.
    pub(crate) fn get_optional_bytes(&self, offset: usize) -> Result<Option<Vec<u8>>, SqliteError> {
        get_optional_bytes(self.row(), offset)
    }

    /// The column at `offset` as text, lossy for invalid UTF-8.
    pub(crate) fn get_string(&self, offset: usize) -> Result<String, SqliteError> {
        Ok(String::from_utf8_lossy(&get_bytes(self.row(), offset)?).into_owned())
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn last_insert_row_id(&self) -> i64 {
        *self.last_insert_row_id
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn changes(&self) -> u64 {
        (*self.changes).max(0) as u64
    }
}

/// `sqlite3_column_int64`.
pub(crate) fn get_i64(row: &Row<'_>, offset: usize) -> Result<i64, SqliteError> {
    Ok(match row.get_ref(offset)? {
        ValueRef::Null => 0,
        ValueRef::Integer(i) => i,
        ValueRef::Real(r) => real_to_i64(r),
        ValueRef::Text(t) | ValueRef::Blob(t) => text_to_i64(t),
    })
}

/// `sqlite3_column_int` (the low 32 bits of the 64-bit value).
pub(crate) fn get_i32(row: &Row<'_>, offset: usize) -> Result<i32, SqliteError> {
    Ok(get_i64(row, offset)? as i32)
}

/// Whether the column's `i32` value is nonzero.
pub(crate) fn get_bool(row: &Row<'_>, offset: usize) -> Result<bool, SqliteError> {
    Ok(get_i32(row, offset)? != 0)
}

pub(crate) fn get_optional_i64(row: &Row<'_>, offset: usize) -> Result<Option<i64>, SqliteError> {
    if matches!(row.get_ref(offset)?, ValueRef::Null) {
        return Ok(None);
    }
    get_i64(row, offset).map(Some)
}

/// The bytes of a text or blob value, or a number's text form, as
/// `sqlite3_column_blob` gives them; empty for NULL.
pub(crate) fn get_bytes(row: &Row<'_>, offset: usize) -> Result<Vec<u8>, SqliteError> {
    Ok(match row.get_ref(offset)? {
        ValueRef::Null => Vec::new(),
        ValueRef::Integer(i) => i.to_string().into_bytes(),
        // Rust's shortest round-trip formatting, not SQLite's "%!.15g": the
        // text is only parsed back as a number (MBTiles zooms and scale).
        ValueRef::Real(r) => format!("{r}").into_bytes(),
        ValueRef::Text(t) | ValueRef::Blob(t) => t.to_vec(),
    })
}

/// The value as bytes (see `get_bytes`); `None` for NULL.
pub(crate) fn get_optional_bytes(row: &Row<'_>, offset: usize) -> Result<Option<Vec<u8>>, SqliteError> {
    if matches!(row.get_ref(offset)?, ValueRef::Null) {
        return Ok(None);
    }
    get_bytes(row, offset).map(Some)
}

/// The value as text; `None` for NULL. Lossy for invalid UTF-8, which C++
/// would keep.
pub(crate) fn get_optional_string(row: &Row<'_>, offset: usize) -> Result<Option<String>, SqliteError> {
    Ok(get_optional_bytes(row, offset)?.map(|bytes| match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }))
}

/// SQLite's REAL to INTEGER conversion (`sqlite3RealToI64`: saturating).
fn real_to_i64(r: f64) -> i64 {
    if r.is_nan() {
        0
    } else if r <= i64::MIN as f64 {
        i64::MIN
    } else if r >= i64::MAX as f64 {
        i64::MAX
    } else {
        r as i64
    }
}

/// SQLite's TEXT to INTEGER conversion for `sqlite3_column_int64`: the
/// longest integer prefix after leading spaces, saturating.
fn text_to_i64(t: &[u8]) -> i64 {
    let mut i = 0;
    while i < t.len() && t[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut negative = false;
    if i < t.len() && (t[i] == b'-' || t[i] == b'+') {
        negative = t[i] == b'-';
        i += 1;
    }
    let mut value: i128 = 0;
    while i < t.len() && t[i].is_ascii_digit() {
        value = (value * 10 + i128::from(t[i] - b'0')).min(i128::from(i64::MAX) + 1);
        i += 1;
    }
    if negative {
        value = -value;
    }
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}
