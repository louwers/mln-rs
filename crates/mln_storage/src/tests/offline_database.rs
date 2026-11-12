// Ports test/storage/offline_database.test.cpp.
//
// C++ writes test/fixtures/offline_database/offline.db (and
// offline_sideload.db) next to the fixtures; every port writes a fresh file
// in the temporary directory instead (the Rust harness runs tests in
// parallel), copied from the fixtures in testdata/.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_util::chrono::{Timestamp, now};
use mln_util::constants::DEFAULT_MAX_CACHE_SIZE;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::geometry::{Geometry, Point};
use mln_util::logging::{Event, EventSeverity};
use mln_util::tileset::Scheme;

use super::{
    FixtureLog, Message, TempPath, copy_file, cpp_path, delete_file, error, file_size, random_string, temp_path,
    warning,
};
use crate::offline::{
    OfflineGeometryRegionDefinition, OfflineRegionDefinition, OfflineRegionMetadata, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use crate::offline_database::OfflineDatabase;
use crate::resource::{Kind, LoadingMethod, Resource, TileData};
use crate::response::{Reason, ResourceError, Response};
use crate::sqlite3::{Database, OpenFlag, Query, Statement, result_code};

fn fixture(name: &str) -> PathBuf {
    cpp_path(&format!("test/fixtures/offline_database/{name}"))
}

/// `filename` ("`test/fixtures/offline_database/offline.db")`: a fresh path.
fn filename() -> TempPath {
    temp_path("offline.db")
}

/// `filename_sideload` ("`test/fixtures/offline_database/offline_sideload.db`").
fn filename_sideload() -> TempPath {
    temp_path("offline_sideload.db")
}

fn delete_database_files(filename: &str) {
    // Delete leftover journaling files as well.
    delete_file(filename);
    delete_file(&format!("{filename}-wal"));
    delete_file(&format!("{filename}-journal"));
}

fn query_pragma_i32(path: &str, sql: &str) -> i32 {
    let db = Database::open(path, OpenFlag::ReadOnly).unwrap();
    let conn = db.conn();
    let mut stmt = Statement::new(&conn, sql).unwrap();
    let mut query = Query::new(&mut stmt);
    query.run().unwrap();
    query.get_i32(0).unwrap()
}

fn database_page_count(path: &str) -> i32 {
    query_pragma_i32(path, "pragma page_count")
}

fn database_user_version(path: &str) -> i32 {
    query_pragma_i32(path, "pragma user_version")
}

fn database_journal_mode(path: &str) -> String {
    let db = Database::open(path, OpenFlag::ReadOnly).unwrap();
    let conn = db.conn();
    let mut stmt = Statement::new(&conn, "pragma journal_mode").unwrap();
    let mut query = Query::new(&mut stmt);
    query.run().unwrap();
    query.get_string(0).unwrap()
}

fn database_sync_mode(path: &str) -> i32 {
    query_pragma_i32(path, "pragma synchronous")
}

fn database_table_columns(path: &str, name: &str) -> Vec<String> {
    let db = Database::open(path, OpenFlag::ReadOnly).unwrap();
    let conn = db.conn();
    let sql = format!("pragma table_info({name})");
    let mut stmt = Statement::new(&conn, &sql).unwrap();
    let mut query = Query::new(&mut stmt);
    let mut columns = Vec::new();
    while query.run().unwrap() {
        columns.push(query.get_string(1).unwrap());
    }
    columns
}

fn database_auto_vacuum(path: &str) -> i32 {
    query_pragma_i32(path, "pragma auto_vacuum")
}

mod fixture {
    use super::*;

    pub fn resource() -> Resource {
        Resource::new(Kind::Style, "https://example.com/test")
    }

    pub fn tile() -> Resource {
        Resource::tile("https://example.com/test", 1.0, 0, 0, 0, Scheme::XYZ, LoadingMethod::All)
    }

    pub fn response() -> Response {
        let mut res = Response::new();
        res.data = Some(Arc::new(b"first".to_vec()));
        res
    }
}

fn data(s: &str) -> crate::response::Data {
    Arc::new(s.as_bytes().to_vec())
}

fn removing() -> Message {
    warning(-1, "Removing existing incompatible offline database")
}

fn message(severity: EventSeverity, code: i64, msg: &str) -> Message {
    Message { severity, event: Event::Database, code, msg: msg.to_owned() }
}

fn hull(a: (f64, f64), b: (f64, f64)) -> LatLngBounds {
    LatLngBounds::hull(LatLng::new(a.0, a.1).unwrap(), LatLng::new(b.0, b.1).unwrap())
}

fn tile_pyramid(
    style_url: &str,
    bounds: LatLngBounds,
    min_zoom: f64,
    max_zoom: f64,
    pixel_ratio: f32,
    include_ideographs: bool,
) -> OfflineTilePyramidRegionDefinition {
    OfflineTilePyramidRegionDefinition::new(style_url, bounds, min_zoom, max_zoom, pixel_ratio, include_ideographs)
        .unwrap()
}

fn style(url: &str) -> Resource {
    Resource::style(url)
}

fn tile(url: &str, pixel_ratio: f32, x: i32, y: i32, z: i8) -> Resource {
    Resource::tile(url, pixel_ratio, x, y, z, Scheme::XYZ, LoadingMethod::All)
}

fn check_log_empty(log: &FixtureLog) {
    assert_eq!(0, log.unchecked_count(), "{:?}", log.unchecked());
}

/// The checks of an `OfflineTilePyramidRegionDefinition` the region tests
/// repeat (`std::visit(overloaded{...})`).
fn expect_same_tile_pyramid(definition: &OfflineTilePyramidRegionDefinition, region: &OfflineRegionDefinition) {
    if let OfflineRegionDefinition::TilePyramid(def) = region {
        assert_eq!(definition.style_url, def.style_url);
        assert_eq!(definition.bounds, def.bounds);
        assert_eq!(definition.min_zoom, def.min_zoom);
        assert_eq!(definition.max_zoom, def.max_zoom);
        assert_eq!(definition.pixel_ratio, def.pixel_ratio);
        assert_eq!(definition.include_ideographs, def.include_ideographs);
    }
}

// gtest: OfflineDatabase.Create
#[test]
fn create() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    assert!(db.get(&Resource::new(Kind::Unknown, "https://example.com/test")).is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.SchemaVersion
#[test]
fn schema_version() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    {
        let db = Database::open(&filename, OpenFlag::ReadWriteCreate).unwrap();
        db.set_busy_timeout(1000).unwrap();
        db.exec("PRAGMA user_version = 1").unwrap();
    }

    {
        let _db = OfflineDatabase::new(&filename);
    }

    assert_eq!(6, database_user_version(&filename));

    let mut db = OfflineDatabase::new(&filename);
    // Now try inserting and reading back to make sure we have a valid
    // database.
    for res in [fixture::resource(), fixture::tile()] {
        assert_eq!((true, 5), db.put(&res, &fixture::response()));
        check_log_empty(&log);
        let result = db.get(&res);
        check_log_empty(&log);
        let result = result.expect("result");
        assert_eq!(b"first", result.data.expect("data").as_slice());
    }

    check_log_empty(&log);
}

// gtest: OfflineDatabase.Invalid
#[test]
fn invalid() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    std::fs::write(&filename, "this is an invalid file").unwrap();

    let mut db = OfflineDatabase::new(&filename);
    // Checking two possibilities for the error string because it apparently
    // changes between SQLite versions.
    assert_eq!(
        1,
        log.count(&error(result_code::NOT_A_DB, "Can't open database: file is encrypted or is not a database"), true)
            + log.count(&error(result_code::NOT_A_DB, "Can't open database: file is not a database"), true)
    );
    assert_eq!(1, log.count(&removing(), false));

    // Now try inserting and reading back to make sure we have a valid
    // database.
    for res in [fixture::resource(), fixture::tile()] {
        assert_eq!((true, 5), db.put(&res, &fixture::response()));
        check_log_empty(&log);
        let result = db.get(&res);
        check_log_empty(&log);
        let result = result.expect("result");
        assert_eq!(b"first", result.data.expect("data").as_slice());
    }
}

// gtest: OfflineDatabase.PutDoesNotStoreConnectionErrors
#[test]
fn put_does_not_store_connection_errors() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Unknown, "http://example.com/");
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Connection, "")));

    db.put(&resource, &response);
    assert!(db.get(&resource).is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutDoesNotStoreServerErrors
#[test]
fn put_does_not_store_server_errors() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Unknown, "http://example.com/");
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Server, "")));

    db.put(&resource, &response);
    assert!(db.get(&resource).is_none());

    check_log_empty(&log);
}

