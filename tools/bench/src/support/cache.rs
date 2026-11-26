//! The map's disk cache, as an application has one: the database file
//! source, in memory, which a scenario waits for after each request so that
//! its answers come at the same step on every run.

use std::sync::{Arc, mpsc};

use mln_engine::storage::{Cache, DatabaseFileSource, FileSource, Resource, Responder, Response};

/// A database file source in memory. It runs on a thread of its own, as an
/// application's does, and each load and store returns once the database
/// has done it, its answer's callbacks included, which run on its thread:
/// how much work waits for the database's thread when it wakes then never
/// changes what it does.
pub(crate) struct DiskCache {
    database: DatabaseFileSource,
}

impl DiskCache {
    /// An empty cache.
    pub(crate) fn new() -> Arc<DiskCache> {
        Arc::new(DiskCache { database: DatabaseFileSource::new(":memory:", None) })
    }

    /// Waits until the database has done what it was sent.
    fn wait(&self) {
        let (done, wait) = mpsc::channel();
        // The database runs what it's sent in order: this runs after it.
        self.database.invoke(move |_| {
            let _ = done.send(());
        });
        wait.recv().expect("the database answers");
    }
}

impl FileSource for DiskCache {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.database.load(resource, responder);
        self.wait();
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.database.can_request(resource)
    }

    fn supports_cache_only(&self) -> bool {
        self.database.supports_cache_only()
    }
}

impl Cache for DiskCache {
    fn put(&self, resource: &Resource, response: &Response) {
        self.database.put(resource, response);
        self.wait();
    }
}
