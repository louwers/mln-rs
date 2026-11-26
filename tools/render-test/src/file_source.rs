use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mln_engine::exec::pool::Pool;
use mln_engine::storage::constants::ASSET_PROTOCOL;
use mln_engine::storage::network_status::NetworkStatus;
use mln_engine::storage::{
    DatabaseFileSource, FileSource, Kind, Loader, LoadingMethod, OnlineFileSource, Request, Resource, ResourceOptions,
    Responder, Response,
};
use parking_lot::Mutex;

static REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
static TRANSFERRED_SIZE: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static OFFLINE: AtomicBool = AtomicBool::new(true);

/// Port-only: the requests passed on and neither answered nor cancelled yet,
/// by request number: each URL and when it was requested. A test that times
/// out reports them (watchdog.rs), so a hang says what it waited for.
static PENDING: Mutex<BTreeMap<u64, (String, Instant)>> = Mutex::new(BTreeMap::new());
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

/// Adds a request for `url` to `PENDING` and returns its number.
pub(crate) fn track(url: &str) -> u64 {
    let number = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
    PENDING.lock().insert(number, (url.to_string(), Instant::now()));
    number
}

/// Removes request `number` from `PENDING`, if it is there.
pub(crate) fn answered(number: u64) {
    PENDING.lock().remove(&number);
}

/// A file source that loads everything cache-only from the database while
/// offline and maps the test URLs (`local://`, `http://localhost:2900/`) to
/// the `http://` URLs the cache stores.
///
/// Port-only: a test URL whose path is a file under `assets` loads that file
/// instead, so a test can bring its own tiles and images without rows in the
/// cache database.
pub(crate) struct ProxyFileSource {
    pub(crate) default_resource_loader: Arc<dyn FileSource>,
    pub(crate) assets: Option<AssetDir>,
}

/// The directory of files that load in place of the cache's resources.
pub(crate) struct AssetDir {
    pub(crate) path: PathBuf,
    /// The directories `path` holds load only from it: a file missing there
    /// is not found, and never the cache's.
    pub(crate) only: bool,
}

impl AssetDir {
    /// Whether the test URL path `path` loads from the assets.
    fn serves(&self, path: &str) -> bool {
        if self.path.join(path).is_file() {
            return true;
        }
        self.only && path.split_once('/').is_some_and(|(directory, _)| self.path.join(directory).is_dir())
    }
}

/// The files that load in place of the cache's resources.
#[derive(Clone, Copy)]
pub struct Assets<'a> {
    /// The directory of the files.
    pub path: &'a str,
    /// The test is in the assets' suite: the directories `path` holds load
    /// only from it, as on the test server of that suite.
    pub only: bool,
}

impl ProxyFileSource {
    /// Flag to change the networking mode of the file source.
    pub(crate) fn set_offline(status: bool) {
        OFFLINE.store(status, Ordering::SeqCst);
    }

    /// Starts/stops metrics tracking.
    pub(crate) fn set_tracking_active(active: bool) {
        ACTIVE.store(active, Ordering::SeqCst);
        REQUEST_COUNT.store(0, Ordering::SeqCst);
        TRANSFERRED_SIZE.store(0, Ordering::SeqCst);
    }

    /// Returns metrics tracking status.
    pub(crate) fn is_tracking_active() -> bool {
        ACTIVE.load(Ordering::SeqCst)
    }

    /// Returns the total amount of requests.
    pub(crate) fn get_request_count() -> usize {
        REQUEST_COUNT.load(Ordering::SeqCst)
    }

    /// Returns the size of transferred data (in bytes).
    pub(crate) fn get_transferred_size() -> usize {
        TRANSFERRED_SIZE.load(Ordering::SeqCst)
    }

    /// Port-only: the URLs of the requests that are neither answered nor
    /// cancelled, oldest first, with how long ago each was made.
    pub(crate) fn pending_requests() -> Vec<(String, Duration)> {
        let now = Instant::now();
        PENDING.lock().values().map(|(url, since)| (url.clone(), now - *since)).collect()
    }
}

