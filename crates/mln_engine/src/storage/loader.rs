use std::sync::Arc;

use parking_lot::Mutex;

use crate::exec::pool::Pool;
use crate::storage::asset_file_source::AssetFileSource;
#[cfg(feature = "sqlite")]
use crate::storage::chrono::now;
#[cfg(feature = "sqlite")]
use crate::storage::database_file_source::Cache;
#[cfg(all(feature = "sqlite", not(target_family = "wasm")))]
use crate::storage::database_file_source::DatabaseFileSource;
use crate::storage::file_source::FileSource;
use crate::storage::local_file_source::LocalFileSource;
#[cfg(feature = "sqlite")]
use crate::storage::mbtiles_file_source::MBTilesFileSource;
#[cfg(not(target_family = "wasm"))]
use crate::storage::network_status::NetworkStatus;
#[cfg(not(target_family = "wasm"))]
use crate::storage::online_file_source::OnlineFileSource;
use crate::storage::pmtiles_file_source::PMTilesFileSource;
use crate::storage::request::{Request, Responder};
#[cfg(feature = "sqlite")]
use crate::storage::resource::Priority;
use crate::storage::resource::{LoadingMethod, Resource};
#[cfg(not(target_family = "wasm"))]
use crate::storage::resource_options::ResourceOptions;
use crate::storage::response::{Reason, ResourceError, Response};

/// The file sources a [`Loader`] picks from.
#[derive(Clone)]
struct Sources {
    asset: Arc<dyn FileSource>,
    local: Arc<dyn FileSource>,
    #[cfg(feature = "sqlite")]
    mbtiles: Arc<dyn FileSource>,
    pmtiles: Option<Arc<dyn FileSource>>,
    #[cfg(feature = "sqlite")]
    database: Option<Arc<dyn Cache>>,
    network: Option<Arc<dyn FileSource>>,
}

/// A request's sub-request in flight, which goes when the request is
/// cancelled.
type Task = Arc<Mutex<Option<Request>>>;

impl Sources {
    /// The first of the local file sources that loads `resource`.
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
    fn database_for(&self, resource: &Resource) -> Option<&Arc<dyn Cache>> {
        self.database.as_ref().filter(|database| database.can_request(resource))
    }

    #[cfg(not(feature = "sqlite"))]
    #[expect(clippy::unused_self, reason = "the build without SQLite has no database")]
    fn database_for(&self, _resource: &Resource) -> Option<&Arc<dyn FileSource>> {
        None
    }

    fn network_for(&self, resource: &Resource) -> Option<&Arc<dyn FileSource>> {
        self.network.as_ref().filter(|network| network.can_request(resource))
    }

    #[cfg(feature = "sqlite")]
    fn has_database(&self) -> bool {
        self.database.is_some()
    }

