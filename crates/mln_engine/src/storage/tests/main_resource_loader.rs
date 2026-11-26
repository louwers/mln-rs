// The loader's network is an online file source over the simulated network
// (`super::network`), whose server answers as the C++ tests' server does
// (`routes`), with real status codes and headers.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use mln_util::chrono::{Timestamp, now, rfc1123};

use super::network::{Network, Reply};
use super::{LocalRequests, TestLoop, req, temp_path};
use crate::storage::database_file_source::{Cache, DatabaseFileSource};
use crate::storage::file_source::FileSource;
use crate::storage::loader::Loader;
use crate::storage::online_file_source::OnlineFileSource;
use crate::storage::resource::{Kind, LoadingMethod, Resource, StoragePolicy};
use crate::storage::resource_options::ResourceOptions;
use crate::storage::resource_transform::FinishedCallback;
use crate::storage::response::{Caching, Reason, Response};

/// The URL of `path` on the simulated server.
fn url(path: &str) -> String {
    format!("http://example.invalid{path}")
}

/// 2015-01-01 00:00:00 UTC, when `/revalidate-modified` last changed.
const JAN_1_2015: Timestamp = Timestamp::from_secs(1_420_070_400);

/// The routes of the C++ tests' server that these tests load, by path:
/// - `/test`: "Hello World!", without caching headers;
/// - `/load/<n>`: "Request <n>";
/// - `/cache`: "Response <n>" for the n-th request, fresh for 30 seconds;
/// - `/revalidate-same`: "Response" with an `ETag` that must be revalidated,
///   and a 304 to a revalidation of that `ETag`, fresh for a minute and
///   then to be revalidated (C++: a second);
/// - `/revalidate-modified`: the same with `Last-Modified`;
/// - `/revalidate-etag`: "Response <n>" and a new `ETag` each time, which
///   must be revalidated.
fn routes() -> impl Fn(&Resource) -> Reply + Send + Sync + 'static {
    let (cached, etagged) = (AtomicU32::new(0), AtomicU32::new(0));
    move |resource| {
        let path = resource.url.strip_prefix("http://example.invalid").unwrap_or_default();
        let revalidated = || Reply::status(304).header("Cache-Control", "max-age=60, must-revalidate");
        match path {
            "/test" => Reply::ok("Hello World!"),
            "/cache" => {
                let n = cached.fetch_add(1, Ordering::SeqCst) + 1;
                Reply::ok(&format!("Response {n}")).header("Cache-Control", "max-age=30")
            }
            "/revalidate-same" if resource.prior_etag.as_deref() == Some("snowfall") => revalidated(),
            "/revalidate-same" => {
                Reply::ok("Response").header("ETag", "snowfall").header("Cache-Control", "must-revalidate")
            }
            // A request sends `If-Modified-Since` only when it has no ETag.
            "/revalidate-modified"
                if resource.prior_etag.is_none() && resource.prior_modified.is_some_and(|m| m >= JAN_1_2015) =>
            {
                revalidated()
            }
            // Without `no-cache`, `Last-Modified` would give it a heuristic
            // lifetime.
            "/revalidate-modified" => {
                Reply::ok("Response").header("Last-Modified", rfc1123(JAN_1_2015)).header("Cache-Control", "no-cache")
            }
            "/revalidate-etag" => {
                let n = etagged.fetch_add(1, Ordering::SeqCst) + 1;
                Reply::ok(&format!("Response {n}"))
                    .header("ETag", format!("response-{n}"))
                    .header("Cache-Control", "must-revalidate")
            }
            _ => match path.strip_prefix("/load/") {
                Some(n) => Reply::ok(&format!("Request {n}")),
                None => Reply::status(404),
            },
        }
    }
}

/// A loader over an in-memory database and a simulated network, and those
/// two.
struct TestSources {
    loader: Rc<Loader>,
    database: Arc<DatabaseFileSource>,
    network: Network,
}

impl TestSources {
    /// Sources whose server answers by [`routes`].
    fn new() -> Self {
        Self::serving(Network::serving(routes()))
    }

