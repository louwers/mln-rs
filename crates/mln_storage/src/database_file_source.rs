// The database runs on a worker thread of its own ("DatabaseFileSource"),
// which owns it: requests and operations reach the worker as messages of a
// channel, and the responses go to the requests' responders.
//
// The offline region downloads need the style parser, which sits above this
// crate: mln_storage_offline keeps them in the worker's extension, which the
// worker owns beside the database, and which sends its own work to the
// worker through a `DatabaseSender`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mln_util::constants::{ASSET_PROTOCOL, FILE_PROTOCOL};
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::file_source::FileSource;
use crate::offline_database::{DatabaseError, OfflineDatabase};
use crate::request::Responder;
use crate::resource::{LoadingMethod, Resource, StoragePolicy};
use crate::response::{Reason, ResourceError, Response};

/// How long cache hits' `accessed` timestamps wait, so that the reads of a
/// map load share one write.
const ACCESSED_FLUSH_DELAY: Duration = Duration::from_millis(250);

/// What runs on a database's worker, in the order it was sent.
type Task<E> = Box<dyn FnOnce(&mut DatabaseWorker<E>) + Send>;

enum Message<E: 'static> {
    Run(Task<E>),
    /// A request for the cached response to `resource`.
    Load {
        resource: Resource,
        responder: Responder,
    },
    /// Ends the worker, after the messages sent before.
    Stop,
}

/// Sends work to a database's worker, which runs it after the work sent
/// before. Work sent once the database has dropped is dropped.
pub struct DatabaseSender<E: 'static> {
    sender: mpsc::Sender<Message<E>>,
}

impl<E: 'static> Clone for DatabaseSender<E> {
    fn clone(&self) -> Self {
        DatabaseSender { sender: self.sender.clone() }
    }
}

impl<E: 'static> DatabaseSender<E> {
    /// Runs `f` on the worker, with what it owns.
    pub fn invoke(&self, f: impl FnOnce(&mut DatabaseWorker<E>) + Send + 'static) {
        let _ = self.sender.send(Message::Run(Box::new(f)));
    }
}

/// What a database's worker owns, on the database's thread: the database,
/// and what an extension keeps beside it, such as the offline regions'
/// downloads.
pub struct DatabaseWorker<E: 'static> {
    pub db: OfflineDatabase,
    pub extension: E,
    network: Option<Arc<dyn FileSource>>,
    /// A cache hit left `accessed` timestamps to write. A failed flush
    /// isn't retried until the next cache hit, so an idle map stays idle.
    accessed: Arc<AtomicBool>,
    /// When the `accessed` timestamps are written.
    flush_due: Option<Instant>,
}

/// Makes the extension of a database worker, on the worker's thread, from a
/// sender to the worker and its network.
type MakeExtension<E> = Box<dyn FnOnce(DatabaseSender<E>, Option<Arc<dyn FileSource>>) -> E + Send>;