// Port-only: C++ dereferences a tile resource's tile data unchecked; one
// built without it is stored and found by its URL.
#[test]
fn tile_resource_without_tile_data() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Tile, "http://example.com/0/0/0.pbf");
    let mut response = Response::new();
    response.data = Some(data("tile"));
    assert!(db.put(&resource, &response).0);
    assert_eq!(b"tile", db.get(&resource).expect("response").data.unwrap().as_slice());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutResource
#[test]
fn put_resource() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Style, "http://example.com/");
    let mut response = Response::new();

    response.data = Some(data("first"));
    let insert_put_result = db.put(&resource, &response);
    assert!(insert_put_result.0);
    assert_eq!(5, insert_put_result.1);

    let insert_get_result = db.get(&resource).expect("response");
    assert!(insert_get_result.error.is_none());
    assert_eq!(b"first", insert_get_result.data.unwrap().as_slice());

    response.data = Some(data("second"));
    let update_put_result = db.put(&resource, &response);
    assert!(!update_put_result.0);
    assert_eq!(6, update_put_result.1);

    let update_get_result = db.get(&resource).expect("response");
    assert!(update_get_result.error.is_none());
    assert_eq!(b"second", update_get_result.data.unwrap().as_slice());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.GetResourceFromOfflineRegion
#[test]
fn get_resource_from_offline_region() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("satellite_test.db"));

    let mut db = OfflineDatabase::new(&filename);

    let resource = Resource::style("maptiler://maps/hybrid");
    assert!(db.get(&resource).is_some());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutAndGetResource
#[test]
fn put_and_get_resource() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let mut response1 = Response::new();
    response1.data = Some(data("foobar"));

    let resource = Resource::style("https://example.com/style");

    db.put(&resource, &response1);

    let response2 = db.get(&resource);

    assert_eq!(response1.data.unwrap(), response2.unwrap().data.unwrap());

    check_log_empty(&log);
}

fn example_tile() -> Resource {
    Resource::with_tile_data(
        Kind::Tile,
        "http://example.com/",
        Some(TileData { url_template: "http://example.com/".to_owned(), pixel_ratio: 1, x: 0, y: 0, z: 0 }),
        LoadingMethod::All,
    )
}

// gtest: OfflineDatabase.PutTile
#[test]
fn put_tile() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = example_tile();
    let mut response = Response::new();

    response.data = Some(data("first"));
    let insert_put_result = db.put(&resource, &response);
    assert!(insert_put_result.0);
    assert_eq!(5, insert_put_result.1);

    let insert_get_result = db.get(&resource).expect("response");
    assert!(insert_get_result.error.is_none());
    assert_eq!(b"first", insert_get_result.data.unwrap().as_slice());

    response.data = Some(data("second"));
    let update_put_result = db.put(&resource, &response);
    assert!(!update_put_result.0);
    assert_eq!(6, update_put_result.1);

    let update_get_result = db.get(&resource).expect("response");
    assert!(update_get_result.error.is_none());
    assert_eq!(b"second", update_get_result.data.unwrap().as_slice());

    check_log_empty(&log);
}

// Cache-key encoding for byte-range sources (eg. PMTiles). The range is
// appended to the URL so distinct ranges occupy distinct cache rows under the
// resources table's UNIQUE(url) constraint.
// gtest: OfflineDatabase.CacheKey
#[test]
fn cache_key() {
    let bare = Resource::new(Kind::Source, "https://example.com/x.pmtiles");
    assert_eq!("https://example.com/x.pmtiles", OfflineDatabase::cache_key(&bare));

    let mut ranged = Resource::new(Kind::Source, "https://example.com/x.pmtiles");
    ranged.data_range = Some((0, 126));
    assert_eq!("https://example.com/x.pmtiles?_mlnRange=0-126", OfflineDatabase::cache_key(&ranged));

    let mut ranged_with_query = Resource::new(Kind::Source, "https://example.com/x.pmtiles?token=abc");
    ranged_with_query.data_range = Some((200, 399));
    assert_eq!(
        "https://example.com/x.pmtiles?token=abc&_mlnRange=200-399",
        OfflineDatabase::cache_key(&ranged_with_query)
    );
}

// gtest: OfflineDatabase.PutResourceNoContent
#[test]
fn put_resource_no_content() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Style, "http://example.com/");
    let mut response = Response::new();
    response.no_content = true;

    db.put(&resource, &response);
    let res = db.get(&resource).expect("response");
    assert!(res.error.is_none());
    assert!(res.no_content);
    assert!(res.data.is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutTileNotFound
#[test]
fn put_tile_not_found() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = example_tile();
    let mut response = Response::new();
    response.no_content = true;

    db.put(&resource, &response);
    let res = db.get(&resource).expect("response");
    assert!(res.error.is_none());
    assert!(res.no_content);
    assert!(res.data.is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.CreateRegion
#[test]
fn create_region() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];
    let region = db.create_region(&definition.clone().into(), &metadata);
    let region = region.expect("region");

    check_log_empty(&log);

    expect_same_tile_pyramid(&definition, region.get_definition());
    assert_eq!(&metadata, region.get_metadata());
}

// gtest: OfflineDatabase.UpdateMetadata
#[test]
fn update_metadata() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];
    let region = db.create_region(&definition.into(), &metadata).expect("region");

    let newmetadata: OfflineRegionMetadata = vec![4, 5, 6];
    assert!(db.update_metadata(region.get_id(), &newmetadata).is_ok());
    let new_region = db.get_region(region.get_id()).unwrap().expect("region");
    assert_eq!(new_region.get_metadata(), &newmetadata);

    check_log_empty(&log);
}

// gtest: OfflineDatabase.ListRegions
#[test]
fn list_regions() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, false);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];

    let region = db.create_region(&definition.clone().into(), &metadata).expect("region");
    let regions = db.list_regions().unwrap();

    assert_eq!(1, regions.len());

    assert_eq!(region.get_id(), regions[0].get_id());
    expect_same_tile_pyramid(&definition, regions[0].get_definition());
    assert_eq!(&metadata, regions[0].get_metadata());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.GetRegion
#[test]
fn get_region() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, false);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];

    let created_region = db.create_region(&definition.clone().into(), &metadata).expect("region");
    let region_id = created_region.get_id();
    let region = db.get_region(region_id).unwrap().expect("region");

    assert_eq!(region_id, region.get_id());
    expect_same_tile_pyramid(&definition, region.get_definition());
    assert_eq!(&metadata, region.get_metadata());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.GetRegionDefinition
#[test]
fn get_region_definition() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, false);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];

    check_log_empty(&log);

    let region = db.create_region(&definition.clone().into(), &metadata).expect("region");
    let region_definition = db.get_region_definition(region.get_id()).unwrap();

    expect_same_tile_pyramid(&definition, &region_definition);
}