    fn serving(network: Network) -> Self {
        let online: Arc<dyn FileSource> = Arc::<OnlineFileSource>::clone(&network.source);
        let database = Arc::new(DatabaseFileSource::new(":memory:", Some(Arc::clone(&online))));
        let loader = Loader::builder(&super::pool())
            .network(online)
            .database(Arc::<DatabaseFileSource>::clone(&database))
            .build();
        TestSources { loader: Rc::new(loader), database, network }
    }
}

fn main_resource_loader(file_sources: &TestSources) -> Rc<Loader> {
    Rc::clone(&file_sources.loader)
}

fn database_file_source(file_sources: &TestSources) -> Arc<DatabaseFileSource> {
    Arc::clone(&file_sources.database)
}

fn data(res: &Response) -> &[u8] {
    res.data.as_deref().map(Vec::as_slice).expect("data")
}

fn bytes(s: &str) -> crate::storage::response::Data {
    Arc::new(s.as_bytes().to_vec())
}

fn in_one_hour() -> Timestamp {
    Timestamp::from_secs(now().secs_since_epoch() + 3600)
}

fn one_hour_ago() -> Timestamp {
    Timestamp::from_secs(now().secs_since_epoch() - 3600)
}

fn cache_only(url: &str) -> Resource {
    Resource::with_tile_data(Kind::Unknown, url, None, LoadingMethod::CacheOnly)
}