    #[cfg(not(feature = "sqlite"))]
    #[expect(clippy::unused_self, reason = "the build without SQLite has no database")]
    fn has_database(&self) -> bool {
        false
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

    /// Requests `res` from the network for `resource`'s requester, in place
    /// of `task`'s request before, and stores the responses in the database.
    /// Without a network file source that loads `resource`, `task`'s request
    /// stays.
    ///
    /// The callbacks that the file sources hold reach the sources weakly:
    /// the network and the database refer to each other's file sources, and
    /// a strong reference would leave the last of them to drop on its own
    /// thread.
    fn request_from_network(self: &Arc<Self>, resource: &Resource, res: &Resource, task: &Task, responder: &Responder) {
        let Some(network) = self.network_for(resource) else {
            return;
        };
        if !self.has_database() {
            let previous = task.lock().take();
            drop(previous);
            network.load(res, responder.clone());
            return;
        }
        let (sources, stored, responder) = (Arc::downgrade(self), res.clone(), responder.clone());
        let (request, network_responder) = Request::new(move |response: Response| {
            if let Some(sources) = sources.upgrade() {
                sources.store(&stored, &response);
            }
            responder.send(response);
        });
        let previous = task.lock().replace(request);
        drop(previous);
        network.load(res, network_responder);
    }

    /// Loads `resource` from the cache, then from the network. A usable
    /// cached copy goes to the requester at once, and the network revalidates
    /// it at low priority once it is stale. A copy that must revalidate first
    /// waits for the network, and the requester gets it on a 304
    /// (`Resource::deliver`).
    #[cfg(feature = "sqlite")]
    fn request_cached(self: &Arc<Self>, database: &dyn Cache, resource: &Resource, responder: Responder) {
        let task: Task = Arc::default();
        let cancelled = Arc::clone(&task);
        responder.on_cancel(move || drop(cancelled.lock().take()));

        let (sources, resource_copy, chain) = (Arc::downgrade(self), resource.clone(), Arc::clone(&task));
        let (request, database_responder) = Request::new(move |response: Response| {
            let Some(sources) = sources.upgrade() else { return };
            let mut res = resource_copy.clone();

            // A cached copy.
            if response.error.is_none() && !response.no_content {
                res.set_prior(&response);
                if response.caching.is_usable(now()) {
                    responder.send(response.clone());
                    res.set_priority(Priority::Low);
                } else {
                    res.prior_data = response.data.clone();
                }
            }

            sources.request_from_network(&resource_copy, &res, &chain, &responder);
        });
        *task.lock() = Some(request);
        database.load(resource, database_responder);
    }
}

/// The resource loader: one file source over the asset, file, MBTiles and
/// PMTiles file sources, and the cache database and network, if it has them.
///
/// A request goes to the first local file source that loads its URL.
/// Failing that, it goes to the database, and on to the network unless it's
/// cache-only: the network revalidates a cached copy as its caching rules
/// say, and the database stores what the network answers. A cache-only
/// request gets the stored copy whatever its freshness; its caching rules
/// say how it may be used.
pub struct Loader {
    sources: Arc<Sources>,
}

impl Loader {
    /// A loader for `options`: assets from its asset path, a cache database
    /// at its cache path (when the `sqlite` feature is on), and an
    /// [`OnlineFileSource`] for the network, with a [`NetworkStatus`] of its
    /// own, which stays online. It reads files and archives on `pool`.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn new(options: &ResourceOptions, pool: &Pool) -> Loader {
        let network: Arc<dyn FileSource> = Arc::new(OnlineFileSource::new(Arc::new(NetworkStatus::new())));
        let builder = Loader::builder(pool).asset_path(&options.asset_path).network(Arc::clone(&network));
        #[cfg(feature = "sqlite")]
        let builder = builder.database(Arc::new(DatabaseFileSource::new(&options.cache_path, Some(network))));
        builder.build()
    }

    /// A builder for a loader with neither a database nor a network, which
    /// reads files and archives on `pool`.
    pub fn builder(pool: &Pool) -> LoaderBuilder {
        LoaderBuilder {
            asset_path: ".".to_owned(),
            #[cfg(feature = "sqlite")]
            database: None,
            network: None,
            pool: pool.clone(),
        }
    }
}

/// Builds a [`Loader`] from the file sources it's given.
#[must_use]
pub struct LoaderBuilder {
    asset_path: String,
    #[cfg(feature = "sqlite")]
    database: Option<Arc<dyn Cache>>,
    network: Option<Arc<dyn FileSource>>,
    pool: Pool,
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
    pub fn database(mut self, database: Arc<dyn Cache>) -> Self {
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
        let pool = self.pool;
        let mut sources = Sources {
            asset: Arc::new(AssetFileSource::new(&self.asset_path, pool.clone())),
            local: Arc::new(LocalFileSource::new(pool.clone())),
            #[cfg(feature = "sqlite")]
            mbtiles: Arc::new(MBTilesFileSource::new(pool.clone())),
            pmtiles: None,
            #[cfg(feature = "sqlite")]
            database: self.database,
            network: self.network,
        };
        // PMTiles archives load through the same file sources, but a PMTiles
        // archive inside another isn't one.
        let archives: Arc<dyn FileSource> = Arc::new(Loader { sources: Arc::new(sources.clone()) });
        sources.pmtiles = Some(Arc::new(PMTilesFileSource::new(archives, pool)));
        Loader { sources: Arc::new(sources) }
    }
}

impl FileSource for Loader {
    fn supports_cache_only(&self) -> bool {
        self.sources.has_database()
    }

    fn load(&self, resource: &Resource, responder: Responder) {
        let sources = &self.sources;
        if let Some(local) = sources.local_for(resource) {
            local.load(resource, responder);
        } else if let Some(database) = sources.database_for(resource) {
            if resource.loading_method == LoadingMethod::CacheOnly {
                database.load(resource, responder);
            } else {
                #[cfg(feature = "sqlite")]
                sources.request_cached(&**database, resource, responder);
            }
        } else if sources.network_for(resource).is_some() {
            let task: Task = Arc::default();
            let cancelled = Arc::clone(&task);
            responder.on_cancel(move || drop(cancelled.lock().take()));
            sources.request_from_network(resource, resource, &task, &responder);
        } else {
            // No file source loads the resource.
            let mut response = Response::new();
            response.no_content = true;
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "Unsupported resource request.")));
            responder.send(response);
        }
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.sources.local_for(resource).is_some()
            || self.sources.database_for(resource).is_some()
            || self.sources.network_for(resource).is_some()
    }
}
