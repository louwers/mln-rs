// `DatabaseFileSource` is in mln_storage, below the style parser that
// `OfflineDownload` needs, so the region API is an extension trait here, on a
// database whose worker keeps the regions' `Downloads` beside the database.
// Its calls run on the database's thread, after those sent before.

use std::sync::Arc;

use mln_actor::ActorRef;
use mln_storage::database_file_source::DatabaseWorker;
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver,
    OfflineRegionStatus, OfflineRegions,
};
use mln_storage::offline_database::{DatabaseError, OfflineDatabase};
use mln_storage::{DatabaseFileSource, FileSource};

use crate::offline_download::{Downloads, Post};

/// A database whose worker downloads its offline regions.
pub type RegionDatabase = DatabaseFileSource<Downloads>;

/// A database at `cache_path` whose regions download through `network`.
#[must_use]
pub fn region_database(cache_path: &str, network: Option<Arc<dyn FileSource>>) -> RegionDatabase {
    DatabaseFileSource::with_extension(cache_path, network, false, |worker, network| {
        Downloads::new(network, post_to(worker))
    })
}

/// Posts a download's task to the database's worker.
fn post_to(worker: ActorRef<DatabaseWorker<Downloads>>) -> Post {
    Arc::new(move |task| {
        worker.invoke(move |worker: &DatabaseWorker<Downloads>| {
            worker.with(|state| task(&mut state.db, &mut state.extension));
        });
    })
}

/// The region `region_id`, or [`DatabaseError::RegionNotFound`] when the
/// database has none with that ID.
fn require_region(db: &mut OfflineDatabase, region_id: i64) -> Result<OfflineRegion, DatabaseError> {
    db.get_region(region_id)?.ok_or(DatabaseError::RegionNotFound(region_id))
}

/// The offline region API of a [`RegionDatabase`]. Every callback runs on
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

impl DatabaseFileSourceExt for RegionDatabase {
    fn list_offline_regions(&self, callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static) {
        self.invoke(move |t| callback(t.with(|state| state.db.list_regions())));
    }

    fn get_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<Option<OfflineRegion>, DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| callback(t.with(|state| state.db.get_region(region_id))));
    }

    fn create_offline_region(
        &self,
        definition: &OfflineRegionDefinition,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    ) {
        let (definition, metadata) = (definition.clone(), metadata.clone());
        self.invoke(move |t| callback(t.with(|state| state.db.create_region(&definition, &metadata))));
    }

    fn update_offline_metadata(
        &self,
        region_id: i64,
        metadata: &OfflineRegionMetadata,
        callback: impl FnOnce(Result<OfflineRegion, DatabaseError>) + Send + 'static,
    ) {
        let metadata = metadata.clone();
        self.invoke(move |t| {
            callback(t.with(|state| {
                require_region(&mut state.db, region_id)?;
                state.db.update_metadata(region_id, &metadata)?;
                require_region(&mut state.db, region_id)
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
            callback(t.with(|state| {
                let download = state.extension.download(&mut state.db, region_id)?;
                download.set_observer(observer.map(|observer| -> Box<dyn OfflineRegionObserver> { observer }));
                Ok(())
            }));
        });
    }

    fn set_offline_region_download_state(
        &self,
        region_id: i64,
        download_state: OfflineRegionDownloadState,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| {
            callback(t.with(|state| {
                state.extension.download(&mut state.db, region_id)?.set_state(download_state);
                Ok(())
            }));
        });
    }

    fn get_offline_region_status(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<OfflineRegionStatus, DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| {
            callback(t.with(|state| {
                let download = state.extension.download(&mut state.db, region_id)?;
                Ok(download.get_status(&mut state.db))
            }));
        });
    }

    fn merge_offline_regions(
        &self,
        side_database_path: &str,
        callback: impl FnOnce(Result<OfflineRegions, DatabaseError>) + Send + 'static,
    ) {
        let side_database_path = side_database_path.to_owned();
        self.invoke(move |t| callback(t.with(|state| state.db.merge_database(&side_database_path))));
    }

    fn delete_offline_region(&self, region_id: i64, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| {
            callback(t.with(|state| {
                let region = require_region(&mut state.db, region_id)?;
                drop(state.extension.remove(region_id));
                state.db.delete_region(&region).map_or(Ok(()), Err)
            }));
        });
    }

    fn invalidate_offline_region(
        &self,
        region_id: i64,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| {
            callback(t.with(|state| {
                require_region(&mut state.db, region_id)?;
                state.db.invalidate_region(region_id).map_or(Ok(()), Err)
            }));
        });
    }
}