// Disabled due to flakiness: https://github.com/mapbox/mapbox-gl-native/issues/14966
// gtest: OfflineDatabase.DISABLED_MaximumAmbientCacheSize
#[test]
#[ignore = "disabled in C++ (DISABLED_, flaky: mapbox/mapbox-gl-native#14966)"]
fn disabled_maximum_ambient_cache_size() {
    let _log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    let database_size = || file_size(&filename);

    {
        let _db = OfflineDatabase::new(&filename);
    }

    let initial_size = file_size(&filename);
    let mut maximum_size = 50 * 1024 * 1024;

    let mut response = Response::new();
    response.data = Some(random_string(100 * 1024));

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64); // 50 MB

        let definition =
            tile_pyramid("https://example.com/masp/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
        let metadata: OfflineRegionMetadata = vec![1, 2, 3];

        let region = db.create_region(&definition.into(), &metadata).unwrap();

        // Add 100 MB of resources (50/50 ambient/region)
        for i in 0..250 {
            let ambient_tile = tile(&format!("https://example.com/tiles/tiles/ambient_tile_{i}"), 1.0, 0, 0, 0);
            db.put(&ambient_tile, &response);

            let ambient_style = style(&format!("https://example.com/maps/ambient_style_{i}"));
            db.put(&ambient_style, &response);

            let region_tile = tile(&format!("https://example.com/tiles/tiles/region_tile_{i}"), 1.0, 0, 0, 0);
            let _ = db.put_region_resource(region.get_id(), &region_tile, &response);

            let region_style = style(&format!("https://example.com/maps/region_style_{i}"));
            let _ = db.put_region_resource(region.get_id(), &region_style, &response);
        }
    }

    // We are adding about 50 MB of "region" data and 50 MB, of "ambient"
    // data. The effective size of the ambient cache will be zero here
    // because it will try to make room for the region data.
    assert!(database_size() >= maximum_size);
    assert!(database_size() <= 60 * 1024 * 1024);

    maximum_size = 30 * 1024 * 1024;

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64); // 30 MB
    }

    // Setting a new size to the ambient cache should have no effect because
    // it is all taken by offline region anyway.
    assert!(database_size() >= maximum_size);
    assert!(database_size() <= 60 * 1024 * 1024);

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64); // 30 MB
        let region = db.list_regions().unwrap().remove(0);
        db.delete_region(&region);
    }

    // After deleting the offline region, the data will migrate to the
    // ambient cache, respecting the size defined.
    assert!(database_size() <= maximum_size);
    assert!(database_size() >= maximum_size / 2);

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64 * 2); // 60 MB
    }

    // Doubling the size should have no effect if we don't and new tiles and
    // if the ambient cache is already under the maximum size.
    assert!(database_size() <= maximum_size);
    assert!(database_size() >= maximum_size / 2);

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64); // 30 MB

        // Add ~50 MB in ambient cache data.
        for i in 0..250 {
            let ambient_tile = tile(&format!("https://example.com/tiles/tiles/ambient_tile_{i}"), 1.0, 0, 0, 0);
            db.put(&ambient_tile, &response);

            let ambient_style = style(&format!("https://example.com/mas/ambient_style_{i}"));
            db.put(&ambient_style, &response);
        }
    }

    // Only ambient cache now, so it should respect the established size.
    assert!(database_size() <= maximum_size);
    assert!(database_size() >= maximum_size / 2);

    maximum_size = 20 * 1024 * 1024;

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(maximum_size as u64); // 20 MB
    }

    // Should shrink again.
    assert!(database_size() <= maximum_size);
    assert!(database_size() >= initial_size);

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(0);

        for i in 0..5 {
            let ambient_tile = tile(&format!("https://example.com/tiles/tiles/ambient_tile_{i}"), 1.0, 0, 0, 0);
            db.put(&ambient_tile, &response);
            assert!(db.get(&ambient_tile).is_none());

            let ambient_style = style(&format!("https://example.com/maps/ambient_style_{i}"));
            db.put(&ambient_style, &response);
            assert!(db.get(&ambient_style).is_none());
        }
    }

    // Setting the size to zero should effectively clear the cache now.
    assert_eq!(initial_size, file_size(&filename));
}

fn generate_resources(tile_prefix: &str, style_prefix: &str) -> Vec<(Resource, Response)> {
    let response_data = random_string(512 * 1024);
    let mut response = Response::new();
    response.data = Some(response_data);
    let mut resources = Vec::new();
    for i in 0..50 {
        resources.push((tile(&format!("{tile_prefix}{i}"), 1.0, 0, 0, 0), response.clone()));
        resources.push((style(&format!("{style_prefix}{i}")), response.clone()));
    }
    resources
}

// gtest: OfflineDatabase.DeleteRegion
#[test]
fn delete_region() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    {
        let _db_create = OfflineDatabase::new(&filename);
    }

    let initial_size = file_size(&filename);

    {
        let mut response = Response::new();
        response.data = Some(random_string(512 * 1024));
        let _ = response;

        let mut db = OfflineDatabase::new(&filename);

        let definition =
            tile_pyramid("https://example.com/maps/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
        let metadata: OfflineRegionMetadata = vec![1, 2, 3];

        let region1 = db.create_region(&definition.clone().into(), &metadata).unwrap();
        let region2 = db.create_region(&definition.into(), &metadata).unwrap();

        let mut status = OfflineRegionStatus::default();
        db.put_region_resources(
            region1.get_id(),
            &generate_resources("https://example.com/tiles/tiles/tile_1", "https://example.com/maps/style_1"),
            &mut status,
        );
        db.put_region_resources(
            region2.get_id(),
            &generate_resources("https://example.com/tiles/tiles/tile_2", "https://example.com/maps/style_2"),
            &mut status,
        );
        let size_with_two_regions = file_size(&filename);

        db.run_pack_database_automatically(false);
        db.delete_region(&region1);

        assert_eq!(1, db.list_regions().unwrap().len());
        // Region is removed but the size of the database is the same.
        assert_eq!(size_with_two_regions, file_size(&filename));

        db.pack();
        // The size of the database has shrunk after pack().
        let size_with_one_region = file_size(&filename);
        assert!(size_with_one_region < size_with_two_regions);
        db.run_pack_database_automatically(true);
        db.delete_region(&region2);

        // After clearing the cache, the size of the database should get back
        // to the original size.
        db.clear_ambient_cache();

        // The size of the database has shrunk right away after deleted region
        // is evicted from an ambient cache.
        let size_without_regions = file_size(&filename);

        // The tiles from the offline region will migrate to the ambient cache
        // and shrink the database to the maximum size defined by default.
        assert!(size_without_regions as u64 <= DEFAULT_MAX_CACHE_SIZE);

        assert_eq!(0, db.list_regions().unwrap().len());
        assert!(size_without_regions < size_with_one_region);
    }

    assert_eq!(initial_size, file_size(&filename));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.Pack
#[test]
fn pack() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    let mut db = OfflineDatabase::new(&filename);
    let initial_size = file_size(&filename);
    db.run_pack_database_automatically(false);

    let mut response = Response::new();
    response.data = Some(random_string(512 * 1024));

    for i in 0..50 {
        let t = tile(&format!("https://example.com/tiles/tiles/tile_{i}"), 1.0, 0, 0, 0);
        db.put(&t, &response);

        let s = style(&format!("https://example.com/maps/style_{i}"));
        db.put(&s, &response);
    }
    let populated_size = file_size(&filename);
    assert!(populated_size > initial_size);

    db.clear_ambient_cache();
    assert_eq!(populated_size, file_size(&filename));
    check_log_empty(&log);

    db.pack();
    assert_eq!(initial_size, file_size(&filename));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.Invalidate
#[test]
fn invalidate() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let mut response = Response::new();
    response.no_content = true;
    response.must_revalidate = false;
    response.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 3600));

    let ambient_tile = tile("https://example.com/tiles/tiles/tile_ambient", 1.0, 0, 0, 0);
    db.put(&ambient_tile, &response);

    let ambient_style = style("https://example.com/maps/style_ambient");
    db.put(&ambient_style, &response);

    let definition = tile_pyramid("https://example.com/maps/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
    let metadata: OfflineRegionMetadata = vec![1, 2, 3];

    let region1 = db.create_region(&definition.clone().into(), &metadata).unwrap();
    let region1_tile = tile("https://example.com/tiles/tiles/tile_offline_region1", 1.0, 0, 0, 0);
    db.put_region_resource(region1.get_id(), &region1_tile, &response);

    let region1_style = style("https://example.com/maps/style_offline_region1");
    db.put_region_resource(region1.get_id(), &region1_style, &response);

    let region2 = db.create_region(&definition.into(), &metadata).unwrap();
    let region2_tile = tile("https://example.com/tiles/tiles/tile_offline_region2", 1.0, 0, 0, 0);
    db.put_region_resource(region2.get_id(), &region2_tile, &response);

    let region2_style = style("https://example.com/maps/style_offline_region2");
    db.put_region_resource(region2.get_id(), &region2_style, &response);

    let mut get = |r: &Resource| db.get(r).expect("response");

    // Prior to invalidation, all tiles are usable.
    assert!(get(&ambient_tile).is_usable());
    assert!(get(&ambient_style).is_usable());
    assert!(get(&region1_tile).is_usable());
    assert!(get(&region1_style).is_usable());
    assert!(get(&region2_tile).is_usable());
    assert!(get(&region2_style).is_usable());

    // Invalidate a region will not invalidate ambient tiles or other
    // regions.
    assert!(db.invalidate_region(region1.get_id()).is_none());

    let mut get = |r: &Resource| db.get(r).expect("response");
    assert!(get(&ambient_tile).is_usable());
    assert!(get(&ambient_style).is_usable());
    assert!(!get(&region1_tile).is_usable());
    assert!(!get(&region1_style).is_usable());
    assert!(get(&region2_tile).is_usable());
    assert!(get(&region2_style).is_usable());

    // Invalidate the ambient cache will not invalidate the regions that are
    // still valid.
    assert!(db.invalidate_ambient_cache().is_none());

    let mut get = |r: &Resource| db.get(r).expect("response");
    assert!(!get(&ambient_tile).is_usable());
    assert!(!get(&ambient_style).is_usable());
    assert!(!get(&region1_tile).is_usable());
    assert!(!get(&region1_style).is_usable());
    assert!(get(&region2_tile).is_usable());
    assert!(get(&region2_style).is_usable());

    // Sanity check.
    assert!(get(&ambient_tile).expires < Some(now()));
    assert!(get(&ambient_style).expires < Some(now()));
    assert!(get(&region1_tile).expires < Some(now()));
    assert!(get(&region1_style).expires < Some(now()));
    assert!(get(&region2_tile).expires > Some(now()));
    assert!(get(&region2_style).expires > Some(now()));

    assert!(get(&ambient_tile).must_revalidate);
    assert!(get(&ambient_style).must_revalidate);
    assert!(get(&region1_tile).must_revalidate);
    assert!(get(&region1_style).must_revalidate);
    assert!(!get(&region2_tile).must_revalidate);
    assert!(!get(&region2_style).must_revalidate);

    // Should not throw.
    assert!(db.invalidate_region(region2.get_id()).is_none());
    assert!(db.invalidate_region(region2.get_id()).is_none());
    assert!(db.invalidate_region(123).is_none());

    // Invalidate != delete.
    let regions = db.list_regions().unwrap();
    assert_eq!(2, regions.len());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.ClearAmbientCache
#[test]
fn clear_ambient_cache() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    {
        let _db_create = OfflineDatabase::new(&filename);
    }

    let initial_size = file_size(&filename);

    {
        let mut response = Response::new();
        response.data = Some(random_string(512 * 1024));

        let mut db = OfflineDatabase::new(&filename);

        for i in 0..50 {
            let t = tile(&format!("https://example.com/tiles/tiles/tile_{i}"), 1.0, 0, 0, 0);
            db.put(&t, &response);

            let s = style(&format!("https://example.com/maps/style_{i}"));
            db.put(&s, &response);
        }

        db.clear_ambient_cache();
    }

    assert_eq!(initial_size, file_size(&filename));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.CreateRegionInfiniteMaxZoom
#[test]
fn create_region_infinite_max_zoom() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, false);
    let metadata = OfflineRegionMetadata::new();
    let region = db.create_region(&definition.into(), &metadata).expect("region");

    check_log_empty(&log);

    assert_eq!(0.0, region.get_definition().min_zoom());
    assert_eq!(f64::INFINITY, region.get_definition().max_zoom());
}

