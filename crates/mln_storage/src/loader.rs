// The waterfall runs on the "ResourceLoaderThread". `tasks` is keyed by a
// request id. Callbacks of the chained requests reach the thread object
// through a `Weak`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mln_actor::{ActorRef, Thread};
use mln_vec_map::VecMap;

use crate::asset_file_source::AssetFileSource;
use crate::async_request::AsyncRequest;
#[cfg(feature = "sqlite")]
use crate::database_file_source::DatabaseFileSource;
use crate::file_source::{Callback, FileSource};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::local_file_source::LocalFileSource;
#[cfg(feature = "sqlite")]
use crate::mbtiles_file_source::MBTilesFileSource;
use crate::online_file_source::OnlineFileSource;
use crate::pmtiles_file_source::PMTilesFileSource;
use crate::resource::{LoadingMethod, Priority, Resource};
use crate::resource_options::ResourceOptions;
use crate::response::{Reason, ResourceError, Response};

/// The file sources a [`Loader`] picks from.
#[derive(Clone)]
struct Sources {
    asset: Arc<dyn FileSource>,
    local: Arc<dyn FileSource>,
    #[cfg(feature = "sqlite")]
    mbtiles: Arc<dyn FileSource>,
    pmtiles: Option<Arc<dyn FileSource>>,
    #[cfg(feature = "sqlite")]
    database: Option<Arc<DatabaseFileSource>>,
    network: Option<Arc<dyn FileSource>>,
}

impl Sources {
    /// The file sources in the order the loader tries them, that can load
    /// `resource`. The database and the network come last, in that order.
    fn local_for(&self, resource: &Resource) -> Option<&Arc<dyn FileSource>> {
        #[cfg(feature = "sqlite")]
        let mbtiles = Some(&self.mbtiles);
        #[cfg(not(feature = "sqlite"))]
        let mbtiles = None;
        [Some(&self.asset), mbtiles, self.pmtiles.as_ref(), Some(&self.local)]
            .into_iter()
            .flatten()
            .find(|source| source.can_request(resource))
    }

    #[cfg(feature = "sqlite")]
    fn database_for(&self, resource: &Resource) -> Option<&Arc<DatabaseFileSource>> {
        self.database.as_ref().filter(|database| database.can_request(resource))
    }

    #[cfg(not(feature = "sqlite"))]
    #[expect(clippy::unused_self, reason = "the build without SQLite has no database")]
    fn database_for(&self, _resource: &Resource) -> Option<&Arc<dyn FileSource>> {
        None
    }

    /// Stores a response from the network in the database.
    #[cfg(feature = "sqlite")]
    fn store(&self, resource: &Resource, response: &Response) {
        if let Some(database) = &self.database {
            database.put(resource, response);
        }
    }

    #[cfg(not(feature = "sqlite"))]
    #[expect(clippy::unused_self, reason = "the build without SQLite has no database")]
    fn store(&self, _resource: &Resource, _response: &Response) {}
}

struct ThreadInner {
    sources: Sources,
    tasks: RefCell<VecMap<u64, Box<dyn AsyncRequest>>>,
}

pub(crate) struct LoaderThread {
    inner: Rc<ThreadInner>,
}

fn make_callback(r: &ActorRef<FileSourceRequestState>) -> impl Fn(Response) + Clone + 'static {
    let r = r.clone();
    move |res: Response| {
        r.invoke(move |request: &FileSourceRequestState| request.set_response(res));
    }
}

