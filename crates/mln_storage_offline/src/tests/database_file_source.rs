// The offline region API of DatabaseFileSource. C++ has no gtests for it;
// these follow the region lifecycle that the C ABI suite drives through it
// (tools/c-abi-test/testdata/tests/native/abi/resources/offline.c).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionObserver, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use mln_storage::offline_database::{DatabaseError, OfflineDatabase};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::chrono::{Timestamp, now};
use mln_util::geo::{LatLng, LatLngBounds};

use super::TempDir;
use crate::database_file_source::{DatabaseFileSourceExt, RegionDatabase, region_database};

/// How long a test waits for a callback or a status before it fails as
/// hung.
const TIMEOUT: Duration = Duration::from_secs(10);

const STYLE_URL: &str = "http://example.invalid/style.json";

/// Calls `operation` with a callback and waits for its result.
fn wait<T: Send + 'static>(operation: impl FnOnce(Box<dyn FnOnce(T) + Send>)) -> T {
    let (tx, rx) = channel();
    operation(Box::new(move |result| tx.send(result).unwrap()));
    rx.recv_timeout(TIMEOUT).expect("callback")
}

/// A database file source with its database in `dir`, whose regions
/// download through `network`.
fn database_file_source(dir: &TempDir, network: &Arc<StubFileSource>) -> RegionDatabase {
    region_database(&dir.path("cache.db"), Some(Arc::<StubFileSource>::clone(network)), super::registry())
}

fn definition(style_url: &str) -> OfflineRegionDefinition {
    let bounds = LatLngBounds::hull(LatLng::new(1.0, 2.0).unwrap(), LatLng::new(3.0, 4.0).unwrap());
    OfflineTilePyramidRegionDefinition::new(style_url, bounds, 0.0, 0.0, 1.0, false).unwrap().into()
}

/// A style that has no subresources, fresh for an hour.
fn style() -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(br#"{"version":8,"sources":{},"layers":[]}"#.to_vec()));
    response.caching.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 3600));
    response
}

/// A network of `type_` that answers the style with [`style`], and counts
/// the requests it answers.
fn style_network(type_: ResponseType) -> (Arc<StubFileSource>, Arc<AtomicUsize>) {
    let network = Arc::new(StubFileSource::new(type_));
    let answered = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&answered);
    network.set_response(Kind::Style, move |resource: &Resource| {
        assert_eq!(STYLE_URL, resource.url);
        count.fetch_add(1, Ordering::SeqCst);
        Some(style())
    });
    (network, answered)
}

/// Sends every status the region reports.
struct StatusObserver(Sender<OfflineRegionStatus>);

impl OfflineRegionObserver for StatusObserver {
    fn status_changed(&self, status: OfflineRegionStatus) {
        let _ = self.0.send(status);
    }
}

fn observe(source: &RegionDatabase, region: &OfflineRegion) -> Receiver<OfflineRegionStatus> {
    let (tx, rx) = channel();
    wait(|done| source.set_offline_region_observer(region.id, Some(Box::new(StatusObserver(tx))), done)).unwrap();
    rx
}

/// Waits for the download to finish.
fn wait_until_complete(statuses: &Receiver<OfflineRegionStatus>) -> OfflineRegionStatus {
    loop {
        let status = statuses.recv_timeout(TIMEOUT).expect("status");
        if status.complete() && status.download_state == OfflineRegionDownloadState::Inactive {
            return status;
        }
    }
}

#[test]
fn a_region_lives_from_creation_to_deletion() {
    let dir = TempDir::new();
    let (network, _) = style_network(ResponseType::Synchronous);
    let source = database_file_source(&dir, &network);

    let region = wait(|done| source.create_offline_region(&definition(STYLE_URL), &b"one".to_vec(), done)).unwrap();
    assert_eq!(b"one", region.metadata.as_slice());
    assert_eq!(vec![region.clone()], wait(|done| source.list_offline_regions(done)).unwrap());
    assert_eq!(Some(region.clone()), wait(|done| source.get_offline_region(region.id, done)).unwrap());
    assert_eq!(None, wait(|done| source.get_offline_region(region.id + 1, done)).unwrap());

    let updated = wait(|done| source.update_offline_metadata(region.id, &b"two".to_vec(), done)).unwrap();
    assert_eq!(b"two", updated.metadata.as_slice());
    assert_eq!(Some(updated), wait(|done| source.get_offline_region(region.id, done)).unwrap());

    let status = wait(|done| source.get_offline_region_status(region.id, done)).unwrap();
    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(0, status.completed_resource_count);

    let statuses = observe(&source, &region);
    wait(|done| source.set_offline_region_download_state(region.id, OfflineRegionDownloadState::Active, done)).unwrap();
    let status = wait_until_complete(&statuses);
    assert_eq!(1, status.completed_resource_count);
    let status = wait(|done| source.get_offline_region_status(region.id, done)).unwrap();
    assert_eq!(1, status.completed_resource_count);
    assert!(status.complete());

    wait(|done| source.invalidate_offline_region(region.id, done)).unwrap();
    wait(|done| source.delete_offline_region(region.id, done)).unwrap();
    assert_eq!(wait(|done| source.list_offline_regions(done)).unwrap(), Vec::<OfflineRegion>::new());
    let id = region.id;
    let not_found =
        |result: Result<(), DatabaseError>| matches!(result, Err(DatabaseError::RegionNotFound(i)) if i == id);
    assert!(not_found(wait(|done| source.get_offline_region_status(id, done)).map(|_| ())));
    assert!(not_found(wait(|done| source.set_offline_region_download_state(
        id,
        OfflineRegionDownloadState::Active,
        done
    ))));
    assert!(not_found(wait(|done| source.set_offline_region_observer(id, None, done))));
    assert!(not_found(wait(|done| source.invalidate_offline_region(id, done))));
    assert!(not_found(wait(|done| source.delete_offline_region(id, done))));
    assert!(not_found(wait(|done| source.update_offline_metadata(id, &Vec::new(), done)).map(|_| ())));
}