// gtest: OfflineDatabase.ConcurrentUse
#[test]
fn concurrent_use() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    let mut db1 = OfflineDatabase::new(&filename);
    check_log_empty(&log);

    let mut db2 = OfflineDatabase::new(&filename);

    // C++'s FixtureLog observes every thread; the port's observes its own,
    // so each thread checks its log.
    std::thread::scope(|s| {
        let thread1 = s.spawn(|| {
            let log = FixtureLog::new();
            for _ in 0..100 {
                db1.put(&fixture::resource(), &fixture::response());
                assert!(db1.get(&fixture::resource()).is_some());
            }
            check_log_empty(&log);
        });

        let thread2 = s.spawn(|| {
            let log = FixtureLog::new();
            for i in 0..100 {
                db2.put(&fixture::resource(), &fixture::response());
                assert!(db2.get(&fixture::resource()).is_some());

                if i == 50 {
                    db2.clear_ambient_cache();
                }
            }
            check_log_empty(&log);
        });

        thread1.join().unwrap();
        thread2.join().unwrap();
    });

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutReturnsSize
#[test]
fn put_returns_size() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let mut compressible = Response::new();
    compressible.data = Some(Arc::new(vec![0u8; 1024]));
    // gtest pins zlib's 17 bytes; the exact size depends on the deflate
    // implementation, so this checks that the data is stored compressed.
    let compressible_size = db.put(&style("http://example.com/compressible"), &compressible).1;
    assert!(0 < compressible_size && compressible_size < 1024, "{compressible_size}");
    let stored = db.get(&style("http://example.com/compressible")).expect("response");
    assert_eq!(compressible.data, stored.data);

    let mut incompressible = Response::new();
    incompressible.data = Some(random_string(1024));
    assert_eq!(1024, db.put(&style("http://example.com/incompressible"), &incompressible).1);

    let mut no_content = Response::new();
    no_content.no_content = true;
    assert_eq!(0, db.put(&style("http://example.com/noContent"), &no_content).1);

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutEvictsLeastRecentlyUsedResources
#[test]
fn put_evicts_least_recently_used_resources() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let mut response = Response::new();
    response.data = Some(random_string(1024));

    // Add 101 resource to ambient cache, 1 over defined limit.
    for i in 1..=101u32 {
        let resource = style(&format!("http://example.com/{i}"));
        db.put(&resource, &response);
        assert!(db.get(&resource).is_some(), "{i}");
    }

    assert!(db.get(&style("http://example.com/1")).is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.OfflineRegionDoesNotAffectAmbientCacheSize
#[test]
fn offline_region_does_not_affect_ambient_cache_size() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let data_size: u32 = 1024 * 100;
    let number_of_cached_resources: u32 = 2;
    let database_page: u32 = 4096;
    let page_overhead = number_of_cached_resources * database_page;

    // 200KB ambient cache limit + database page overhead.
    db.set_maximum_ambient_cache_size(u64::from(data_size * number_of_cached_resources + page_overhead));

    let mut response = Response::new();
    response.data = Some(random_string(data_size as usize));

    // First 100KB ambient cache resource.
    db.put(&style("http://example.com/ambient1.json"), &response);

    let definition = tile_pyramid("http://example.com/style.json", LatLngBounds::world(), 0.0, 0.0, 1.0, false);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).unwrap();

    // 1MB of offline region data.
    for i in 0..5 {
        let t = tile(&format!("https://example.com/tiles/tiles/tile_{i}"), 1.0, 0, 0, 0);
        db.put_region_resource(region.get_id(), &t, &response);

        let s = style(&format!("https://example.com/maps/style_{i}"));
        db.put_region_resource(region.get_id(), &s, &response);
    }

    // Second 100KB ambient cache resource.
    db.put(&style("http://example.com/ambient2.json"), &response);

    // Offline region resources should not affect ambient cache size.
    assert!(db.get(&style("http://example.com/ambient1.json")).is_some());
    assert!(db.get(&style("http://example.com/ambient2.json")).is_some());
    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutRegionResourceDoesNotEvict
#[test]
fn put_region_resource_does_not_evict() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, true);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");

    let mut response = Response::new();
    response.data = Some(random_string(1024));

    for i in 1..=100u32 {
        db.put_region_resource(region.get_id(), &style(&format!("http://example.com/{i}")), &response);
    }

    assert!(db.get(&style("http://example.com/1")).is_some());
    assert!(db.get(&style("http://example.com/20")).is_some());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutFailsWhenEvictionInsuffices
#[test]
fn put_fails_when_eviction_insuffices() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let mut big = Response::new();
    // One page over the cache size.
    big.data = Some(random_string(1024 * 100 + 4096));

    assert!(!db.put(&style("http://example.com/big"), &big).0);

    assert_eq!(1, log.count(&message(EventSeverity::Info, -1, "Unable to make space for entry"), false));
    check_log_empty(&log);

    assert!(db.get(&style("http://example.com/big")).is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.PutSkipsZeroSizeAmbientCache
#[test]
fn put_skips_zero_size_ambient_cache() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(0);

    // A region keeps the database enabled, so put() reaches the ambient cache.
    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, true);
    db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");

    let mut response = Response::new();
    response.data = Some(random_string(1024));

    assert!(!db.put(&style("http://example.com/ambient"), &response).0);
    assert!(db.get(&style("http://example.com/ambient")).is_none());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.GetRegionCompletedStatus
