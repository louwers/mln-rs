use super::{FixtureLog, cpp_path};
use crate::sqlite3::{self, Database, OpenFlag, Query, Statement, Transaction, TransactionMode};

// gtest: SQLite.InMemoryDatabaseFilename
#[test]
fn in_memory_database_filename() {
    for path in ["", ":memory:", "file::memory:", "file::memory:?cache=shared", "file:filename-test?mode=memory"] {
        let db = Database::open(path, OpenFlag::ReadWriteCreate).unwrap();
        assert!(db.get_filename().is_empty(), "{path}");
    }
}

// gtest: SQLite.Statement
#[test]
fn statement() {
    let db = Database::open(":memory:", OpenFlag::ReadWriteCreate).unwrap();
    db.exec("CREATE TABLE test (id INTEGER);").unwrap();
    let conn = db.conn();

    let mut stmt1 = Statement::new(&conn, "INSERT INTO test (id) VALUES (?1);").unwrap();
    let mut query1 = Query::new(&mut stmt1);
    assert_eq!(query1.last_insert_row_id(), 0);
    assert_eq!(query1.changes(), 0);
    query1.bind(1, 10).unwrap();
    query1.run().unwrap();
    assert_eq!(query1.last_insert_row_id(), 1);
    assert_eq!(query1.changes(), 1);

    let mut stmt2 = Statement::new(&conn, "INSERT INTO test (id) VALUES (?1);").unwrap();
    let mut query2 = Query::new(&mut stmt2);
    assert_eq!(query2.last_insert_row_id(), 0);
    assert_eq!(query2.changes(), 0);
    query2.bind(1, 20).unwrap();
    query2.run().unwrap();
    assert_eq!(query2.last_insert_row_id(), 2);
    assert_eq!(query2.changes(), 1);
}

// gtest: SQLite.TryOpen
#[test]
fn try_open() {
    let log = FixtureLog::new();

    // Should return a CANTOPEN exception when the database doesn't exist,
    // make sure all the backends behave the same way.
    let path = cpp_path("test/fixtures/offline_database/foobar123.db");
    let result = Database::try_open(path.to_str().unwrap(), OpenFlag::ReadOnly);
    let Err(exception) = result else {
        panic!("expected an exception");
    };
    assert_eq!(exception.code, sqlite3::result_code::CANT_OPEN);
    assert_eq!(0, log.unchecked_count(), "{:?}", log.unchecked());
}

// gtest: SQLite.CloseDatabaseWithPendingTransaction
#[test]
fn close_database_with_pending_transaction() {
    let db = Box::new(Database::open(":memory:", OpenFlag::ReadWriteCreate).unwrap());
    let mut transaction = Transaction::new(&db, TransactionMode::Deferred).unwrap();
    transaction.commit().unwrap();
}

// gtest: SQLite.CloseMovedDatabaseWithPendingTransaction
#[test]
fn close_moved_database_with_pending_transaction() {
    // Verifies that we can correctly commit a transaction even if we move
    // the Database object to another address.
    let db1 = Database::open(":memory:", OpenFlag::ReadWriteCreate).unwrap();
    let db2: Box<Database>;
    let mut transaction = Transaction::new(&db1, TransactionMode::Deferred).unwrap();
    db2 = Box::new(db1);
    transaction.commit().unwrap();
    drop(db2);
}

/// The text SQLite 3.45 gives for these reals, from `sqlite3_mprintf("%!.15g")`.
#[test]
fn real_to_text() {
    let cases = [
        (1.0, "1.0"),
        (-0.0, "0.0"),
        (0.1 + 0.2, "0.3"),
        (100.0, "100.0"),
        (-2.5, "-2.5"),
        (123_456_789_012_345.67, "123456789012346.0"),
        (999_999_999_999_999.0, "999999999999999.0"),
        (9_999_999_999_999_999.0, "1.0e+16"),
        (1e15, "1.0e+15"),
        // A tie at the 16th digit rounds up.
        (1_234_567_890_123_445.0, "1.23456789012345e+15"),
        (1e-4, "0.0001"),
        (1e-5, "1.0e-05"),
        (1.5e-7, "1.5e-07"),
        (1e100, "1.0e+100"),
        (f64::MAX, "1.79769313486232e+308"),
        (5e-324, "4.94065645841247e-324"),
        (f64::INFINITY, "Inf"),
        (f64::NEG_INFINITY, "-Inf"),
    ];
    for (real, text) in cases {
        assert_eq!(sqlite3::real_to_text(real), text, "{real:e}");
    }

    let db = Database::open(":memory:", OpenFlag::ReadWriteCreate).unwrap();
    let conn = db.conn();
    let mut stmt = Statement::new(&conn, "SELECT 1.0, 1e15").unwrap();
    let mut query = Query::new(&mut stmt);
    assert!(query.run().unwrap());
    assert_eq!(query.get_string(0).unwrap(), "1.0");
    assert_eq!(query.get_string(1).unwrap(), "1.0e+15");
}
