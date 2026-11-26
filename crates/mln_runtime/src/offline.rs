//! The offline regions in a runtime's cache database.

use std::path::Path;

use mln_storage::ResourceError;
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionExpiry, OfflineRegionMetadata,
    OfflineRegionObserver, OfflineRegionStatus,
};
use mln_storage_offline::DatabaseFileSourceExt;
use mln_style::error::Error;

use crate::completion::Completion;
use crate::event::{EventMask, EventSource, RuntimeEvent, RuntimeEventKind};
use crate::runtime::{CoreQueue, Runtime, post, reply_on};

/// The offline regions in a runtime's cache database, from
/// [`Runtime::offline`]. Each call is a command of the runtime, and the
/// regions download on the database's thread.
pub struct Offline<'a> {
    runtime: &'a Runtime,
}

impl<'a> Offline<'a> {
    pub(crate) fn new(runtime: &'a Runtime) -> Self {
        Offline { runtime }
    }

    /// Creates a region. It doesn't download until
    /// [`set_region_download_state`](Self::set_region_download_state) starts
    /// it.
    pub fn create_region(
        &self,
        definition: OfflineRegionDefinition,
        metadata: OfflineRegionMetadata,
    ) -> Result<Completion<OfflineRegion>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.create_offline_region(&definition, &metadata, reply_on(core.queue.clone(), resolver));
        })
    }

    /// The region `region_id`, or `None` when the database has none.
    pub fn region(&self, region_id: i64) -> Result<Completion<Option<OfflineRegion>>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.get_offline_region(region_id, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Every region in the database.
    pub fn regions(&self) -> Result<Completion<Vec<OfflineRegion>>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.list_offline_regions(reply_on(core.queue.clone(), resolver));
        })
    }

    /// Copies the regions of another cache database into this one, and gives
    /// this database's copy of each. A region that this database has already
    /// isn't copied again. The other database only needs to be readable.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) when
    /// `side_database_path` names no database file that opens for reading.
    /// That check opens the file on the calling thread.
    pub fn merge_regions(
        &self,
        side_database_path: impl Into<String>,
    ) -> Result<Completion<Vec<OfflineRegion>>, Error> {
        let side_database_path = side_database_path.into();
        check_side_database(&side_database_path)?;
        self.runtime.offline_command(move |database, core, resolver| {
            database.merge_offline_regions(&side_database_path, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Replaces a region's metadata, and gives the updated region.
    pub fn update_region_metadata(
        &self,
        region_id: i64,
        metadata: OfflineRegionMetadata,
    ) -> Result<Completion<OfflineRegion>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.update_offline_metadata(region_id, &metadata, reply_on(core.queue.clone(), resolver));
        })
    }

    /// A region's download progress.
    pub fn region_status(&self, region_id: i64) -> Result<Completion<OfflineRegionStatus>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.get_offline_region_status(region_id, reply_on(core.queue.clone(), resolver));
        })
    }

    /// How long a region's stored resources stay usable. Before
    /// [`usable_offline_until`](OfflineRegionExpiry::usable_offline_until), a
    /// download refreshes them.
    pub fn region_expiry(&self, region_id: i64) -> Result<Completion<OfflineRegionExpiry>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.get_offline_region_expiry(region_id, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Selects the events that the runtime reports about a region's
    /// download, from [`EventMask::ALL_RUNTIME_EVENTS`]. It replaces the
    /// region's earlier selection, and [`EventMask::NONE`] stops them.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a
    /// mask with map events.
    pub fn set_region_observed(&self, region_id: i64, events: EventMask) -> Result<Completion<()>, Error> {
        if events.bits() & !EventMask::ALL_RUNTIME_EVENTS.bits() != 0 {
            return Err(Error::invalid_argument("an offline region reports only runtime events"));
        }
        self.runtime.offline_command(move |database, core, resolver| {
            let observer = (events != EventMask::NONE).then(|| -> Box<dyn OfflineRegionObserver + Send> {
                Box::new(RegionObserver { region_id, events, queue: core.queue.clone() })
            });
            database.set_offline_region_observer(region_id, observer, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Starts or pauses a region's download. A download fetches the
    /// resources the region lacks, and revalidates those that are stale.
    pub fn set_region_download_state(
        &self,
        region_id: i64,
        state: OfflineRegionDownloadState,
    ) -> Result<Completion<()>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.set_offline_region_download_state(region_id, state, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Makes each resource of a region stale, so maps check it with its
    /// server when they next use it, and the region's next download
    /// refreshes them all. The region keeps its resources, which stay usable
    /// unless their server requires checking them first.
    pub fn invalidate_region(&self, region_id: i64) -> Result<Completion<()>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.invalidate_offline_region(region_id, reply_on(core.queue.clone(), resolver));
        })
    }

    /// Deletes a region, with the resources that no other region uses.
    pub fn delete_region(&self, region_id: i64) -> Result<Completion<()>, Error> {
        self.runtime.offline_command(move |database, core, resolver| {
            database.delete_offline_region(region_id, reply_on(core.queue.clone(), resolver));
        })
    }
}

/// Checks that `path` names an existing file that opens for reading. SQLite would otherwise take an empty path, `:memory:` or a
/// `file:` URI for an in-memory database, and create a missing file.
fn check_side_database(path: &str) -> Result<(), Error> {
    let unreadable = || Error::invalid_argument("side database path must name an existing readable database file");
    if path.is_empty() || path.starts_with(':') || path.starts_with("file:") || !Path::new(path).is_file() {
        return Err(unreadable());
    }
    std::fs::File::open(path).map(drop).map_err(|_| unreadable())
}

/// Sends an observed region's download events to the runtime's handler, on
/// the runtime's thread.
struct RegionObserver {
    region_id: i64,
    events: EventMask,
    queue: CoreQueue,
}

impl RegionObserver {
    fn send(&self, kind: RuntimeEventKind) {
        if !self.events.contains(kind.event_type()) {
            return;
        }
        let event = RuntimeEvent { source: EventSource::Runtime, kind };
        post(&self.queue, move |core, outbox| core.send_events(vec![event], outbox));
    }
}

impl OfflineRegionObserver for RegionObserver {
    fn status_changed(&self, status: OfflineRegionStatus) {
        self.send(RuntimeEventKind::OfflineRegionStatusChanged { region_id: self.region_id, status });
    }

    fn response_error(&self, error: ResourceError) {
        self.send(RuntimeEventKind::OfflineRegionResponseError { region_id: self.region_id, error });
    }
}