#[test]
fn get_region_completed_status() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("http://example.com/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, false);
    let metadata = OfflineRegionMetadata::new();
    let region = db.create_region(&definition.into(), &metadata).expect("region");

    let status1 = db.get_region_completed_status(region.get_id()).expect("status");
    assert_eq!(0, status1.completed_resource_count);
    assert_eq!(0, status1.completed_resource_size);
    assert_eq!(0, status1.completed_tile_count);
    assert_eq!(0, status1.completed_tile_size);

    let mut response = Response::new();
    response.data = Some(data("data"));

    let style_size = db.put_region_resource(region.get_id(), &style("http://example.com/"), &response);

    let status2 = db.get_region_completed_status(region.get_id()).expect("status");
    assert_eq!(1, status2.completed_resource_count);
    assert_eq!(style_size, status2.completed_resource_size);
    assert_eq!(0, status2.completed_tile_count);
    assert_eq!(0, status2.completed_tile_size);

    let tile_size = db.put_region_resource(region.get_id(), &tile("http://example.com/", 1.0, 0, 0, 0), &response);

    let status3 = db.get_region_completed_status(region.get_id()).expect("status");
    assert_eq!(2, status3.completed_resource_count);
    assert_eq!(style_size + tile_size, status3.completed_resource_size);
    assert_eq!(1, status3.completed_tile_count);
    assert_eq!(tile_size, status3.completed_tile_size);

    check_log_empty(&log);
}

// gtest: OfflineDatabase.HasRegionResource
#[test]
fn has_region_resource() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, true);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");

    assert!(db.has_region_resource(&style("http://example.com/1")).is_none());
    assert!(db.has_region_resource(&style("http://example.com/20")).is_none());

    let mut response = Response::new();
    response.data = Some(random_string(1024));

    for i in 1..=100u32 {
        db.put_region_resource(region.get_id(), &style(&format!("http://example.com/{i}")), &response);
    }

    assert!(db.has_region_resource(&style("http://example.com/1")).is_some());
    assert!(db.has_region_resource(&style("http://example.com/20")).is_some());
    assert_eq!(Some(1024), db.has_region_resource(&style("http://example.com/20")));

    check_log_empty(&log);
}

// gtest: OfflineDatabase.HasRegionResourceTile
#[test]
fn has_region_resource_tile() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, false);
    let region = db.create_region(&definition.clone().into(), &OfflineRegionMetadata::new()).expect("region");

    let resource = example_tile();
    let mut response = Response::new();

    response.data = Some(data("first"));

    assert!(db.has_region_resource(&resource).is_none());
    db.put_region_resource(region.get_id(), &resource, &response);
    assert!(db.has_region_resource(&resource).is_some());
    assert_eq!(Some(5), db.has_region_resource(&resource));

    let another_region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");
    assert!(region.get_id() < another_region.get_id());
    assert!(db.has_region_resource(&resource).is_some());
    assert_eq!(Some(5), db.has_region_resource(&resource));

    check_log_empty(&log);
}

// Not a C++ test: ffi patch 0077 tests these through OfflineDownload.
// checkRegionResource reports whether the stored copy is usable, and
// putRegionResource reports the stored size for a 304.
#[test]
fn check_region_resource_and_not_modified_size() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, false);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");

    for resource in [style("http://example.com/style.json"), example_tile()] {
        let mut response = Response::new();
        response.data = Some(data("first"));
        assert_eq!(None, db.check_region_resource(&resource));
        assert_eq!(5, db.put_region_resource(region.get_id(), &resource, &response));
        assert_eq!(Some((5, true)), db.check_region_resource(&resource));

        assert!(db.invalidate_region(region.get_id()).is_none());
        assert_eq!(Some((5, false)), db.check_region_resource(&resource));
        assert_eq!(Some(5), db.has_region_resource(&resource));

        let mut not_modified = Response::new();
        not_modified.not_modified = true;
        not_modified.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 3600));
        assert_eq!(5, db.put_region_resource(region.get_id(), &resource, &not_modified));
        assert_eq!(Some((5, true)), db.check_region_resource(&resource));
    }

    check_log_empty(&log);
}

// gtest: OfflineDatabase.BatchInsertion
#[test]
fn batch_insertion() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");
    db.set_maximum_ambient_cache_size(1024 * 100);

    let definition = tile_pyramid("", LatLngBounds::world(), 0.0, f64::INFINITY, 1.0, true);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).expect("region");

    let mut response = Response::new();
    response.data = Some(random_string(1024));
    let mut resources = Vec::new();

    for i in 1..=100u32 {
        resources.push((style(&format!("http://example.com/{i}")), response.clone()));
    }

    let mut status = OfflineRegionStatus::default();
    db.put_region_resources(region.get_id(), &resources, &mut status);

    for i in 1..=100u32 {
        assert!(db.get(&style(&format!("http://example.com/{i}"))).is_some());
    }

    check_log_empty(&log);
}

/// The migration tests' block: open, `setMaximumAmbientCacheSize(0)`,
/// delete every region.
fn open_and_delete_regions(filename: &str) {
    let mut db = OfflineDatabase::new(filename);
    db.set_maximum_ambient_cache_size(0);

    let regions = db.list_regions().expect("regions");
    for region in regions {
        db.delete_region(&region);
    }
}

// gtest: OfflineDatabase.MigrateFromV2Schema
#[test]
fn migrate_from_v2_schema() {
    // v2.db is a v2 database containing a single offline region with a small
    // number of resources.
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("v2.db"));

    open_and_delete_regions(&filename);

    assert_eq!(6, database_user_version(&filename));
    assert!(database_page_count(&filename) < database_page_count(fixture("v2.db").to_str().unwrap()));

    check_log_empty(&log);
}

// gtest: OfflineDatabase.MigrateFromV3Schema
#[test]
fn migrate_from_v3_schema() {
    // v3.db is a v3 database, migrated from v2.
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("v3.db"));

    open_and_delete_regions(&filename);

    assert_eq!(6, database_user_version(&filename));

    check_log_empty(&log);
}

// gtest: OfflineDatabase.MigrateFromV4Schema
#[test]
fn migrate_from_v4_schema() {
    // v4.db is a v4 database, migrated from v2 & v3. This database used
    // `journal_mode = WAL` and `synchronous = NORMAL`.
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("v4.db"));

    open_and_delete_regions(&filename);

    assert_eq!(6, database_user_version(&filename));

    // Journal mode should be DELETE after migration to v5.
    assert_eq!("delete", database_journal_mode(&filename));

    // Synchronous setting should be FULL (2) after migration to v5.
    assert_eq!(2, database_sync_mode(&filename));

    check_log_empty(&log);
}

const TILES_COLUMNS: [&str; 13] = [
    "id",
    "url_template",
    "pixel_ratio",
    "z",
    "x",
    "y",
    "expires",
    "modified",
    "etag",
    "data",
    "compressed",
    "accessed",
    "must_revalidate",
];
const RESOURCES_COLUMNS: [&str; 10] =
    ["id", "url", "kind", "expires", "modified", "etag", "data", "compressed", "accessed", "must_revalidate"];

// gtest: OfflineDatabase.MigrateFromV5Schema
#[test]
fn migrate_from_v5_schema() {
    // v5.db is a v5 database, migrated from v2, v3 & v4.
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("v5.db"));

    open_and_delete_regions(&filename);

    assert_eq!(6, database_user_version(&filename));

    assert_eq!(TILES_COLUMNS.to_vec(), database_table_columns(&filename, "tiles"));
    assert_eq!(RESOURCES_COLUMNS.to_vec(), database_table_columns(&filename, "resources"));

    check_log_empty(&log);
}

// gtest: OfflineDatabase.IncrementalVacuum
#[test]
fn incremental_vacuum() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("no_auto_vacuum.db"));
    assert_eq!(0, database_auto_vacuum(&filename));

    open_and_delete_regions(&filename);

    assert_eq!(2, database_auto_vacuum(&filename));

    check_log_empty(&log);
}

// gtest: OfflineDatabase.DowngradeSchema
#[test]
fn downgrade_schema() {
    // v999.db is a v999 database, it should be deleted and recreated with the
    // current schema.
    let log = FixtureLog::new();
    let filename = filename();
    delete_file(&filename);
    copy_file(&filename, &fixture("v999.db"));

    {
        let mut db = OfflineDatabase::new(&filename);
        db.set_maximum_ambient_cache_size(0);
    }

    assert_eq!(6, database_user_version(&filename));

    assert_eq!(TILES_COLUMNS.to_vec(), database_table_columns(&filename, "tiles"));
    assert_eq!(RESOURCES_COLUMNS.to_vec(), database_table_columns(&filename, "resources"));

    assert_eq!(1, log.count(&removing(), false));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.CorruptDatabaseOnOpen
#[test]
fn corrupt_database_on_open() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_file(&filename);
    copy_file(&filename, &fixture("corrupt-immediate.db"));

    // This database is corrupt in a way that will prevent opening the
    // database.
    let mut db = OfflineDatabase::new(&filename);
    assert_eq!(
        1,
        log.count(&error(result_code::CORRUPT, "Can't open database: database disk image is malformed"), true)
    );
    assert_eq!(1, log.count(&removing(), false));
    check_log_empty(&log);

    // Now try inserting and reading back to make sure we have a valid
    // database.
    for res in [fixture::resource(), fixture::tile()] {
        assert_eq!((true, 5), db.put(&res, &fixture::response()));
        check_log_empty(&log);
        let result = db.get(&res);
        check_log_empty(&log);
        let result = result.expect("result");
        assert_eq!(b"first", result.data.expect("data").as_slice());
    }
}

