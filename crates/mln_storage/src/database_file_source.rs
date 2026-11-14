// The database runs on its own thread ("DatabaseFileSource"); requests reach
// it as actor messages and the response goes to the request's responder.
//
// The offline region API needs `OfflineDownload`, which parses styles and so
// sits above this crate: mln_storage_offline adds it to `DatabaseFileSource`
// through `invoke` and `DatabaseFileSourceThread::extension`.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
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

/// Owns the `OfflineDatabase` on the database thread.
pub struct DatabaseFileSourceThread {
    db: Rc<RefCell<OfflineDatabase>>,
    online_file_source: Option<Arc<dyn FileSource>>,
    /// The state extensions keep on the database thread, one per type.
    extensions: RefCell<Vec<Rc<dyn Any>>>,
    self_ref: ActorRef<DatabaseFileSourceThread>,
    accessed_flush_timer: Timer,
    /// A flush of the `accessed` timestamps is scheduled. A failed flush
    /// isn't rescheduled until the next cache hit, so an idle map stays
    /// idle.
    accessed_flush_scheduled: Arc<AtomicBool>,
}

impl DatabaseFileSourceThread {
    fn new(
        self_ref: ActorRef<DatabaseFileSourceThread>,
        online_file_source: Option<Arc<dyn FileSource>>,
        cache_path: &str,
        read_only: bool,
    ) -> Self {
        let mut db = OfflineDatabase::new(cache_path);
        if read_only {
            db.reopen_database_read_only(true);
        }
        let accessed_flush_scheduled = Arc::new(AtomicBool::new(false));
        let (scheduled, actor) = (Arc::clone(&accessed_flush_scheduled), self_ref.clone());
        db.set_accessed_callback(Box::new(move || {
            if !scheduled.swap(true, Ordering::Relaxed) {
                actor.invoke(DatabaseFileSourceThread::start_accessed_flush_timer);
            }
        }));
        DatabaseFileSourceThread {
            db: Rc::new(RefCell::new(db)),
            online_file_source,
            extensions: RefCell::new(Vec::new()),
            self_ref,
            accessed_flush_timer: Timer::new(),
            accessed_flush_scheduled,
        }
    }

    fn start_accessed_flush_timer(&self) {
        let actor = self.self_ref.clone();
        self.accessed_flush_timer.start(ACCESSED_FLUSH_DELAY, Duration::ZERO, move || {
            actor.invoke(|t: &DatabaseFileSourceThread| {
                t.accessed_flush_scheduled.store(false, Ordering::Relaxed);
                t.db.borrow_mut().flush_accessed();
            });
        });
    }

    pub(crate) fn request(&self, resource: &Resource, responder: &Responder) {
        let mut offline_response =
            if resource.storage_policy == StoragePolicy::Volatile { None } else { self.db.borrow_mut().get(resource) };
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

    #[cfg(test)]
    pub(crate) fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        self.db.borrow_mut().change_path(path);
        if let Some(callback) = callback {
            callback();
        }
    }

    pub(crate) fn put(&self, resource: &Resource, response: &Response) {
        self.db.borrow_mut().put(resource, response);
    }

    pub(crate) fn reset_database(&self, callback: impl FnOnce(Result<(), DatabaseError>)) {
        callback(into_result(self.db.borrow_mut().reset_database()));
    }

    pub(crate) fn pack_database(&self, callback: impl FnOnce(Result<(), DatabaseError>)) {
        callback(into_result(self.db.borrow_mut().pack()));
    }

    pub(crate) fn invalidate_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>)) {
        callback(into_result(self.db.borrow_mut().invalidate_ambient_cache()));
    }

    pub(crate) fn clear_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>)) {
        callback(into_result(self.db.borrow_mut().clear_ambient_cache()));
    }

    pub(crate) fn set_maximum_ambient_cache_size(&self, size: u64, callback: impl FnOnce(Result<(), DatabaseError>)) {
        callback(into_result(self.db.borrow_mut().set_maximum_ambient_cache_size(size)));
    }

    /// The database. Borrow it only for the duration of one call: requests
    /// and other messages on this thread borrow it too.
    pub fn database(&self) -> &Rc<RefCell<OfflineDatabase>> {
        &self.db
    }

    /// The Network file source, if there is one.
    pub fn online_file_source(&self) -> Option<&Arc<dyn FileSource>> {
        self.online_file_source.as_ref()
    }

    /// The `T` this thread keeps for an extension, created on first use. It
    /// lives on the database thread and is dropped with the database.
    pub fn extension<T: Default + 'static>(&self) -> Rc<T> {
        let existing = self.extensions.borrow().iter().find_map(|extension| Rc::clone(extension).downcast::<T>().ok());
        existing.unwrap_or_else(|| {
            let extension = Rc::new(T::default());
            self.extensions.borrow_mut().push(Rc::<T>::clone(&extension));
            extension
        })
    }
}