impl<E: 'static> DatabaseWorker<E> {
    fn new(
        sender: DatabaseSender<E>,
        network: Option<Arc<dyn FileSource>>,
        cache_path: &str,
        read_only: bool,
        make: MakeExtension<E>,
    ) -> Self {
        let mut db = OfflineDatabase::new(cache_path);
        if read_only {
            db.reopen_database_read_only(true);
        }
        let accessed = Arc::new(AtomicBool::new(false));
        let hit = Arc::clone(&accessed);
        db.set_accessed_callback(Box::new(move || hit.store(true, Ordering::Relaxed)));
        let extension = make(sender, network.clone());
        DatabaseWorker { db, extension, network, accessed, flush_due: None }
    }

    /// Runs the worker's messages, and writes the `accessed` timestamps once
    /// they are due, until the database drops.
    fn run(mut self, receiver: &mpsc::Receiver<Message<E>>) {
        loop {
            let message = match self.flush_due {
                Some(due) => match receiver.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match receiver.recv() {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvError) => return,
                },
            };
            match message {
                Some(Message::Run(task)) => task(&mut self),
                Some(Message::Load { resource, responder }) => {
                    if !responder.is_cancelled() {
                        self.request(&resource, &responder);
                    }
                }
                Some(Message::Stop) => return,
                None => {}
            }
            if self.flush_due.is_some_and(|due| due <= Instant::now()) {
                self.flush_due = None;
                self.accessed.store(false, Ordering::Relaxed);
                self.db.flush_accessed();
            }
            if self.flush_due.is_none() && self.accessed.load(Ordering::Relaxed) {
                self.flush_due = Some(Instant::now() + ACCESSED_FLUSH_DELAY);
            }
        }
    }

    /// The network that offline regions download through, if there is one.
    pub fn network(&self) -> Option<&Arc<dyn FileSource>> {
        self.network.as_ref()
    }

    fn request(&mut self, resource: &Resource, responder: &Responder) {
        let offline_response =
            if resource.storage_policy == StoragePolicy::Volatile { None } else { self.db.get(resource) };
        let response = match offline_response {
            None => {
                let mut response = Response::new();
                response.no_content = true;
                response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Not found in offline database")));
                response
            }
            Some(mut response) => {
                if !response.is_usable() {
                    response.error =
                        Some(Box::new(ResourceError::new(Reason::NotFound, "Cached resource is unusable")));
                }
                response
            }
        };
        responder.send(response);
    }

    fn maintain(
        &mut self,
        operation: impl FnOnce(&mut OfflineDatabase) -> Option<DatabaseError>,
        callback: impl FnOnce(Result<(), DatabaseError>),
    ) {
        let error = operation(&mut self.db);
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
/// extension `E` beside it. Its drop waits for the worker to end, unless it
/// drops on the worker's thread. When the worker's thread can't start, the
/// source logs why, fails each request, and drops the work sent to it.
pub struct DatabaseFileSource<E: 'static = ()> {
    sender: DatabaseSender<E>,
    thread: Option<std::thread::JoinHandle<()>>,
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
    /// extension that `make` makes on the worker's thread, from a sender to
    /// the worker and the database's network.
    #[must_use]
    pub fn with_extension(
        cache_path: &str,
        network: Option<Arc<dyn FileSource>>,
        read_only: bool,
        make: impl FnOnce(DatabaseSender<E>, Option<Arc<dyn FileSource>>) -> E + Send + 'static,
    ) -> Self {
        let cache_path = cache_path.to_owned();
        let make: MakeExtension<E> = Box::new(make);
        let (sender, receiver) = mpsc::channel();
        let sender = DatabaseSender { sender };
        let own = sender.clone();
        let thread = mln_util::thread::spawn("DatabaseFileSource", move || {
            DatabaseWorker::new(own, network, &cache_path, read_only, make).run(&receiver);
        })
        .inspect_err(|error| Log::error(Event::Database, format!("The database's thread didn't start: {error}")))
        .ok();
        DatabaseFileSource { sender, thread }
    }

    /// Holds the database thread until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(|hold| self.invoke(move |_| hold()))
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
        self.sender.invoke(f);
    }
}

impl<E: 'static> Drop for DatabaseFileSource<E> {
    fn drop(&mut self) {
        let _ = self.sender.sender.send(Message::Stop);
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
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
        let load = Message::Load { resource: resource.clone(), responder };
        if let Err(mpsc::SendError(Message::Load { responder, .. })) = self.sender.sender.send(load) {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "The database's thread isn't running.")));
            responder.send(response);
        }
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(LoadingMethod::Cache)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}

#[cfg(test)]
mod tests {
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
        let sender = database.sender.clone();
        let (dropped, done) = mpsc::channel();
        sender.invoke(move |_| {
            drop(database);
            dropped.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(10)).expect("the drop returns");
    }
}