// gtest: OfflineDatabase.CorruptDatabaseOnQuery
#[test]
fn corrupt_database_on_query() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_file(&filename);
    copy_file(&filename, &fixture("corrupt-delayed.db"));

    // This database is corrupt in a way that won't manifest itself until we
    // start querying it, so just opening it will not cause an error.
    let mut db = OfflineDatabase::new(&filename);

    // Just opening this corrupt database should not have produced an error
    // yet, since PRAGMA user_version still succeeds with this database.
    check_log_empty(&log);

    // The first request fails because the database is corrupt and has to be
    // recreated.
    assert!(db.get(&fixture::tile()).is_none());
    assert_eq!(
        1,
        log.count(&error(result_code::CORRUPT, "Can't read resource: database disk image is malformed"), true)
    );
    assert_eq!(1, log.count(&removing(), false));
    check_log_empty(&log);

    // Now try inserting and reading back to make sure we have a valid
    // database.
    for res in [fixture::resource(), fixture::tile()] {
        assert_eq!((true, 5), db.put(&res, &fixture::response()));
        check_log_empty(&log);
        let result = db.get(&res);
        check_log_empty(&log);
        let result = result.expect("result");
        assert_eq!(b"first", result.data.expect("data").as_slice());
    }
}

// gtest: OfflineDatabase.MergeDatabaseWithSingleRegion_New
#[test]
fn merge_database_with_single_region_new() {
    let filename_sideload = filename_sideload();
    delete_file(&filename_sideload);
    copy_file(&filename_sideload, &fixture("sideload_sat.db"));

    let mut db = OfflineDatabase::new(":memory:");
    assert_eq!(0, db.list_regions().unwrap().len());

    let mut db_sideload = OfflineDatabase::new(&filename_sideload);
    let new_regions_count = db_sideload.list_regions().unwrap().len();

    let result = db.merge_database(&filename_sideload);
    assert_eq!(new_regions_count, db.list_regions().unwrap().len());

    let region_id = result.unwrap()[0].get_id();
    let status = db.get_region_completed_status(region_id).unwrap();
    assert_eq!(398, status.completed_resource_count);
    assert_eq!(5, status.completed_tile_count);
}

const SATELLITE_TILE_URL: &str = "maptiler://tiles/tiles/satellite/{z}/{x}/{y}{ratio}.jpg";

// gtest: OfflineDatabase.MergeDatabaseWithSingleRegion_Update
#[test]
fn merge_database_with_single_region_update() {
    let filename = filename();
    let filename_sideload = filename_sideload();
    delete_database_files(&filename);
    delete_file(&filename_sideload);
    copy_file(&filename, &fixture("satellite_test.db"));
    copy_file(&filename_sideload, &fixture("sideload_sat.db"));

    {
        let mut db1 = OfflineDatabase::new(&filename_sideload);
        let original_tile = db1.get_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 0, 1));
        let original_stamp = original_tile.expect("tile").0.modified;

        let mut db = OfflineDatabase::new(&filename);
        let regions = db.list_regions();
        assert_eq!(1, db.list_regions().unwrap().len());
        let region_id = regions.unwrap()[0].get_id();

        let result = db.merge_database(&filename_sideload);
        assert_eq!(1, result.unwrap().len());

        let status = db.get_region_completed_status(region_id).unwrap();
        assert_eq!(395, status.completed_resource_count);
        assert_eq!(2, status.completed_tile_count);

        // Verify the modified timestamp matches the tile in the sideloaded db.
        let updated_tile = db.get_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 0, 1));

        let updated_stamp = updated_tile.expect("tile").0.modified;
        assert_eq!(original_stamp.unwrap(), updated_stamp.unwrap());
    }
}

// gtest: OfflineDatabase.MergeDatabaseWithSingleRegion_NoUpdate
#[test]
fn merge_database_with_single_region_no_update() {
    let filename = filename();
    let filename_sideload = filename_sideload();
    delete_database_files(&filename);
    delete_file(&filename_sideload);

    // Swap sideload/main database from update test and ensure that an older
    // tile is not copied over
    copy_file(&filename_sideload, &fixture("satellite_test.db"));
    copy_file(&filename, &fixture("sideload_sat.db"));

    let mut db = OfflineDatabase::new(&filename);

    let original_tile = db.get_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 0, 1));
    let original_stamp = original_tile.expect("tile").0.modified;

    let result = db.merge_database(&filename_sideload);
    assert_eq!(1, result.unwrap().len());
    assert_eq!(2, db.list_regions().unwrap().len());

    let updated_tile = db.get_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 0, 1));

    // Verify the modified timestamp matches the tile in the main db.
    assert_eq!(original_stamp, updated_tile.expect("tile").0.modified);
}

// gtest: OfflineDatabase.MergeDatabaseWithSingleRegion_AmbientTiles
#[test]
fn merge_database_with_single_region_ambient_tiles() {
    let filename_sideload = filename_sideload();
    delete_file(&filename_sideload);
    copy_file(&filename_sideload, &fixture("sideload_ambient.db"));

    let mut db = OfflineDatabase::new(":memory:");
    let _result = db.merge_database(&filename_sideload);

    assert!(db.has_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 0, 1)).is_some());

    // Ambient resources should not be copied
    assert!(db.has_region_resource(&style("maptiler://maps/streets")).is_none());
    assert!(db.has_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 0, 1, 2)).is_none());
    assert!(db.has_region_resource(&tile(SATELLITE_TILE_URL, 1.0, 1, 1, 2)).is_none());
}

// gtest: OfflineDatabase.MergeDatabaseWithMultipleRegions_New
#[test]
fn merge_database_with_multiple_regions_new() {
    let filename_sideload = filename_sideload();
    delete_file(&filename_sideload);
    copy_file(&filename_sideload, &fixture("sideload_sat_multiple.db"));

    let mut db = OfflineDatabase::new(":memory:");
    assert_eq!(0, db.list_regions().unwrap().len());

    let result = db.merge_database(&filename_sideload).unwrap();
    assert_eq!(2, result.len());
    assert_eq!(2, db.list_regions().unwrap().len());

    let region1_id = result.first().unwrap().get_id();
    let status = db.get_region_completed_status(region1_id).unwrap();
    assert_eq!(398, status.completed_resource_count);
    assert_eq!(5, status.completed_tile_count);

    let region2_id = result.last().unwrap().get_id();
    let status = db.get_region_completed_status(region2_id).unwrap();
    assert_eq!(0, status.completed_tile_count);
    assert_eq!(200, status.completed_resource_count);
}

// gtest: OfflineDatabase.MergeDatabaseWithMultipleRegionsWithOverlap
#[test]
fn merge_database_with_multiple_regions_with_overlap() {
    let filename = filename();
    let filename_sideload = filename_sideload();
    delete_database_files(&filename);
    delete_file(&filename_sideload);
    copy_file(&filename, &fixture("sideload_sat.db"));
    copy_file(&filename_sideload, &fixture("sideload_sat_multiple.db"));

    {
        let mut db = OfflineDatabase::new(&filename);
        assert_eq!(1, db.list_regions().unwrap().len());

        let result = db.merge_database(&filename_sideload);
        assert_eq!(2, result.unwrap().len());
        assert_eq!(2, db.list_regions().unwrap().len());
    }

    let db = Database::open(&filename, OpenFlag::ReadOnly).unwrap();
    let conn = db.conn();
    {
        let mut stmt = Statement::new(
            &conn,
            "SELECT COUNT(*) \
             FROM region_tiles \
             JOIN tiles WHERE tile_id = id",
        )
        .unwrap();
        let mut query = Query::new(&mut stmt);
        query.run().unwrap();

        // Ensure multiple entries for tiles shared between regions
        assert_eq!(5, query.get_i32(0).unwrap());
    }
    {
        let mut stmt = Statement::new(
            &conn,
            "SELECT COUNT(*) \
             FROM region_resources \
             JOIN resources WHERE resource_id = id",
        )
        .unwrap();
        let mut query = Query::new(&mut stmt);
        query.run().unwrap();

        // Ensure multiple entries for resources shared between regions
        assert_eq!(593, query.get_i32(0).unwrap());
    }
}