impl ThreadInner {
    /// Requests `res` from the network, keeping `parent` alive meanwhile,
    /// and stores the response in the database. Without a network file
    /// source that loads `resource`, it returns `parent`.
    fn request_from_network(
        self: &Rc<Self>,
        resource: &Resource,
        res: &Resource,
        parent: Option<Box<dyn AsyncRequest>>,
        callback: impl Fn(Response) + 'static,
    ) -> Option<Box<dyn AsyncRequest>> {
        let network = match &self.sources.network {
            Some(network) if network.can_request(resource) => Arc::clone(network),
            _ => return parent,
        };

        // Keep parent request alive while chained request is being processed.
        let parent_keep_alive: Rc<Option<Box<dyn AsyncRequest>>> = Rc::new(parent);

        let weak = Rc::downgrade(self);
        let forwarded = res.clone();
        Some(network.request(
            res,
            Box::new(move |response: Response| {
                let _ptr = &parent_keep_alive;
                if let Some(this) = weak.upgrade() {
                    this.sources.store(&forwarded, &response);
                }
                callback(response);
            }),
        ))
    }
}

impl LoaderThread {
    pub(crate) fn request(&self, req: u64, resource: &Resource, r: &ActorRef<FileSourceRequestState>) {
        let inner = &self.inner;
        let callback = make_callback(r);

        // Waterfall resource request processing and return early once
        // resource was requested.
        let task: Option<Box<dyn AsyncRequest>> = if let Some(local) = inner.sources.local_for(resource) {
            Some(local.request(resource, Box::new(callback.clone())))
        } else if let Some(database) = inner.sources.database_for(resource) {
            // Try cache only request if needed.
            if resource.loading_method == LoadingMethod::CacheOnly {
                Some(database.request(resource, Box::new(callback.clone())))
            } else {
                // Cache request with fallback to network with cache control
                let weak: Weak<ThreadInner> = Rc::downgrade(inner);
                let resource_copy = resource.clone();
                let callback = callback.clone();
                Some(database.request(
                    resource,
                    Box::new(move |response: Response| {
                        let Some(this) = weak.upgrade() else {
                            return;
                        };
                        let mut res = resource_copy.clone();

                        // Resource is in the cache
                        if !response.no_content {
                            if response.is_usable() {
                                callback(response.clone());
                                // Set the priority of existing resource to
                                // low if it's expired but usable.
                                res.set_priority(Priority::Low);
                            } else {
                                // Set prior data only if it was not returned
                                // to the requester. Once we get 304 response
                                // from the network, we will forward response
                                // to the requester.
                                res.prior_data = response.data.clone();
                            }

                            // Copy response fields for cache control request
                            res.prior_modified = response.modified;
                            res.prior_expires = response.expires;
                            res.prior_etag = response.etag;
                        }

                        let parent = this.tasks.borrow_mut().remove(&req);
                        let task = this.request_from_network(&resource_copy, &res, parent, callback.clone());
                        if let Some(task) = task {
                            this.tasks.borrow_mut().insert(req, task);
                        }
                    }),
                ))
            }
        } else {
            // Get from the online file source
            inner.request_from_network(resource, resource, None, callback.clone())
        };

        if let Some(task) = task {
            inner.tasks.borrow_mut().insert(req, task);
        } else {
            // If no new tasks were added, notify client that request
            // cannot be processed.
            let mut response = Response::new();
            response.no_content = true;
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "Unsupported resource request.")));
            callback(response);
        }
    }

    pub(crate) fn cancel(&self, req: u64) {
        let task = self.inner.tasks.borrow_mut().remove(&req);
        drop(task);
    }
}

/// The resource loader: one file source over the asset, file, MBTiles and
/// PMTiles file sources, and the cache database and network, if it has them.
///
/// A request goes to the first local file source that loads its URL.
/// Failing that, it goes to the database, and on to the network unless it's
/// cache-only: the network revalidates a cached response that has expired,
/// and the database stores what the network answers.
pub struct Loader {
    /// Dropped (joined) before the file sources.
    thread: Thread<LoaderThread>,
    sources: Sources,
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

impl Loader {
    /// A loader for `options`: assets from its asset path, a cache database
    /// at its cache path (when the `sqlite` feature is on), and an
    /// [`OnlineFileSource`] for the network.
    #[must_use]
    pub fn new(options: &ResourceOptions) -> Loader {
        let network: Arc<dyn FileSource> = Arc::new(OnlineFileSource::new());
        let builder = Loader::builder().asset_path(&options.asset_path).network(Arc::clone(&network));
        #[cfg(feature = "sqlite")]
        let builder = builder.database(Arc::new(DatabaseFileSource::new(&options.cache_path, Some(network))));
        builder.build()
    }