// gtest: MainResourceLoader.CacheResponse
#[test]
fn cache_response() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let resource = Resource::new(Kind::Unknown, url("/cache"));

    let req1 = req();
    let req2 = req();

    let (r1, r2, fs2, stop) = (Rc::clone(&req1), Rc::clone(&req2), Rc::clone(&fs), run_loop.clone());
    let res2_resource = resource.clone();
    *req1.borrow_mut() = Some(fs.request_local(
        &local,
        &resource,
        Box::new(move |res: Response| {
            r1.borrow_mut().take();
            assert_eq!(None, res.error);
            assert_eq!(b"Response 1", data(&res));
            assert!(res.caching.expires.is_some());
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            let response = res;

            // Now test that we get the same values as in the previous
            // request. If we'd go to the server again, we'd get different
            // values.
            let (r2c, stop) = (Rc::clone(&r2), stop.clone());
            *r2.borrow_mut() = Some(fs2.request_local(
                &queue,
                &res2_resource,
                Box::new(move |res2: Response| {
                    r2c.borrow_mut().take();
                    assert_eq!(response.error, res2.error);
                    assert_eq!(data(&response), data(&res2));
                    assert_eq!(response.caching.expires, res2.caching.expires);
                    assert_eq!(response.caching.must_revalidate, res2.caching.must_revalidate);
                    assert_eq!(response.modified, res2.modified);
                    assert_eq!(response.etag, res2.etag);

                    stop.stop();
                }),
            ));
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

// gtest: MainResourceLoader.VolatileStoragePolicy
#[test]
fn volatile_storage_policy() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let mut resource = Resource::new(Kind::Unknown, url("/cache"));
    resource.storage_policy = StoragePolicy::Volatile;

    let req = req();
    let (r, fs2, stop, resource2) = (Rc::clone(&req), Rc::clone(&fs), run_loop.clone(), resource.clone());
    *req.borrow_mut() = Some(fs.request_local(
        &local,
        &resource,
        Box::new(move |res1: Response| {
            assert_eq!(None, res1.error);
            let first_data = data(&res1).to_vec();

            // Volatile resources are not stored in cache, so we always get
            // new data from the server ("Response N+1").
            let (rc, stop) = (Rc::clone(&r), stop.clone());
            *r.borrow_mut() = Some(fs2.request_local(
                &queue,
                &resource2,
                Box::new(move |res2: Response| {
                    rc.borrow_mut().take();
                    assert_eq!(None, res2.error);
                    assert_ne!(first_data.as_slice(), data(&res2));
                    stop.stop();
                }),
            ));
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

/// Caches the response at `path`, then requests it again so that it is
/// revalidated with the server; `check` inspects the second response.
fn cache_revalidate(path: &str, check: fn(&Response)) {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let revalidate = Resource::new(Kind::Unknown, url(path));
    let req1 = req();
    let req2 = req();
    let got_response = Rc::new(Cell::new(false));
    let clock = file_sources.network.clock.clone();

    let (r1, r2, fs2, stop, revalidate2) =
        (Rc::clone(&req1), Rc::clone(&req2), Rc::clone(&fs), run_loop.clone(), revalidate.clone());
    // First request causes the response to get cached.
    *req1.borrow_mut() = Some(fs.request_local(
        &local,
        &revalidate,
        Box::new(move |res: Response| {
            r1.borrow_mut().take();

            assert_eq!(None, res.error);
            assert!(!res.not_modified);
            assert_eq!(b"Response", data(&res));
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
            assert!(res.caching.must_revalidate);
            check(&res);

            // The first response is stored in the cache, but it has
            // 'must-revalidate' set. This means it can't return the cached
            // response right away and we must wait for the revalidation
            // request to complete. We can distinguish the cached response
            // from the revalided response because the latter has an
            // expiration date, while the cached response doesn't.
            let (r2c, stop, got_response, clock) =
                (Rc::clone(&r2), stop.clone(), Rc::clone(&got_response), clock.clone());
            *r2.borrow_mut() = Some(fs2.request_local(
                &queue,
                &revalidate2,
                Box::new(move |res2: Response| {
                    if got_response.get() {
                        // The server sends a Cache-Control header with a
                        // max-age of a minute. This means that our
                        // OnlineFileSource implementation will request the
                        // tile again after a minute. This time, our request
                        // already had a prior response, so we don't need to
                        // send the data again, and instead can actually
                        // forward the notModified flag.
                        r2c.borrow_mut().take();
                        assert_eq!(None, res2.error);
                        assert!(res2.not_modified);
                        assert!(res2.data.is_none());
                        assert!(res2.caching.expires.is_some());
                        assert!(res2.caching.must_revalidate);
                        check(&res2);
                        stop.stop();
                    } else {
                        // Even though we could find the response in the
                        // database, we send a revalidation request and get a
                        // 304 response. Since we haven't sent a reply yet,
                        // we're forcing notModified to be false so that
                        // implementations can continue to use the
                        // notModified flag to skip parsing new data.
                        got_response.set(true);
                        assert_eq!(None, res2.error);
                        assert!(!res2.not_modified);
                        assert_eq!(b"Response", data(&res2));
                        assert!(res2.caching.expires.is_some());
                        assert!(res2.caching.must_revalidate);
                        check(&res2);
                        clock.advance(Duration::from_secs(60));
                    }
                }),
            ));
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

// gtest: MainResourceLoader.CacheRevalidateSame
#[test]
fn cache_revalidate_same() {
    cache_revalidate("/revalidate-same", |res| {
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// gtest: MainResourceLoader.CacheRevalidateModified
#[test]
fn cache_revalidate_modified() {
    cache_revalidate("/revalidate-modified", |res| {
        assert_eq!(Some(JAN_1_2015), res.modified);
        assert!(res.etag.is_none());
    });
}

// gtest: MainResourceLoader.CacheRevalidateEtag
#[test]
fn cache_revalidate_etag() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let revalidate_etag = Resource::new(Kind::Unknown, url("/revalidate-etag"));
    let req1 = req();
    let req2 = req();

    let (r1, r2, fs2, stop, revalidate2) =
        (Rc::clone(&req1), Rc::clone(&req2), Rc::clone(&fs), run_loop.clone(), revalidate_etag.clone());
    // First request causes the response to get cached.
    *req1.borrow_mut() = Some(fs.request_local(
        &local,
        &revalidate_etag,
        Box::new(move |res: Response| {
            r1.borrow_mut().take();

            assert_eq!(None, res.error);
            assert_eq!(b"Response 1", data(&res));
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
            assert!(res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert_eq!(Some("response-1"), res.etag.as_deref());

            // Second request does not return the cached response, since it
            // had Cache-Control: must-revalidate set.
            let (r2c, stop) = (Rc::clone(&r2), stop.clone());
            *r2.borrow_mut() = Some(fs2.request_local(
                &queue,
                &revalidate2,
                Box::new(move |res2: Response| {
                    r2c.borrow_mut().take();

                    assert_eq!(None, res2.error);
                    assert!(res2.data.is_some());
                    assert_ne!(res.data, res2.data);
                    assert_eq!(b"Response 2", data(&res2));
                    assert!(res2.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
                    assert!(res2.caching.must_revalidate);
                    assert!(res2.modified.is_none());
                    assert_eq!(Some("response-2"), res2.etag.as_deref());

                    stop.stop();
                }),
            ));
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

// Test for https://github.com/mapbox/mapbox-gl-native/issue/1369
//
// A request for http://example.com is made. This triggers a cache get. While
// the cache get is pending, the request is canceled. This removes it from
// pending. Then, still while the cache get is pending, a second request is
// made for the same resource. This adds an entry back to pending and queues
// another cache request, even though the first one is still pending. Now
// both cache requests resolve to misses, resulting in two HTTP requests for
// the same resource. The first one will notify as expected, the second one
// will have bound a DefaultFileRequest* in the lambda that gets invalidated
// by the first notify's pending.erase, and when it gets notified, the crash
// occurs.
// gtest: MainResourceLoader.HTTPIssue1369
#[test]
fn http_issue_1369() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = main_resource_loader(&file_sources);

    let resource = Resource::new(Kind::Unknown, url("/test"));

    let req = req();
    *req.borrow_mut() =
        Some(fs.request_local(&local, &resource, Box::new(|_: Response| panic!("Callback should not be called"))));
    req.borrow_mut().take();
    let (r, stop) = (Rc::clone(&req), run_loop.clone());
    *req.borrow_mut() = Some(fs.request_local(
        &local,
        &resource,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            assert_eq!(None, res.error);
            assert_eq!(b"Hello World!", data(&res));
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

/// A cached value is returned for a cache-only request whether it has
/// expired or not.
fn optional_cached(expires: Timestamp) {
    let file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let optional_resource = cache_only(&url("/test"));

    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.caching.expires = Some(expires);

    let req = req();
    let dbfs = database_file_source(&file_sources);
    let (r, fs2, stop, resource2, expected) =
        (Rc::clone(&req), Rc::clone(&fs), run_loop.clone(), optional_resource.clone(), response.clone());
    dbfs.put_then(
        &optional_resource,
        &response,
        run_loop.dispatcher().callback(move || {
            let rc = Rc::clone(&r);
            *r.borrow_mut() = Some(fs2.request_local(
                &queue,
                &resource2,
                Box::new(move |res: Response| {
                    rc.borrow_mut().take();
                    assert_eq!(None, res.error);
                    assert_eq!(b"Cached value", data(&res));
                    assert!(res.caching.expires.is_some());
                    assert_eq!(expected.caching.expires, res.caching.expires);
                    assert!(!res.caching.must_revalidate);
                    assert!(res.modified.is_none());
                    assert!(res.etag.is_none());
                    stop.stop();
                }),
            ));
        }),
    );

    run_loop.run();
}

// gtest: MainResourceLoader.OptionalNonExpired
#[test]
fn optional_non_expired() {
    optional_cached(in_one_hour());
}

// gtest: MainResourceLoader.OptionalExpired
#[test]
fn optional_expired() {
    optional_cached(one_hour_ago());
}

// gtest: MainResourceLoader.OptionalNotFound
#[test]
fn optional_not_found() {
    let file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = main_resource_loader(&file_sources);

    let optional_resource = cache_only(&url("/test"));

    let req = req();
    let (r, stop) = (Rc::clone(&req), run_loop.clone());
    *req.borrow_mut() = Some(fs.request_local(
        &local,
        &optional_resource,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            let error = res.error.as_ref().expect("error");
            assert_eq!(Reason::NotFound, error.reason);
            assert_eq!("Not found in offline database", error.message);
            assert!(res.data.is_none());
            assert!(res.caching.expires.is_none());
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    ));

    run_loop.run();
}

/// The `NoCache`* tests: put a fake value into the cache to make sure we're
/// not retrieving anything from the cache, then request network-only.
fn no_cache(resource: &Resource, check: impl Fn(&Response) + 'static) {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.caching.expires = Some(in_one_hour());
    let req = req();
    let dbfs = database_file_source(&file_sources);
    let (r, fs2, stop, resource2) = (Rc::clone(&req), Rc::clone(&fs), run_loop.clone(), resource.clone());
    dbfs.put_then(
        resource,
        &response,
        run_loop.dispatcher().callback(move || {
            let rc = Rc::clone(&r);
            *r.borrow_mut() = Some(fs2.request_local(
                &queue,
                &resource2,
                Box::new(move |res: Response| {
                    rc.borrow_mut().take();
                    assert_eq!(None, res.error);
                    check(&res);
                    stop.stop();
                }),
            ));
        }),
    );

    run_loop.run_with_network(&mut file_sources.network);
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheRefreshEtagNotModified
#[test]
fn no_cache_refresh_etag_not_modified() {
    let mut resource = Resource::new(Kind::Unknown, url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_etag = Some("snowfall".to_owned());

    // The 304 says max-age=60, counted from no earlier than the request.
    let requested = now();
    no_cache(&resource, move |res| {
        assert!(res.not_modified);
        assert!(res.data.is_none());
        assert!(res.caching.expires.is_some());
        assert!(Some(Timestamp::from_secs(requested.secs_since_epoch() + 60)) <= res.caching.expires);
        assert!(res.caching.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheRefreshEtagModified
#[test]
fn no_cache_refresh_etag_modified() {
    let mut resource = Resource::new(Kind::Unknown, url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_etag = Some("sunshine".to_owned());

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
        assert!(res.caching.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheFull
#[test]
fn no_cache_full() {
    let mut resource = Resource::new(Kind::Unknown, url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
        assert!(res.caching.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that we can make a request with a Modified field that doesn't first
// try to load from cache like a regular request
// gtest: MainResourceLoader.NoCacheRefreshModifiedNotModified
#[test]
fn no_cache_refresh_modified_not_modified() {
    let mut resource = Resource::new(Kind::Unknown, url("/revalidate-modified"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_modified = Some(JAN_1_2015);

    // The 304 says max-age=60, counted from no earlier than the request.
    let requested = now();
    no_cache(&resource, move |res| {
        assert!(res.not_modified);
        assert!(res.data.is_none());
        assert!(res.caching.expires.is_some());
        assert!(Some(Timestamp::from_secs(requested.secs_since_epoch() + 60)) <= res.caching.expires);
        assert!(res.caching.must_revalidate);
        assert_eq!(Some(JAN_1_2015), res.modified);
        assert!(res.etag.is_none());
    });
}

// Test that we can make a request with a Modified field that doesn't first
// try to load from cache like a regular request
// gtest: MainResourceLoader.NoCacheRefreshModifiedModified
#[test]
fn no_cache_refresh_modified_modified() {
    let mut resource = Resource::new(Kind::Unknown, url("/revalidate-modified"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_modified = Some(Timestamp::from_secs(1_417_392_000)); // December 1, 2014

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
        assert!(res.caching.must_revalidate);
        assert_eq!(Some(JAN_1_2015), res.modified);
        assert!(res.etag.is_none());
    });
}

/// What the test's resource transform runs on the test's thread.
struct UrlTransform {
    test_url: String,
}

impl UrlTransform {
    /// Translates "<localhost://test>" to the server's /test.
    fn call(&self, url: &str, cb: FinishedCallback) {
        if url == "localhost://test" {
            cb(Ok(self.test_url.clone()));
        } else {
            cb(Ok(url.to_owned()));
        }
    }
}

// gtest: MainResourceLoader.SetResourceTransform
#[test]
fn set_resource_transform() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let resource_loader = main_resource_loader(&file_sources);
    let onlinefs = Arc::clone(&file_sources.network.source);

    // Translates the URL "localhost://test" to the server's /test on the
    // test's thread.
    let transform = Arc::new(UrlTransform { test_url: url("/test") });
    let queue = run_loop.handle().downgrade();
    onlinefs.set_resource_transform(Some(Arc::new(move |_kind: Kind, url: &str, cb: FinishedCallback| {
        let (url, transform) = (url.to_owned(), Arc::clone(&transform));
        if let Some(queue) = queue.upgrade() {
            queue.post(move |_| transform.call(&url, cb));
        }
    })));
    let resource1 = Resource::new(Kind::Unknown, "localhost://test");

    let req = req();
    let check = |res: &Response| {
        assert_eq!(None, res.error);
        assert_eq!(b"Hello World!", data(res));
        assert!(res.caching.expires.is_some_and(|expires| expires <= now()), "stale at once");
        assert!(!res.caching.must_revalidate);
        assert!(res.modified.is_none());
        assert!(res.etag.is_none());
    };
    let (r, stop) = (Rc::clone(&req), run_loop.clone());
    *req.borrow_mut() = Some(resource_loader.request_local(
        &local,
        &resource1,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            check(&res);
            stop.stop();
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);

    onlinefs.set_resource_transform(None);
    let resource2 = Resource::new(Kind::Unknown, url("/test"));

    let (r, stop) = (Rc::clone(&req), run_loop.clone());
    *req.borrow_mut() = Some(resource_loader.request_local(
        &local,
        &resource2,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            check(&res);
            stop.stop();
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

// gtest: MainResourceLoader.SetResourceCachePath
#[test]
fn set_resource_cache_path() {
    let file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let _resource_loader = main_resource_loader(&file_sources);
    let dbfs = database_file_source(&file_sources);
    let stop = run_loop.handle();
    // C++: "./new_offline.db".
    let path = temp_path("new_offline.db");
    dbfs.set_database_path(&path, Some(Box::new(move || stop.stop())));
    run_loop.run();
}

// Test that a stale cache file that has must-revalidate set will trigger a
// response.
// gtest: MainResourceLoader.RespondToStaleMustRevalidate
#[test]
fn respond_to_stale_must_revalidate() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let resource = Rc::new(RefCell::new(Resource::new(Kind::Unknown, url("/revalidate-same"))));
    resource.borrow_mut().loading_method = LoadingMethod::CacheOnly;

    // Put an existing value in the cache that has expired, and has
    // must-revalidate set.
    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.modified = Some(Timestamp::from_secs(1_417_392_000)); // December 1, 2014
    response.caching.expires = Some(Timestamp::from_secs(1_417_392_000));
    response.caching.must_revalidate = true;
    response.etag = Some("snowfall".to_owned());
    let req = req();
    let dbfs = database_file_source(&file_sources);
    let (r, fs2, stop, resource2) = (Rc::clone(&req), Rc::clone(&fs), run_loop.clone(), Rc::clone(&resource));
    let forwarded = resource.borrow().clone();
    dbfs.put_then(
        &forwarded,
        &response,
        run_loop.dispatcher().callback(move || {
            let rc = Rc::clone(&r);
            let request = resource2.borrow().clone();
            *r.borrow_mut() = Some(fs2.request_local(
                &queue,
                &request,
                Box::new(move |res: Response| {
                    rc.borrow_mut().take();
                    // The cache answers with its copy, which may not be used.
                    assert_eq!(None, res.error);
                    assert!(!res.caching.is_usable(now()));
                    assert!(!res.not_modified);
                    assert_eq!(b"Cached value", data(&res));
                    assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.caching.expires);
                    assert!(res.caching.must_revalidate);
                    assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.modified);
                    assert_eq!(Some("snowfall"), res.etag.as_deref());

                    let mut resource = resource2.borrow_mut();
                    resource.prior_etag = res.etag.clone();
                    resource.prior_modified = res.modified;
                    resource.prior_caching.expires = res.caching.expires;
                    resource.prior_data = res.data.clone();

                    stop.stop();
                }),
            ));
        }),
    );

    run_loop.run_with_network(&mut file_sources.network);

    // Now run this request again, with the data we gathered from the
    // previous stale/unusable request. We're replacing the data so that we
    // can check that the MainResourceLoader doesn't attempt another database
    // access if we already have the value.
    resource.borrow_mut().loading_method = LoadingMethod::NetworkOnly;
    resource.borrow_mut().prior_data = Some(bytes("Prior value"));

    let (r, stop) = (Rc::clone(&req), run_loop.clone());
    let request = resource.borrow().clone();
    *req.borrow_mut() = Some(fs.request_local(
        &local,
        &request,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            assert_eq!(None, res.error);
            // Since the data was found in the cache, we're doing a
            // revalidation request. Yet, since this request hasn't returned
            // data before, we're setting notModified to false in the
            // OnlineFileSource to ensure that requestors know that this is
            // the first time they're seeing this data.
            assert!(!res.not_modified);
            // Ensure that it's the value that we manually inserted into the
            // cache rather than the value the server returns, since we should
            // be executing a revalidation request which doesn't return new
            // data, only a 304 Not Modified response.
            assert_eq!(b"Prior value", data(&res));
            assert!(res.caching.expires.is_some());
            assert!(Some(now()) <= res.caching.expires);
            assert!(res.caching.must_revalidate);
            assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.modified);
            assert_eq!(Some("snowfall"), res.etag.as_deref());
            stop.stop();
        }),
    ));

    run_loop.run_with_network(&mut file_sources.network);
}

// Test that requests that revalidate cached copies, which the requester gets
// while they revalidate, have lower priority than requests for new resources
// gtest: MainResourceLoader.CachedResourceLowPriority
#[test]
fn cached_resource_low_priority() {
    let mut file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = main_resource_loader(&file_sources);
    let dbfs = database_file_source(&file_sources);

    // Expired copies of requests 3 and 4.
    let resource =
        |n: u32| Resource::with_tile_data(Kind::Unknown, url(&format!("/load/{n}")), None, LoadingMethod::All);
    for n in [3, 4] {
        let mut response = Response::new();
        response.caching.expires = Some(one_hour_ago());
        response.data = Some(bytes(&format!("Cached Request {n}")));
        dbfs.put(&resource(n), &response);
    }
    file_sources.network.source.set_maximum_concurrent_requests(1);

    // The network's answers, in the order they came, and the cached copies
    // the requests for 3 and 4 got before theirs.
    let answered: Rc<RefCell<Vec<String>>> = Rc::default();
    let cached: Rc<RefCell<Vec<String>>> = Rc::default();
    let requests: Vec<_> = [0, 1, 3, 4, 2]
        .into_iter()
        .map(|n| {
            let (answered, cached, stop) = (Rc::clone(&answered), Rc::clone(&cached), run_loop.clone());
            fs.request_local(
                &local,
                &resource(n),
                Box::new(move |res: Response| {
                    let data = String::from_utf8(data(&res).to_vec()).unwrap();
                    if data.starts_with("Cached") {
                        cached.borrow_mut().push(data);
                        return;
                    }
                    answered.borrow_mut().push(data);
                    if answered.borrow().len() == 5 {
                        stop.stop();
                    }
                }),
            )
        })
        .collect();

    // Every request reaches the network before it runs: the loader sends a
    // request to the network once the database has answered it.
    drop(dbfs.pause());
    run_loop.run_with_network(&mut file_sources.network);
    drop(requests);

    assert_eq!(["Cached Request 3", "Cached Request 4"].to_vec(), *cached.borrow());
    assert_eq!(["Request 0", "Request 1", "Request 2", "Request 3", "Request 4"].to_vec(), *answered.borrow());
}

// gtest: MainResourceLoader.NoDoubleDispatch
// A cached copy goes to the requester once: the network's 304 that
// revalidates it comes as not modified.
#[test]
fn no_double_dispatch() {
    let mut response = Response::new();
    response.data = Some(bytes("data"));
    response.etag = Some("snowfall".to_owned());
    let answers = answers_with_cached(TestSources::new(), &url("/revalidate-same"), &response, 2);

    for res in &answers {
        assert_eq!(None, res.error);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    }
    assert_eq!(b"data", data(&answers[0]));
    assert!(!answers[0].not_modified);
    assert!(answers[1].not_modified);
    assert!(answers[1].data.is_none());
}

// Not a C++ test: the "Unsupported resource request." branch (a request
// with no loading method that no file source accepts).
#[test]
fn unsupported_resource_request() {
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = Loader::new(&ResourceOptions::default(), &super::pool());
    let resource = Resource::with_tile_data(Kind::Unknown, "http://example.com/x.json", None, LoadingMethod::None);
    let result: Rc<RefCell<Option<Response>>> = Rc::new(RefCell::new(None));
    let (out, stop) = (Rc::clone(&result), run_loop.clone());
    let _req = fs.request_local(
        &local,
        &resource,
        Box::new(move |res: Response| {
            *out.borrow_mut() = Some(res);
            stop.stop();
        }),
    );
    run_loop.run();
    let res = result.borrow_mut().take().expect("response");
    assert!(res.no_content);
    assert_eq!("Unsupported resource request.", res.error.unwrap().message);
}

/// The first `count` answers to a request of `url` from `file_sources`,
/// whose cache holds `cached`.
fn answers_with_cached(mut file_sources: TestSources, url: &str, cached: &Response, count: usize) -> Vec<Response> {
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);
    let resource = Resource::new(Kind::Unknown, url);
    let answers = Rc::new(RefCell::new(Vec::new()));
    let req = req();
    let dbfs = database_file_source(&file_sources);
    let (r, stop, collected) = (Rc::clone(&req), run_loop.clone(), Rc::clone(&answers));
    let request = resource.clone();
    dbfs.put_then(
        &resource,
        cached,
        run_loop.dispatcher().callback(move || {
            *r.borrow_mut() = Some(fs.request_local(
                &queue,
                &request,
                Box::new(move |res: Response| {
                    collected.borrow_mut().push(res);
                    if collected.borrow().len() == count {
                        stop.stop();
                    }
                }),
            ));
        }),
    );
    run_loop.run_with_network(&mut file_sources.network);
    req.borrow_mut().take();
    answers.take()
}

fn stale_copy() -> Response {
    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.caching.expires = Some(one_hour_ago());
    response
}

/// `response`, with its caching rules changed by `change`.
fn with_caching(mut response: Response, change: impl FnOnce(&mut Caching)) -> Response {
    change(&mut response.caching);
    response
}

/// A stale copy goes to the requester at once, and the network's answer
/// follows; one that must revalidate waits for the network.
#[test]
fn a_stale_copy_shows_while_it_revalidates_unless_it_must_revalidate() {
    let answers = answers_with_cached(TestSources::new(), &url("/test"), &stale_copy(), 2);
    assert_eq!(b"Cached value", data(&answers[0]));
    assert_eq!(b"Hello World!", data(&answers[1]));

    let cached = with_caching(stale_copy(), |caching| caching.must_revalidate = true);
    let answers = answers_with_cached(TestSources::new(), &url("/test"), &cached, 1);
    assert_eq!(b"Hello World!", data(&answers[0]));
}

/// When the server can't be reached, the requester keeps the stale copy it
/// was shown, and hears of the failure; a copy that must revalidate isn't
/// delivered.
#[test]
fn a_stale_copy_shows_when_the_server_cant_be_reached() {
    let unreachable = || TestSources::serving(Network::serving(|_| Reply::Refused));
    let answers = answers_with_cached(unreachable(), &url("/test"), &stale_copy(), 2);
    assert_eq!(b"Cached value", data(&answers[0]));
    assert_eq!(Reason::Connection, answers[1].error.as_ref().expect("an error").reason);

    let cached = with_caching(stale_copy(), |caching| caching.must_revalidate = true);
    let answers = answers_with_cached(unreachable(), &url("/test"), &cached, 1);
    assert_eq!(Reason::Connection, answers[0].error.as_ref().expect("an error").reason);
}
