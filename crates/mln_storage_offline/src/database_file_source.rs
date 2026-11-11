// Ports the offline region half of include/mln/storage/database_file_source.hpp
// and platform/default/src/mln/storage/database_file_source.cpp.
//
// `DatabaseFileSource` itself is in mln_storage, below the style parser that
// `OfflineDownload` needs, so the region API is an extension trait here. Its
// messages run on the database thread like the C++ ones; the downloads live
// there in a `DatabaseFileSourceThread::extension`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mln_storage::DatabaseFileSource;
use mln_storage::database_file_source::DatabaseFileSourceThread;
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver,
    OfflineRegionStatus, OfflineRegions,
};
use mln_storage::offline_database::DatabaseError;

use crate::offline_download::OfflineDownload;

/// `DatabaseFileSourceThread::downloads`: one download per region whose
/// observer, download state or status was asked for, until the region is
/// deleted.
#[derive(Default)]
struct Downloads {
    downloads: RefCell<BTreeMap<i64, Rc<OfflineDownload>>>,
}

/// `DatabaseFileSourceThread::getDownload(regionID)`.
fn get_download(thread: &DatabaseFileSourceThread, region_id: i64) -> Result<Rc<OfflineDownload>, DatabaseError> {
    let Some(online_file_source) = thread.online_file_source() else {
        return Err(DatabaseError::Runtime("Network file source unavailable.".to_owned()));
    };
    let downloads = thread.extension::<Downloads>();
    if let Some(download) = downloads.downloads.borrow().get(&region_id) {
        return Ok(download.clone());
    }
    let definition = thread.database().borrow_mut().get_region_definition(region_id)?;
    let download =
        Rc::new(OfflineDownload::new(region_id, definition, thread.database().clone(), online_file_source.clone()));
    downloads.downloads.borrow_mut().insert(region_id, download.clone());
    Ok(download)
}

/// The offline region API of `DatabaseFileSource`. Every callback runs on
/// the database thread.
pub trait DatabaseFileSourceExt {
    /// `listOfflineRegions(callback)`: every region in the database.
    fn list_offline_regions(&self, callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static);

    /// `getOfflineRegion(regionID, callback)`: the region, or `None` if the
    /// database has none with that id.
    fn get_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<Option<OfflineRegion>, DatabaseError>) + Send + 'static,
    );

    /// `createOfflineRegion(definition, metadata, callback)`. The region
    /// starts inactive; `set_offline_region_download_state` starts its
    /// download.
    fn create_offline_region(
        &self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    );

    /// `updateOfflineMetadata(regionID, metadata, callback)`.
    fn update_offline_metadata(
        &self,
        region_id: i64,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegionMetadata, DatabaseError>) + Send + 'static,
    );

    /// `setOfflineRegionObserver(region, observer)`: the region's only
    /// observer, replacing the previous one; `None` removes it. Its methods
    /// run on the database thread.
    fn set_offline_region_observer(
        &self,
        region: &OfflineRegion,
        observer: Option<Box<dyn OfflineRegionObserver + Send>>,
    );

    /// `setOfflineRegionDownloadState(region, state)`: starts or stops the
    /// region's download.
    fn set_offline_region_download_state(&self, region: &OfflineRegion, state: OfflineRegionDownloadState);

    /// `getOfflineRegionStatus(region, callback)`.
    fn get_offline_region_status(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<OfflineRegionStatus, DatabaseError>) + Send + 'static,
    );

    /// `mergeOfflineRegions(sideDatabasePath, callback)`: copies the regions
    /// of another database, and the resources and tiles they use, into this
    /// one. Identical regions become one. The side database is upgraded to
    /// the current schema in place, so it must be writable. Fails with
    /// `DatabaseError::MapboxTileLimitExceeded` if the merge would exceed
    /// the offline Mapbox tile limit.
    fn merge_offline_regions(
        &self,
        side_database_path: &str,
        callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static,
    );

    /// `deleteOfflineRegion(region, callback)`: stops the region's download
    /// and deletes the region, evicting the resources no other region uses.
    fn delete_offline_region(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    );

    /// `invalidateOfflineRegion(region, callback)`: makes the region's
    /// resources revalidate with their server before use, which transfers
    /// nothing for resources that haven't changed.
    fn invalidate_offline_region(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    );
}

impl DatabaseFileSourceExt for DatabaseFileSource {
    fn list_offline_regions(&self, callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static) {
        self.invoke(move |t| callback(t.database().borrow_mut().list_regions()));
    }

    fn get_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<Option<OfflineRegion>, DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| callback(t.database().borrow_mut().get_region(region_id)));
    }

    fn create_offline_region(
        &self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    ) {
        let (definition, metadata) = (definition.clone(), metadata.clone());
        self.invoke(move |t| callback(t.database().borrow_mut().create_region(&definition, &metadata)));
    }

    fn update_offline_metadata(
        &self,
        region_id: i64,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegionMetadata, DatabaseError>) + Send + 'static,
    ) {
        let metadata = metadata.clone();
        self.invoke(move |t| callback(t.database().borrow_mut().update_metadata(region_id, &metadata)));
    }

    fn set_offline_region_observer(
        &self,
        region: &OfflineRegion,
        observer: Option<Box<dyn OfflineRegionObserver + Send>>,
    ) {
        let region_id = region.get_id();
        self.invoke(move |t| {
            if let Ok(download) = get_download(t, region_id) {
                let observer = observer.map(|observer| -> Rc<dyn OfflineRegionObserver> {
                    let observer: Rc<dyn OfflineRegionObserver + Send> = Rc::from(observer);
                    observer
                });
                download.set_observer(observer);
            }
        });
    }

    fn set_offline_region_download_state(&self, region: &OfflineRegion, state: OfflineRegionDownloadState) {
        let region_id = region.get_id();
        self.invoke(move |t| {
            if let Ok(download) = get_download(t, region_id) {
                download.set_state(state);
            }
        });
    }

    fn get_offline_region_status(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<OfflineRegionStatus, DatabaseError>) + Send + 'static,
    ) {
        let region_id = region.get_id();
        self.invoke(move |t| callback(get_download(t, region_id).map(|download| download.get_status())));
    }

    fn merge_offline_regions(
        &self,
        side_database_path: &str,
        callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static,
    ) {
        let side_database_path = side_database_path.to_owned();
        self.invoke(move |t| callback(t.database().borrow_mut().merge_database(&side_database_path)));
    }

    fn delete_offline_region(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        let region = region.clone();
        self.invoke(move |t| {
            let download = t.extension::<Downloads>().downloads.borrow_mut().remove(&region.get_id());
            drop(download);
            let error = t.database().borrow_mut().delete_region(region);
            callback(error.map_or(Ok(()), Err));
        });
    }

    fn invalidate_offline_region(
        &self,
        region: &OfflineRegion,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        let region_id = region.get_id();
        self.invoke(move |t| {
            let error = t.database().borrow_mut().invalidate_region(region_id);
            callback(error.map_or(Ok(()), Err));
        });
    }
}
