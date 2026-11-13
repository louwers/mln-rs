//! The offline regions of the browser build, which has none: it has no cache
//! database, and the browser caches the runtime's requests. Each call fails
//! with [`ErrorKind::Unsupported`](mln_util::ErrorKind), as offline.rs's
//! would on a runtime without a database.

use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionStatus,
};
use mln_util::error::Error;

use crate::completion::Completion;
use crate::event::EventMask;
use crate::runtime::Runtime;

/// The offline regions in a runtime's cache database, from
/// [`Runtime::offline`]: none in the browser.
pub struct Offline<'a> {
    runtime: &'a Runtime,
}

fn no_database<T>(runtime: &Runtime) -> Result<T, Error> {
    let _ = runtime;
    Err(Error::unsupported("the browser build has no cache database, so no offline regions"))
}

#[allow(clippy::needless_pass_by_value, reason = "the signatures of offline.rs")]
impl<'a> Offline<'a> {
    pub(crate) fn new(runtime: &'a Runtime) -> Self {
        Offline { runtime }
    }

    pub fn create_region(
        &self,
        definition: OfflineRegionDefinition,
        metadata: OfflineRegionMetadata,
    ) -> Result<Completion<OfflineRegion>, Error> {
        let _ = (definition, metadata);
        no_database(self.runtime)
    }

    pub fn region(&self, region_id: i64) -> Result<Completion<Option<OfflineRegion>>, Error> {
        let _ = region_id;
        no_database(self.runtime)
    }

    pub fn regions(&self) -> Result<Completion<Vec<OfflineRegion>>, Error> {
        no_database(self.runtime)
    }

    pub fn merge_regions(
        &self,
        side_database_path: impl Into<String>,
    ) -> Result<Completion<Vec<OfflineRegion>>, Error> {
        let _ = side_database_path.into();
        no_database(self.runtime)
    }

    pub fn update_region_metadata(
        &self,
        region_id: i64,
        metadata: OfflineRegionMetadata,
    ) -> Result<Completion<OfflineRegion>, Error> {
        let _ = (region_id, metadata);
        no_database(self.runtime)
    }

    pub fn region_status(&self, region_id: i64) -> Result<Completion<OfflineRegionStatus>, Error> {
        let _ = region_id;
        no_database(self.runtime)
    }

    pub fn set_region_observed(&self, region_id: i64, events: EventMask) -> Result<Completion<()>, Error> {
        let _ = (region_id, events);
        no_database(self.runtime)
    }

    pub fn set_region_download_state(
        &self,
        region_id: i64,
        state: OfflineRegionDownloadState,
    ) -> Result<Completion<()>, Error> {
        let _ = (region_id, state);
        no_database(self.runtime)
    }

    pub fn invalidate_region(&self, region_id: i64) -> Result<Completion<()>, Error> {
        let _ = region_id;
        no_database(self.runtime)
    }

    pub fn delete_region(&self, region_id: i64) -> Result<Completion<()>, Error> {
        let _ = region_id;
        no_database(self.runtime)
    }
}