/// Turns an optional error into a `Result`.
fn into_result(error: Option<DatabaseError>) -> Result<(), DatabaseError> {
    error.map_or(Ok(()), Err)
}

/// The `Database` file source: the ambient cache and the offline regions,
/// in an SQLite database.
pub struct DatabaseFileSource {
    thread: Thread<DatabaseFileSourceThread>,
}

impl DatabaseFileSource {
    /// Holds the database thread until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(&self.thread.actor())
    }

    /// A database at `cache_path`, created if needed; `":memory:"` keeps it
    /// in memory. Offline regions download through `network`, and can't
    /// without one.
    #[must_use]
    pub fn new(cache_path: &str, network: Option<Arc<dyn FileSource>>) -> Self {
        Self::open(cache_path, network, false)
    }

    /// The database at `cache_path`, opened read-only: it answers requests
    /// but stores nothing.
    #[must_use]
    pub fn read_only(cache_path: &str, network: Option<Arc<dyn FileSource>>) -> Self {
        Self::open(cache_path, network, true)
    }

    fn open(cache_path: &str, network: Option<Arc<dyn FileSource>>, read_only: bool) -> Self {
        let cache_path = cache_path.to_owned();
        let thread = Thread::new("DatabaseFileSource", move |self_ref| {
            DatabaseFileSourceThread::new(self_ref, network, &cache_path, read_only)
        });
        DatabaseFileSource { thread }
    }

    /// Stores a response to `resource`, unless the resource is volatile.
    pub fn put(&self, resource: &Resource, response: &Response) {
        if resource.storage_policy == StoragePolicy::Volatile {
            return;
        }
        let resource = resource.clone();
        let response = response.clone();
        self.invoke(move |t| t.put(&resource, &response));
    }

    /// Switches to the database at `path`, creating it if needed. The
    /// callback runs on the database thread.
    #[cfg(test)]
    pub(crate) fn set_database_path(&self, path: &str, callback: Option<Box<dyn FnOnce() + Send>>) {
        let path = path.to_owned();
        self.thread.actor().invoke(move |t: &DatabaseFileSourceThread| t.set_database_path(&path, callback));
    }

    /// Stores a response to `resource`, then runs `then` on this thread.
    #[cfg(test)]
    pub(crate) fn put_then(&self, resource: &Resource, response: &Response, then: Box<dyn FnOnce() + Send>) {
        let mut then = mln_actor::scheduler::bind_once(&mln_actor::scheduler::get_current(), then);
        let resource = resource.clone();
        let response = response.clone();
        self.invoke(move |t| {
            t.put(&resource, &response);
            then();
        });
    }

    /// Deletes the database file and starts an empty one. The callback runs
    /// on the database thread.
    pub fn reset_database(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.reset_database(callback));
    }

    /// Vacuums the database file to the space its data needs. The callback
    /// runs on the database thread.
    pub fn pack_database(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.pack_database(callback));
    }

    /// Makes the ambient cache's resources revalidate with their server
    /// before use. Offline regions' resources are unaffected. The callback
    /// runs on the database thread.
    pub fn invalidate_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.invalidate_ambient_cache(callback));
    }

    /// Erases the ambient cache's resources. Offline regions' resources are
    /// unaffected. The callback runs on the database thread.
    pub fn clear_ambient_cache(&self, callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static) {
        self.invoke(move |t| t.clear_ambient_cache(callback));
    }

    /// Limits the database to `size` bytes, evicting ambient resources to
    /// fit. Offline regions count towards the limit but are never evicted; 0
    /// disables the ambient cache. The callback runs on the database thread.
    pub fn set_maximum_ambient_cache_size(
        &self,
        size: u64,
        callback: impl FnOnce(Result<(), DatabaseError>) + Send + 'static,
    ) {
        self.invoke(move |t| t.set_maximum_ambient_cache_size(size, callback));
    }

    /// Runs `f` on the database thread, after the requests and operations
    /// sent before it.
    pub fn invoke(&self, f: impl FnOnce(&DatabaseFileSourceThread) + Send + 'static) {
        self.thread.actor().invoke(f);
    }
}

impl FileSource for DatabaseFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &DatabaseFileSourceThread| {
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
