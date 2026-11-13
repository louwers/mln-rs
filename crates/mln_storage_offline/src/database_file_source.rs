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

/// One download per region whose observer, download state or status was
/// asked for, until the region is deleted.
#[derive(Default)]
struct Downloads {
    downloads: RefCell<BTreeMap<i64, Rc<OfflineDownload>>>,
}

fn get_download(thread: &DatabaseFileSourceThread, region_id: i64) -> Result<Rc<OfflineDownload>, DatabaseError> {
    let Some(online_file_source) = thread.online_file_source() else {
        return Err(DatabaseError::Runtime("Network file source unavailable.".to_owned()));
    };
    let downloads = thread.extension::<Downloads>();
    if let Some(download) = downloads.downloads.borrow().get(&region_id) {
        return Ok(download.clone());
    }
    let definition = require_region(thread, region_id)?.definition.clone();
    let download =
        Rc::new(OfflineDownload::new(region_id, definition, thread.database().clone(), online_file_source.clone()));
    downloads.downloads.borrow_mut().insert(region_id, download.clone());
    Ok(download)
}

/// The offline region API of `DatabaseFileSource`. Every callback runs on
/// the database thread.
pub trait DatabaseFileSourceExt {
    /// Lists every region in the database.
    fn list_offline_regions(&self, callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static);

    /// Gets the region `region_id`, or `None` if the database has none with
    /// that id.
    fn get_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<Option<OfflineRegion>, DatabaseError>) + Send + 'static,
    );

    /// Creates a region. The region starts inactive;
    /// `set_offline_region_download_state` starts its download.
    fn create_offline_region(
        &self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    );

    /// Replaces the region's metadata, and gives the updated region. Fails
    /// when the database has no region `region_id`.
    fn update_offline_metadata(
        &self,
        region_id: i64,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    );

    /// Sets the region's only observer, replacing the previous one; `None`
    /// removes it. Its methods run on the database thread. Fails when the
    /// database has no region `region_id`.
    fn set_offline_region_observer(
        &self,
        region_id: i64,
        observer: Option<Box<dyn OfflineRegionObserver + Send>>,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    );

    /// Starts or stops the region's download. Fails when the database has no
    /// region `region_id`.
    fn set_offline_region_download_state(
        &self,
        region_id: i64,
        state: OfflineRegionDownloadState,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    );

    fn get_offline_region_status(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<OfflineRegionStatus, DatabaseError>) + Send + 'static,
    );

    /// Copies the regions of another database, and the resources and tiles
    /// they use, into this one. Identical regions become one. The side
    /// database is upgraded to the current schema in place, so it must be
    /// writable.
    fn merge_offline_regions(
        &self,
        side_database_path: &str,
        callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static,
    );

    /// Stops the region's download and deletes the region, evicting the
    /// resources no other region uses. Fails when the database has no region
    /// `region_id`.
    fn delete_offline_region(&self, region_id: i64, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static);

    /// Makes the region's resources revalidate with their server before use,
    /// which transfers nothing for resources that haven't changed.
    fn invalidate_offline_region(
        &self,
        region_id: i64,
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
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    ) {
        let metadata = metadata.clone();
        self.invoke(move |t| {
            callback(require_region(t, region_id).and_then(|_| {
                t.database().borrow_mut().update_metadata(region_id, &metadata)?;
                require_region(t, region_id)
            }));
        });
    }

    fn set_offline_region_observer(
        &self,
        region_id: i64,
        observer: Option<Box<dyn OfflineRegionObserver + Send>>,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| {
            callback(get_download(t, region_id).map(|download| {
                let observer = observer.map(|observer| -> Rc<dyn OfflineRegionObserver> {
                    let observer: Rc<dyn OfflineRegionObserver + Send> = Rc::from(observer);
                    observer
                });
                download.set_observer(observer);
            }));
        });
    }

    fn set_offline_region_download_state(
        &self,
        region_id: i64,
        state: OfflineRegionDownloadState,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| callback(get_download(t, region_id).map(|download| download.set_state(state))));
    }

    fn get_offline_region_status(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<OfflineRegionStatus, DatabaseError>) + Send + 'static,
    ) {
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

    fn delete_offline_region(&self, region_id: i64, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| {
            callback(require_region(t, region_id).and_then(|region| {
                let download = t.extension::<Downloads>().downloads.borrow_mut().remove(&region_id);
                drop(download);
                t.database().borrow_mut().delete_region(&region).map_or(Ok(()), Err)
            }));
        });
    }

    fn invalidate_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| {
            callback(
                require_region(t, region_id)
                    .and_then(|_| t.database().borrow_mut().invalidate_region(region_id).map_or(Ok(()), Err)),
            );
        });
    }
}

/// The region `region_id`, or [`DatabaseError::RegionNotFound`] when the
/// database has none with that ID.
fn require_region(thread: &DatabaseFileSourceThread, region_id: i64) -> Result<OfflineRegion, DatabaseError> {
    thread.database().borrow_mut().get_region(region_id)?.ok_or(DatabaseError::RegionNotFound(region_id))
}
