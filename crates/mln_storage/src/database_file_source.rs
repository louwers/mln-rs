// The database runs on a worker thread of its own ("DatabaseFileSource"),
// which owns it; requests and operations reach the worker as actor messages
// and the responses go to the requests' responders.
//
// The offline region downloads need the style parser, which sits above this
// crate: mln_storage_offline keeps them in the worker's extension, which the
// worker owns beside the database.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_actor::{ActorRef, Thread, Timer};
use mln_util::constants::{ASSET_PROTOCOL, FILE_PROTOCOL};

use crate::file_source::FileSource;
use crate::offline_database::{DatabaseError, OfflineDatabase};
use crate::request::Responder;
use crate::resource::{LoadingMethod, Resource, StoragePolicy};
use crate::response::{Reason, ResourceError, Response};

/// How long cache hits' `accessed` timestamps wait, so that the reads of a
/// map load share one write.
const ACCESSED_FLUSH_DELAY: Duration = Duration::from_millis(250);

/// What a database worker owns: the database, and what an extension keeps
/// beside it, such as the offline regions' downloads.
pub struct DatabaseState<E> {
    pub db: OfflineDatabase,
    pub extension: E,
}

/// The worker that owns a database, on the database's thread.
pub struct DatabaseWorker<E: 'static> {
    state: RefCell<DatabaseState<E>>,
    network: Option<Arc<dyn FileSource>>,
    self_ref: ActorRef<DatabaseWorker<E>>,
    accessed_flush_timer: Timer,
    /// A flush of the `accessed` timestamps is scheduled. A failed flush
    /// isn't rescheduled until the next cache hit, so an idle map stays
    /// idle.
    accessed_flush_scheduled: Arc<AtomicBool>,
}

/// Makes the extension of a database worker, on the worker's thread, from the
/// worker's own reference and its network.
type MakeExtension<E> = Box<dyn FnOnce(ActorRef<DatabaseWorker<E>>, Option<Arc<dyn FileSource>>) -> E + Send>;

impl<E: 'static> DatabaseWorker<E> {
    fn new(
        self_ref: ActorRef<DatabaseWorker<E>>,
        network: Option<Arc<dyn FileSource>>,
        cache_path: &str,
        read_only: bool,
        make: MakeExtension<E>,
    ) -> Self {
        let mut db = OfflineDatabase::new(cache_path);
        if read_only {
            db.reopen_database_read_only(true);
        }
        let accessed_flush_scheduled = Arc::new(AtomicBool::new(false));
        let (scheduled, actor) = (Arc::clone(&accessed_flush_scheduled), self_ref.clone());
        db.set_accessed_callback(Box::new(move || {
            if !scheduled.swap(true, Ordering::Relaxed) {
                actor.invoke(|t: &DatabaseWorker<E>| t.start_accessed_flush_timer());
            }
        }));
        let extension = make(self_ref.clone(), network.clone());
        DatabaseWorker {
            state: RefCell::new(DatabaseState { db, extension }),
            network,
            self_ref,
            accessed_flush_timer: Timer::new(),
            accessed_flush_scheduled,
        }
    }

    fn start_accessed_flush_timer(&self) {
        let actor = self.self_ref.clone();
        self.accessed_flush_timer.start(ACCESSED_FLUSH_DELAY, Duration::ZERO, move || {
            actor.invoke(|t: &DatabaseWorker<E>| {
                t.accessed_flush_scheduled.store(false, Ordering::Relaxed);
                t.with(|state| state.db.flush_accessed());
            });
        });
    }

    /// Runs `f` with what the worker owns. What `f` sets off comes back
    /// later, as a message to the worker, not inside `f`.
    pub fn with<R>(&self, f: impl FnOnce(&mut DatabaseState<E>) -> R) -> R {
        f(&mut self.state.borrow_mut())
    }

    /// The network that offline regions download through, if there is one.
    pub fn network(&self) -> Option<&Arc<dyn FileSource>> {
        self.network.as_ref()
    }

    pub(crate) fn request(&self, resource: &Resource, responder: &Responder) {
        let mut offline_response = if resource.storage_policy == StoragePolicy::Volatile {
            None
        } else {
            self.with(|state| state.db.get(resource))
        };
        match &mut offline_response {
            None => {
                let mut response = Response::new();
                response.no_content = true;
                response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Not found in offline database")));
                offline_response = Some(response);
            }
            Some(response) => {
                if !response.is_usable() {
                    response.error =
                        Some(Box::new(ResourceError::new(Reason::NotFound, "Cached resource is unusable")));
                }
            }
        }
        responder.send(offline_response.expect("response"));
    }

    fn maintain(
        &self,
        operation: impl FnOnce(&mut OfflineDatabase) -> Option<DatabaseError>,
        callback: impl FnOnce(Result<(), DatabaseError>),
    ) {
        let error = self.with(|state| operation(&mut state.db));
        callback(error.map_or(Ok(()), Err));
    }
}

/// A cache that a [`Loader`](crate::Loader) reads before the network, and
/// stores the network's responses in.
pub trait Cache: FileSource {
    /// Stores a response to `resource`, unless the resource is volatile.
    fn put(&self, resource: &Resource, response: &Response);
}

/// The `Database` file source: the ambient cache and the offline regions,
/// in an SQLite database that a worker thread owns, with the worker's
/// extension `E` beside it.
pub struct DatabaseFileSource<E: 'static = ()> {
    thread: Thread<DatabaseWorker<E>>,
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
    /// extension that `make` makes on the worker's thread.
    #[must_use]
    pub fn with_extension(
        cache_path: &str,
        network: Option<Arc<dyn FileSource>>,
        read_only: bool,
        make: impl FnOnce(ActorRef<DatabaseWorker<E>>, Option<Arc<dyn FileSource>>) -> E + Send + 'static,
    ) -> Self {
        let cache_path = cache_path.to_owned();
        let make: MakeExtension<E> = Box::new(make);
        let thread = Thread::new("DatabaseFileSource", move |self_ref| {
            DatabaseWorker::new(self_ref, network, &cache_path, read_only, make)
        });
        DatabaseFileSource { thread }
    }

    /// Holds the database thread until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(&self.thread.actor())
    }

    /// Switches to the database at `path`, creating it if needed. The
    /// callback runs on the database thread.
    #[cfg(test)]
    pub(crate) fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        let path = path.to_owned();
        self.invoke(move |t| {
            t.with(|state| state.db.change_path(&path));
            if let Some(callback) = callback {
                callback();
            }
        });
    }

    /// Stores a response to `resource`, then runs `then` on this thread.
    #[cfg(test)]
    pub(crate) fn put_then(&self, resource: &Resource, response: &Response, then: Box<dyn FnOnce() + Send>) {
        let mut then = mln_actor::scheduler::bind_once(&mln_actor::scheduler::get_current(), then);
        let resource = resource.clone();
        let response = response.clone();
        self.invoke(move |t| {
            t.with(|state| state.db.put(&resource, &response));
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
    pub fn invoke(&self, f: impl FnOnce(&DatabaseWorker<E>) + Send + 'static) {
        self.thread.actor().invoke(f);
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
            t.with(|state| state.db.put(&resource, &response));
        });
    }
}

impl<E: 'static> FileSource for DatabaseFileSource<E> {
    fn load(&self, resource: &Resource, responder: Responder) {
        let resource = resource.clone();
        self.invoke(move |t| {
            if !responder.is_cancelled() {
                t.request(&resource, &responder);
            }
        });
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(LoadingMethod::Cache)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}