/// Replaces `prefix` at the start of `s` with `http://`, and returns whether
/// `s` started with it.
fn replace_prefix(s: &mut String, prefix: &str) -> bool {
    if s.starts_with(prefix) {
        s.replace_range(..prefix.len(), "http://");
        true
    } else {
        false
    }
}

/// The part of a test URL after its scheme and host: the path of the asset
/// it names.
fn test_url_path(url: &str) -> Option<&str> {
    url.strip_prefix("local://").or_else(|| url.strip_prefix("http://localhost:2900/"))
}

impl FileSource for ProxyFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let mut transformed = resource.clone();

        if let Some(assets) = &self.assets
            && let Some(path) = test_url_path(&resource.url)
            && assets.serves(path)
        {
            transformed.url = format!("{ASSET_PROTOCOL}{path}");
        }

        // If offline, force always loading the resource from the cache so we
        // don't make any network request.
        if OFFLINE.load(Ordering::SeqCst) {
            transformed.loading_method = LoadingMethod::CacheOnly;
        }

        // This is needed for compatibility with the style tests that are
        // using local:// instead of http:// which is the schema we support
        // for cached files.
        if replace_prefix(&mut transformed.url, "local://")
            && transformed.kind == Kind::Tile
            && let Some(tile_data) = &mut transformed.tile_data
        {
            // C++ replaces the first 8 characters unconditionally.
            tile_data.url_template.replace_range(..8.min(tile_data.url_template.len()), "http://");
        }

        if replace_prefix(&mut transformed.url, "http://localhost:2900/")
            && transformed.kind == Kind::Tile
            && let Some(tile_data) = &mut transformed.tile_data
        {
            // C++ replaces the first 22 characters unconditionally.
            tile_data.url_template.replace_range(..22.min(tile_data.url_template.len()), "http://");
        }

        let number = track(&transformed.url);
        let logged = transformed.clone();
        let outer = responder.clone();
        let (request, inner) = Request::new(move |response: Response| {
            answered(number);
            if logged.loading_method == LoadingMethod::CacheOnly && response.no_content {
                match (&logged.tile_data, logged.kind) {
                    (Some(tile_data), Kind::Tile) => {
                        log::info!("Resource not found in cache: {}({})", logged.url, tile_data.url_template);
                    }
                    _ => log::info!("Resource not found in cache: {}", logged.url),
                }
            }

            let size = response.data.as_ref().map_or(0, |data| data.len());
            if ACTIVE.load(Ordering::SeqCst) {
                REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
                TRANSFERRED_SIZE.fetch_add(size, Ordering::SeqCst);
            }
            outer.send(response);
        });
        // A request that is cancelled leaves `PENDING` too.
        responder.on_cancel(move || {
            answered(number);
            drop(request);
        });
        self.default_resource_loader.load(&transformed, inner);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

/// A `ProxyFileSource` over a loader for `resource_options`, whose cache
/// database is read-only while offline (`TestRunner::registerProxyFileSource()`).
/// `assets` is the files that load in place of the cache's resources, if any.
#[must_use]
pub fn proxy_loader(
    resource_options: &ResourceOptions,
    assets: Option<Assets<'_>>,
    pool: &Pool,
) -> Arc<dyn FileSource> {
    let network: Arc<dyn FileSource> = Arc::new(OnlineFileSource::new(Arc::new(NetworkStatus::new())));
    let cache_path = &resource_options.cache_path;
    let database = if OFFLINE.load(Ordering::SeqCst) {
        DatabaseFileSource::read_only(cache_path, Some(Arc::clone(&network)))
    } else {
        DatabaseFileSource::new(cache_path, Some(Arc::clone(&network)))
    };
    let loader = Loader::builder(pool)
        .asset_path(assets.map_or(resource_options.asset_path.as_str(), |assets| assets.path))
        .network(network)
        .database(Arc::new(database))
        .build();
    Arc::new(ProxyFileSource {
        default_resource_loader: Arc::new(loader),
        assets: assets.map(|assets| AssetDir { path: PathBuf::from(assets.path), only: assets.only }),
    })
}