#[test]
fn a_deleted_region_stops_downloading() {
    let dir = TempDir::new();
    let (network, answered) = style_network(ResponseType::Manual);
    let source = database_file_source(&dir, &network);

    let region = wait(|done| source.create_offline_region(&definition(STYLE_URL), &Vec::new(), done)).unwrap();
    let statuses = observe(&source, &region);
    wait(|done| source.set_offline_region_download_state(region.id, OfflineRegionDownloadState::Active, done)).unwrap();
    assert_eq!(OfflineRegionDownloadState::Active, statuses.recv_timeout(TIMEOUT).unwrap().download_state);
    // The download has requested the style by the time the database answers
    // a call made after the activation.
    wait(|done| source.get_offline_region_status(region.id, done)).unwrap();
    wait(|done| source.delete_offline_region(region.id, done)).unwrap();

    // The style answers after the deletion, unless the deletion cancelled
    // its request; the download it belonged to is gone, so the observer
    // hears nothing more once the database has run what the answer sent.
    network.respond_to_all();
    wait(|done| source.list_offline_regions(done)).unwrap();
    assert!(statuses.try_recv().is_err());
    assert!(answered.load(Ordering::SeqCst) <= 1);
}

// The C ABI suite's a_download_refreshes_an_invalidated_region:
// a download after invalidating the region revalidates the style with the
// network, and a 304 keeps it.
#[test]
fn a_download_refreshes_an_invalidated_region() {
    let dir = TempDir::new();
    let network = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let (fetched, revalidated) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (fetches, revalidations) = (Arc::clone(&fetched), Arc::clone(&revalidated));
    network.set_response(Kind::Style, move |resource: &Resource| {
        if resource.prior_etag.as_deref() == Some("\"style-1\"") {
            revalidations.fetch_add(1, Ordering::SeqCst);
            let mut not_modified = Response::new();
            not_modified.not_modified = true;
            not_modified.caching.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 3600));
            return Some(not_modified);
        }
        fetches.fetch_add(1, Ordering::SeqCst);
        let mut response = style();
        response.etag = Some("\"style-1\"".to_owned());
        Some(response)
    });
    let source = database_file_source(&dir, &network);
    let calls = || (fetched.load(Ordering::SeqCst), revalidated.load(Ordering::SeqCst));

    let region = wait(|done| source.create_offline_region(&definition(STYLE_URL), &Vec::new(), done)).unwrap();
    let statuses = observe(&source, &region);
    wait(|done| source.set_offline_region_download_state(region.id, OfflineRegionDownloadState::Active, done)).unwrap();
    let downloaded = wait_until_complete(&statuses);
    assert_eq!((1, 0), calls());

    wait(|done| source.invalidate_offline_region(region.id, done)).unwrap();
    wait(|done| source.set_offline_region_download_state(region.id, OfflineRegionDownloadState::Active, done)).unwrap();
    let refreshed = wait_until_complete(&statuses);
    assert_eq!((1, 1), calls());
    assert_eq!(downloaded.completed_resource_count, refreshed.completed_resource_count);
    assert_eq!(downloaded.completed_resource_size, refreshed.completed_resource_size);
}

#[test]
fn merging_copies_the_regions_of_another_database() {
    let dir = TempDir::new();
    let side_path = dir.path("side.db");
    let side_region = OfflineDatabase::new(&side_path)
        .create_region(&definition("http://example.com/style.json"), &b"side".to_vec())
        .unwrap();
    let (network, _) = style_network(ResponseType::Synchronous);
    let source = database_file_source(&dir, &network);

    let merged = wait(|done| source.merge_offline_regions(&side_path, done)).unwrap();
    assert_eq!(1, merged.len());
    assert_eq!(side_region.definition, merged[0].definition);
    assert_eq!(b"side", merged[0].metadata.as_slice());
    assert_eq!(merged, wait(|done| source.list_offline_regions(done)).unwrap());

    let missing = wait(|done| source.merge_offline_regions(&dir.path("missing.db"), done));
    assert!(
        matches!(&missing, Err(DatabaseError::Runtime(message)) if message.contains("user_version")),
        "{missing:?}"
    );
}