// C++ skips this one on Windows (`#ifndef WIN32`), which can't open a folder
// to copy it as a file.
// gtest: OfflineDatabase.MergeDatabaseWithInvalidPath
#[cfg(not(windows))]
#[test]
fn merge_database_with_invalid_path() {
    let log = FixtureLog::new();

    let filename_sideload = filename_sideload();
    delete_file(&filename_sideload);
    copy_file(&filename_sideload, &cpp_path("test/fixtures/offline_database"));

    let mut db = OfflineDatabase::new(":memory:");

    let result = db.merge_database(&filename_sideload);
    assert!(result.is_err());

    assert_eq!(1, log.count(&message(EventSeverity::Error, -1, "Merge database has incorrect user_version"), false));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.MergeDatabaseWithInvalidDb
#[test]
fn merge_database_with_invalid_db() {
    let log = FixtureLog::new();

    let filename_sideload = filename_sideload();
    delete_file(&filename_sideload);
    copy_file(&filename_sideload, &fixture("corrupt-immediate.db"));

    let mut db = OfflineDatabase::new(":memory:");

    let result = db.merge_database(&filename_sideload);
    assert!(result.is_err());

    assert_eq!(
        1,
        log.count(
            &error(
                result_code::CORRUPT,
                &format!("Can't attach database ({filename_sideload}) for merge: database disk image is malformed")
            ),
            false
        )
    );
    check_log_empty(&log);
}

// gtest: OfflineDatabase.ChangePath
#[test]
fn change_path() {
    let new_path = temp_path("test.db");
    let mut db = OfflineDatabase::new(":memory:");
    db.change_path(&new_path);
    Database::open(&new_path, OpenFlag::ReadOnly).unwrap();
    delete_file(&new_path);
}

// gtest: OfflineDatabase.ResetDatabase
#[test]
fn reset_database() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    copy_file(&filename, &fixture("satellite_test.db"));

    let mut db = OfflineDatabase::new(&filename);
    let result = db.reset_database();
    assert!(result.is_none());

    let regions = db.list_regions().unwrap();
    assert_eq!(0, regions.len());
    assert_eq!(1, log.count(&removing(), false));
    check_log_empty(&log);
}

// gtest: OfflineDatabase.ResetInMemoryDatabase
#[test]
fn reset_in_memory_database() {
    for path in [":memory:", "file::memory:", "file::memory:?cache=shared", "file:reset-test?mode=memory"] {
        let log = FixtureLog::new();
        // C++ writes a decoy file at the relative path, which the reset must
        // not delete. Skipped where the working directory is read-only.
        let decoy = std::fs::write(path, "unrelated file").is_ok();

        let mut db = OfflineDatabase::new(path);
        assert!(db.reset_database().is_none(), "{path}");
        if decoy {
            assert_eq!("unrelated file", std::fs::read_to_string(path).unwrap(), "{path}");
            delete_file(path);
        }
        assert_eq!(1, log.count(&removing(), false), "{path}");
        assert_eq!(0, log.unchecked_count(), "{path}: {:?}", log.unchecked());
    }
}

// gtest: OfflineDatabase.PutResourceReadOnlyMode
#[test]
fn put_resource_read_only_mode() {
    let log = FixtureLog::new();
    let mut db = OfflineDatabase::new(":memory:");

    let resource = Resource::new(Kind::Style, "http://example.com/");
    let mut response = Response::new();
    response.data = Some(data("success"));

    // In read-only mode put() is a no-op
    db.reopen_database_read_only(true);
    let failed_put_result = db.put(&resource, &response);
    assert!(!failed_put_result.0);
    assert_eq!(0, failed_put_result.1);

    // put() works, if read-only mode is disabled
    db.reopen_database_read_only(false);
    let succeeded_put_result = db.put(&resource, &response);
    assert!(succeeded_put_result.0);
    assert_eq!(7, succeeded_put_result.1);

    let get_result = db.get(&resource).expect("response");
    assert!(get_result.error.is_none());
    assert_eq!(b"success", get_result.data.unwrap().as_slice());

    check_log_empty(&log);
}

// gtest: OfflineDatabase.UpdateDatabaseReadOnlyMode
#[test]
fn update_database_read_only_mode() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);

    let mut db = OfflineDatabase::new(&filename);
    db.reopen_database_read_only(true);
    db.clear_ambient_cache();
    assert_eq!(
        1,
        log.count(
            &message(EventSeverity::Error, -1, "Can't clear ambient cache: Cannot modify database in read-only mode"),
            false
        )
    );

    check_log_empty(&log);
}

// The geometry region definition round trip through the database (not a C++
// test; covers OfflineGeometryRegionDefinition storage).
#[test]
fn geometry_region_round_trip() {
    let mut db = OfflineDatabase::new(":memory:");
    let definition = OfflineGeometryRegionDefinition::new(
        "https://example.com/style",
        Geometry::Point(Point::new(-122.5744, 37.6609)),
        0.0,
        2.0,
        1.0,
        false,
    )
    .unwrap();
    let region = db.create_region(&definition.clone().into(), &OfflineRegionMetadata::new()).unwrap();
    let regions = db.list_regions().unwrap();
    assert_eq!(1, regions.len());
    assert_eq!(region.get_id(), regions[0].get_id());
    assert_eq!(&OfflineRegionDefinition::Geometry(definition), regions[0].get_definition());
}

fn cached_accessed(filename: &str, table: &str) -> i64 {
    let db = Database::open(filename, OpenFlag::ReadOnly).unwrap();
    let conn = db.conn();
    let sql = format!("SELECT accessed FROM {table}");
    let mut stmt = Statement::new(&conn, &sql).unwrap();
    let mut query = Query::new(&mut stmt);
    assert!(query.run().unwrap());
    query.get_i64(0).unwrap()
}

fn cache_sql(filename: &str, sql: &str) {
    Database::open(filename, OpenFlag::ReadWriteCreate).unwrap().exec(sql).unwrap();
}

fn constraint_warning(msg: &str) -> Message {
    warning(i64::from(result_code::CONSTRAINT), msg)
}

// gtest: OfflineDatabase.BatchedReadTouchesDeduplicateAndSkipMisses
#[test]
fn batched_read_touches_deduplicate_and_skip_misses() {
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    db.put(&fixture::resource(), &fixture::response());
    db.put(&fixture::tile(), &fixture::response());
    cache_sql(&filename, "UPDATE resources SET accessed = 100; UPDATE tiles SET accessed = 100");
    let callbacks = Arc::new(AtomicUsize::new(0));
    let counter = callbacks.clone();
    db.set_accessed_callback(Box::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    assert!(db.get(&fixture::resource()).is_some());
    assert!(db.get(&fixture::resource()).is_some());
    assert!(db.get(&fixture::tile()).is_some());
    assert!(db.get(&fixture::tile()).is_some());
    assert!(db.get_region_resource(&fixture::resource()).is_some());
    assert!(db.get(&style("not-cached")).is_none());
    assert_eq!(5, callbacks.load(Ordering::Relaxed));
    assert_eq!(2, db.pending_accessed_count());
    assert_eq!(100, cached_accessed(&filename, "resources"));
    assert_eq!(100, cached_accessed(&filename, "tiles"));
    assert!(db.flush_accessed());
    assert_eq!(0, db.pending_accessed_count());
    assert!(cached_accessed(&filename, "resources") > 100);
    assert!(cached_accessed(&filename, "tiles") > 100);
}

// gtest: OfflineDatabase.BatchedReadTouchesRollbackAndRetry
#[test]
fn batched_read_touches_rollback_and_retry() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    db.put(&fixture::resource(), &fixture::response());
    db.put(&fixture::tile(), &fixture::response());
    cache_sql(
        &filename,
        "UPDATE resources SET accessed = 100; UPDATE tiles SET accessed = 100; \
         CREATE TRIGGER fail_touch BEFORE UPDATE OF accessed ON tiles \
         BEGIN SELECT RAISE(ABORT, 'injected flush failure'); END",
    );
    assert!(db.get(&fixture::resource()).is_some());
    assert!(db.get(&fixture::tile()).is_some());
    assert!(!db.flush_accessed());
    assert_eq!(1, log.count(&constraint_warning("Can't flush accessed timestamps: injected flush failure"), false));
    assert_eq!(2, db.pending_accessed_count());
    // The resource update ran first, but the whole transaction rolled back.
    assert_eq!(100, cached_accessed(&filename, "resources"));
    assert_eq!(100, cached_accessed(&filename, "tiles"));
    cache_sql(&filename, "DROP TRIGGER fail_touch");
    assert!(db.flush_accessed());
    assert_eq!(0, db.pending_accessed_count());
    assert!(cached_accessed(&filename, "resources") > 100);
    assert!(cached_accessed(&filename, "tiles") > 100);
    check_log_empty(&log);
}