    /// A builder for a loader with neither a database nor a network.
    pub fn builder() -> LoaderBuilder {
        LoaderBuilder {
            asset_path: ".".to_owned(),
            #[cfg(feature = "sqlite")]
            database: None,
            network: None,
        }
    }

    fn with_sources(sources: Sources) -> Loader {
        let thread_sources = sources.clone();
        let thread = Thread::new("ResourceLoaderThread", move |_| LoaderThread {
            inner: Rc::new(ThreadInner { sources: thread_sources, tasks: RefCell::new(VecMap::new()) }),
        });
        Loader { thread, sources }
    }

    /// The cache database, if the loader has one.
    #[cfg(feature = "sqlite")]
    #[must_use]
    pub fn database(&self) -> Option<&Arc<DatabaseFileSource>> {
        self.sources.database.as_ref()
    }

    /// Holds the loader's thread until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(&self.thread.actor())
    }
}

/// Builds a [`Loader`] from the file sources it's given.
#[must_use]
pub struct LoaderBuilder {
    asset_path: String,
    #[cfg(feature = "sqlite")]
    database: Option<Arc<DatabaseFileSource>>,
    network: Option<Arc<dyn FileSource>>,
}

impl LoaderBuilder {
    /// Loads `asset://` URLs from the directory `path`, rather than the
    /// working directory.
    pub fn asset_path(mut self, path: &str) -> Self {
        path.clone_into(&mut self.asset_path);
        self
    }

    /// Caches responses in `database`, and answers cache-only requests from
    /// it.
    #[cfg(feature = "sqlite")]
    pub fn database(mut self, database: Arc<DatabaseFileSource>) -> Self {
        self.database = Some(database);
        self
    }

    /// Loads the requests that no local file source loads through `network`.
    pub fn network(mut self, network: Arc<dyn FileSource>) -> Self {
        self.network = Some(network);
        self
    }

    #[must_use]
    pub fn build(self) -> Loader {
        let mut sources = Sources {
            asset: Arc::new(AssetFileSource::new(&self.asset_path)),
            local: Arc::new(LocalFileSource::new()),
            #[cfg(feature = "sqlite")]
            mbtiles: Arc::new(MBTilesFileSource::new()),
            pmtiles: None,
            #[cfg(feature = "sqlite")]
            database: self.database,
            network: self.network,
        };
        // PMTiles archives load through the same file sources, but a PMTiles
        // archive inside another isn't one.
        let archives: Arc<dyn FileSource> = Arc::new(Loader::with_sources(sources.clone()));
        sources.pmtiles = Some(Arc::new(PMTilesFileSource::new(archives)));
        Loader::with_sources(sources)
    }
}

impl FileSource for Loader {
    fn supports_cache_only(&self) -> bool {
        #[cfg(feature = "sqlite")]
        return self.sources.database.is_some();
        #[cfg(not(feature = "sqlite"))]
        false
    }

    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let mut req = FileSourceRequest::new(callback);
        let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);

        let actor_ref = self.thread.actor();
        req.on_cancel(Box::new(move || {
            actor_ref.invoke(move |t: &LoaderThread| t.cancel(req_id));
        }));
        let req_ref = req.actor();
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &LoaderThread| t.request(req_id, &resource, &req_ref));
        Box::new(req)
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.sources.local_for(resource).is_some()
            || self.sources.database_for(resource).is_some()
            || self.sources.network.as_ref().is_some_and(|network| network.can_request(resource))
    }
}
