// The database runs on a worker thread of its own ("DatabaseFileSource"),
// which owns it and runs the tasks of its queue: requests and operations
// reach the worker as tasks, and the responses go to the requests'
// responders.
//
// The offline region downloads need the style parser: `offline` keeps them in the worker's extension, which the
// worker owns beside the database, and which posts its own work to the
// worker's queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_util::constants::{ASSET_PROTOCOL, FILE_PROTOCOL};
use mln_util::queue::Queue;
use parking_lot::Mutex;

use crate::log_target;
use crate::storage::file_source::FileSource;
use crate::storage::offline_database::{DatabaseError, OfflineDatabase};
use crate::storage::request::Responder;
use crate::storage::resource::{LoadingMethod, Resource, StoragePolicy};
use crate::storage::response::{Reason, ResourceError, Response};

/// How long cache hits' `accessed` timestamps wait, so that the reads of a
/// map load share one write.
const ACCESSED_FLUSH_DELAY: Duration = Duration::from_millis(250);

/// The queue of a database's worker, whose tasks run on the database's
/// thread in the order they were posted. Once the database has dropped, the
/// worker has ended and the queue drops what is posted to it.
pub type DatabaseQueue<E> = Queue<DatabaseWorker<E>>;

/// What a database's worker owns, on the database's thread: the database,
/// and what an extension keeps beside it, such as the offline regions'
/// downloads.
pub struct DatabaseWorker<E: 'static> {
    pub db: OfflineDatabase,
    pub extension: E,
    network: Option<Arc<dyn FileSource>>,
}

/// Makes the extension of a database worker, on the worker's thread, from
/// the worker's queue and its network.
type MakeExtension<E> = Box<dyn FnOnce(DatabaseQueue<E>, Option<Arc<dyn FileSource>>) -> E + Send>;

impl<E: 'static> DatabaseWorker<E> {
    fn new(
        queue: &DatabaseQueue<E>,
        network: Option<Arc<dyn FileSource>>,
        cache_path: &str,
        read_only: bool,
        make: MakeExtension<E>,
    ) -> Self {
        let mut db = OfflineDatabase::new(cache_path);
        if read_only {
            db.reopen_database_read_only(true);
        }
        // A cache hit that leaves `accessed` timestamps to write, while no
        // write is due, makes one due. A failed write isn't retried until
        // the next cache hit, so an idle map stays idle.
        let due = Arc::new(AtomicBool::new(false));
        let flush = queue.clone();
        db.set_accessed_callback(Box::new(move || {
            if !due.swap(true, Ordering::Relaxed) {
                let due = Arc::clone(&due);
                flush.post_after(ACCESSED_FLUSH_DELAY, move |worker| {
                    due.store(false, Ordering::Relaxed);
                    worker.db.flush_accessed();
                });
            }
        }));
        let extension = make(queue.clone(), network.clone());
        DatabaseWorker { db, extension, network }
    }

    /// The network that offline regions download through, if there is one.
    pub fn network(&self) -> Option<&Arc<dyn FileSource>> {
        self.network.as_ref()
    }

    /// Answers a request with the stored copy, whatever its freshness: its
    /// caching rules say how it may be used. Without a copy, the answer has
    /// no content and a `NotFound` error.
    fn request(&mut self, resource: &Resource, responder: &Responder) {
        let offline_response =
            if resource.storage_policy == StoragePolicy::Volatile { None } else { self.db.get(resource) };
        let response = offline_response.unwrap_or_else(|| {
            let mut response = Response::new();
            response.no_content = true;
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Not found in offline database")));
            response
        });
        responder.send(response);
    }

    fn maintain(
        &mut self,
        operation: impl FnOnce(&mut OfflineDatabase) -> Result<(), DatabaseError>,
        callback: impl FnOnce(Result<(), DatabaseError>),
    ) {
        callback(operation(&mut self.db));
    }
}

/// A cache that a [`Loader`](crate::storage::Loader) reads before the network, and
/// stores the network's responses in.
pub trait Cache: FileSource {
    /// Stores a response to `resource`, unless the resource is volatile.
    fn put(&self, resource: &Resource, response: &Response);
}

/// The `Database` file source: the ambient cache and the offline regions,
/// in an SQLite database that a worker thread owns, with the worker's
/// extension `E` beside it. Its drop or [`shut_down`](Self::shut_down) waits
/// for the worker to end, unless it runs on the worker's thread. When the
/// worker's thread can't start, the source logs why, fails each request, and
/// drops the work sent to it.
pub struct DatabaseFileSource<E: 'static = ()> {
    queue: DatabaseQueue<E>,
    /// The worker's thread, until the database shuts down.
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DatabaseFileSource {
    /// A database at `cache_path`, created if needed; `":memory:"` keeps it
    /// in memory. Offline regions download through `network`, and can't
    /// without one.
    #[must_use]
    pub fn new(cache_path: &str, network: Option<Arc<dyn FileSource>>) -> Self {
        Self::with_extension(cache_path, network, false, |_, _| ())
    }

    /// The database at `cache_path`, opened read-only: it answers requests
    /// but stores nothing.
    #[must_use]
    pub fn read_only(cache_path: &str, network: Option<Arc<dyn FileSource>>) -> Self {
        Self::with_extension(cache_path, network, true, |_, _| ())
    }
}