// gtest: OfflineDatabase.BatchedReadTouchesPreserveNewerTimestampAndFlushReadOnly
#[test]
fn batched_read_touches_preserve_newer_timestamp_and_flush_read_only() {
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    db.put(&fixture::resource(), &fixture::response());
    assert!(db.get(&fixture::resource()).is_some());
    cache_sql(&filename, "UPDATE resources SET accessed = 4102444800");
    assert!(db.flush_accessed());
    assert_eq!(4_102_444_800, cached_accessed(&filename, "resources"));
    cache_sql(&filename, "UPDATE resources SET accessed = 100");
    assert!(db.get(&fixture::resource()).is_some());
    db.reopen_database_read_only(true);
    assert_eq!(0, db.pending_accessed_count());
    assert!(cached_accessed(&filename, "resources") > 100);
    assert!(db.get(&fixture::resource()).is_some());
    assert_eq!(0, db.pending_accessed_count());
}

// gtest: OfflineDatabase.BatchedReadTouchesDiscardAfterClear
#[test]
fn batched_read_touches_discard_after_clear() {
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    db.put(&fixture::resource(), &fixture::response());
    assert!(db.get(&fixture::resource()).is_some());
    assert_eq!(1, db.pending_accessed_count());
    assert!(db.clear_ambient_cache().is_none());
    assert_eq!(0, db.pending_accessed_count());
    assert!(db.get(&fixture::resource()).is_none());
    assert!(db.flush_accessed());
}

// gtest: OfflineDatabase.BatchedReadTouchesFlushOnPathChangeAndClose
#[test]
fn batched_read_touches_flush_on_path_change_and_close() {
    let filename = filename();
    delete_database_files(&filename);
    {
        let mut db = OfflineDatabase::new(&filename);
        db.put(&fixture::resource(), &fixture::response());
        cache_sql(&filename, "UPDATE resources SET accessed = 100");
        assert!(db.get(&fixture::resource()).is_some());
        db.change_path(":memory:");
        assert_eq!(0, db.pending_accessed_count());
        assert!(cached_accessed(&filename, "resources") > 100);
        assert!(db.get(&fixture::resource()).is_none());
    }
    cache_sql(&filename, "UPDATE resources SET accessed = 100");
    {
        let mut db = OfflineDatabase::new(&filename);
        assert!(db.get(&fixture::resource()).is_some());
    }
    assert!(cached_accessed(&filename, "resources") > 100);
}

// gtest: OfflineDatabase.BatchedReadTouchesBoundPersistentFailures
#[test]
fn batched_read_touches_bound_persistent_failures() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    cache_sql(
        &filename,
        "WITH RECURSIVE sequence(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM sequence WHERE n<4100) \
         INSERT INTO resources(url,kind,accessed) SELECT 'key-'||n,2,100 FROM sequence; \
         CREATE TRIGGER fail_touch BEFORE UPDATE OF accessed ON resources \
         BEGIN SELECT RAISE(ABORT,'injected persistent failure'); END",
    );
    for i in 1..=4100 {
        assert!(db.get(&Resource::new(Kind::Source, format!("key-{i}"))).is_some());
    }
    assert_eq!(4096, db.pending_accessed_count());
    assert_eq!(
        1,
        log.count(&constraint_warning("Can't flush accessed timestamps: injected persistent failure"), false)
    );
    cache_sql(&filename, "DROP TRIGGER fail_touch");
    assert!(db.flush_accessed());
    assert_eq!(0, db.pending_accessed_count());
    check_log_empty(&log);
}

// gtest: OfflineDatabase.BatchedReadTouchesParticipateInEvictionTransaction
#[test]
fn batched_read_touches_participate_in_eviction_transaction() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    cache_sql(
        &filename,
        "WITH RECURSIVE sequence(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM sequence WHERE n<60) \
         INSERT INTO resources(url,kind,accessed,data) SELECT 'evict-'||n,2,n,randomblob(1024) FROM sequence",
    );
    assert!(db.set_maximum_ambient_cache_size(100_000).is_none());
    let recent = Resource::new(Kind::Source, "evict-1");
    assert!(db.get(&recent).is_some());
    assert_eq!(1, db.pending_accessed_count());
    let mut response = Response::new();
    response.data = Some(random_string(50000));
    let incoming = Resource::new(Kind::Source, "evict-incoming");
    cache_sql(
        &filename,
        "CREATE TRIGGER fail_insert BEFORE INSERT ON resources WHEN NEW.url='evict-incoming' \
         BEGIN SELECT RAISE(ABORT,'injected insertion failure'); END",
    );
    assert!(!db.put(&incoming, &response).0);
    assert_eq!(1, log.count(&constraint_warning("Can't write resource: injected insertion failure"), false));
    assert_eq!(1, db.pending_accessed_count());
    assert_eq!(1, cached_accessed(&filename, "resources"));
    cache_sql(&filename, "DROP TRIGGER fail_insert");
    assert!(db.put(&incoming, &response).0);
    assert_eq!(0, db.pending_accessed_count());
    assert!(db.get(&recent).is_some());
    assert!(db.get(&Resource::new(Kind::Source, "evict-2")).is_none());
    let cached = db.get(&incoming).expect("cached");
    assert_eq!(response.data, cached.data);
    check_log_empty(&log);
}

// gtest: OfflineDatabase.BatchedReadTouchFailureDoesNotPreventCacheResize
#[test]
fn batched_read_touch_failure_does_not_prevent_cache_resize() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    db.put(&fixture::resource(), &fixture::response());
    assert!(db.get(&fixture::resource()).is_some());
    cache_sql(
        &filename,
        "CREATE TRIGGER fail_touch BEFORE UPDATE OF accessed ON resources \
         BEGIN SELECT RAISE(ABORT,'injected resize touch failure'); END",
    );
    assert!(db.set_maximum_ambient_cache_size(0).is_none());
    assert_eq!(
        1,
        log.count(&constraint_warning("Can't flush accessed timestamps: injected resize touch failure"), false)
    );
    assert!(db.get(&fixture::resource()).is_none());
    assert_eq!(1, db.pending_accessed_count());
    cache_sql(&filename, "DROP TRIGGER fail_touch");
    assert!(db.flush_accessed());
    assert_eq!(0, db.pending_accessed_count());
    check_log_empty(&log);
}

// gtest: OfflineDatabase.BatchedReadTouchFailureDoesNotPreventRegionDeletion
#[test]
fn batched_read_touch_failure_does_not_prevent_region_deletion() {
    let log = FixtureLog::new();
    let filename = filename();
    delete_database_files(&filename);
    let mut db = OfflineDatabase::new(&filename);
    assert!(db.set_maximum_ambient_cache_size(0).is_none());
    let definition = tile_pyramid("https://example.com/maps/style", hull((1.0, 2.0), (3.0, 4.0)), 5.0, 6.0, 2.0, true);
    let region = db.create_region(&definition.into(), &OfflineRegionMetadata::new()).unwrap();
    db.put_region_resource(region.get_id(), &fixture::tile(), &fixture::response());
    assert!(db.get_region_resource(&fixture::tile()).is_some());
    cache_sql(
        &filename,
        "CREATE TRIGGER fail_touch BEFORE UPDATE OF accessed ON tiles \
         BEGIN SELECT RAISE(ABORT,'injected delete touch failure'); END",
    );
    assert!(db.delete_region(&region).is_none());
    assert_eq!(
        1,
        log.count(&constraint_warning("Can't flush accessed timestamps: injected delete touch failure"), false)
    );
    assert_eq!(db.list_regions().unwrap(), Vec::new());
    assert!(db.get(&fixture::tile()).is_none());
    assert_eq!(1, db.pending_accessed_count());
    cache_sql(&filename, "DROP TRIGGER fail_touch");
    assert!(db.flush_accessed());
    assert_eq!(0, db.pending_accessed_count());
    check_log_empty(&log);
}