impl<E: 'static> DatabaseFileSource<E> {
    /// A database at `cache_path`, read-only or not, whose worker keeps the
    /// extension that `make` makes on the worker's thread, from the
    /// worker's queue and the database's network.
    #[must_use]
    pub fn with_extension(
        cache_path: &str,
        network: Option<Arc<dyn FileSource>>,
        read_only: bool,
        make: impl FnOnce(DatabaseQueue<E>, Option<Arc<dyn FileSource>>) -> E + Send + 'static,
    ) -> Self {
        let cache_path = cache_path.to_owned();
        let make: MakeExtension<E> = Box::new(make);
        let queue = DatabaseQueue::new();
        let own = queue.clone();
        let thread = mln_util::thread::spawn("DatabaseFileSource", move || {
            let mut worker = DatabaseWorker::new(&own, network, &cache_path, read_only, make);
            own.run(&mut worker);
            // What comes once the database has dropped drops at once, so the
            // handles that the worker's extension holds can't keep the queue
            // alive.
            own.close();
            drop(worker);
        })
        .inspect_err(|error| log::error!(target: log_target::DATABASE, "The database's thread didn't start: {error}"))
        .ok();
        if thread.is_none() {
            queue.close();
        }
        DatabaseFileSource { queue, thread: Mutex::new(thread) }
    }

    /// Holds the database thread until the guard drops (`crate::storage::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::storage::tests::Paused {
        crate::storage::tests::pause(|hold| self.invoke(move |_| hold()))
    }

    /// Whether a task waits on the database thread for its time to come,
    /// such as a write of cache hits' `accessed` timestamps.
    #[cfg(test)]
    pub(crate) fn has_timed_task(&self) -> bool {
        self.queue.next_deadline().is_some()
    }

    /// Switches to the database at `path`, creating it if needed. The
    /// callback runs on the database thread.
    #[cfg(test)]
    pub(crate) fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        let path = path.to_owned();
        self.invoke(move |t| {
            t.db.change_path(&path);
            if let Some(callback) = callback {
                callback();
            }
        });
    }

    /// Stores a response to `resource`, then runs `then` on the database
    /// thread.
    #[cfg(test)]
    pub(crate) fn put_then(&self, resource: &Resource, response: &Response, then: Box<dyn FnOnce() + Send>) {
        let resource = resource.clone();
        let response = response.clone();
        self.invoke(move |t| {
            t.db.put(&resource, &response);
            then();
        });
    }

    /// Deletes the database file and starts an empty one. The callback runs
    /// on the database thread.
    pub fn reset_database(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.maintain(OfflineDatabase::reset_database, callback));
    }

    /// Vacuums the database file to the space its data needs. The callback
    /// runs on the database thread.
    pub fn pack_database(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.maintain(OfflineDatabase::pack, callback));
    }

    /// Makes the ambient cache's resources revalidate with their server
    /// before use. Offline regions' resources are unaffected. The callback
    /// runs on the database thread.
    pub fn invalidate_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.maintain(OfflineDatabase::invalidate_ambient_cache, callback));
    }

    /// Erases the ambient cache's resources. Offline regions' resources are
    /// unaffected. The callback runs on the database thread.
    pub fn clear_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.maintain(OfflineDatabase::clear_ambient_cache, callback));
    }

    /// Limits the database to `size` bytes, evicting ambient resources to
    /// fit. Offline regions count towards the limit but are never evicted; 0
    /// disables the ambient cache. The callback runs on the database thread.
    pub fn set_maximum_ambient_cache_size(
        &self,
        size: u64,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| t.maintain(|db| db.set_maximum_ambient_cache_size(size), callback));
    }

    /// Runs `f` on the database thread, after the requests and operations
    /// sent before it.
    pub fn invoke(&self, f: impl FnOnce(&mut DatabaseWorker<E>) + Send + 'static) {
        self.queue.post(f);
    }

    /// Stops the database, as its drop does, for an owner that must know
    /// its worker has ended although the database may be held elsewhere.
    /// The first call waits for the worker, unless it runs on the worker's
    /// thread; a later call returns at once, so one owner shuts it down.
    /// The work sent after drops, and requests fail as when the thread
    /// didn't start.
    pub fn shut_down(&self) {
        self.queue.stop();
        // Released before the join, as the thread may load from this source
        // meanwhile.
        let thread = self.thread.lock().take();
        if let Some(thread) = thread
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

impl<E: 'static> Drop for DatabaseFileSource<E> {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl<E: 'static> Cache for DatabaseFileSource<E> {
    fn put(&self, resource: &Resource, response: &Response) {
        if resource.storage_policy == StoragePolicy::Volatile {
            return;
        }
        let resource = resource.clone();
        let response = response.clone();
        self.invoke(move |t| {
            t.db.put(&resource, &response);
        });
    }
}

impl<E: 'static> FileSource for DatabaseFileSource<E> {
    fn load(&self, resource: &Resource, responder: Responder) {
        if self.thread.lock().is_none() {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "The database's thread isn't running.")));
            responder.send(response);
            return;
        }
        let resource = resource.clone();
        self.invoke(move |worker| {
            if !responder.is_cancelled() {
                worker.request(&resource, &responder);
            }
        });
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(LoadingMethod::Cache)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn a_drop_waits_for_the_work_sent_before() {
        let database = DatabaseFileSource::new(":memory:", None);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        database.invoke(move |_| {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        });
        drop(database);
        assert!(ran.load(Ordering::SeqCst));
    }

    /// The last owner of a database may drop it on the database's thread.
    #[test]
    fn a_database_dropped_on_its_own_thread_ends() {
        let database = DatabaseFileSource::new(":memory:", None);
        let queue = database.queue.clone();
        let (dropped, done) = mpsc::channel();
        queue.post(move |_| {
            drop(database);
            dropped.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(10)).expect("the drop returns");
    }
}
