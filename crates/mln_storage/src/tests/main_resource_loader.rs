// The tests run against the test server (`mln_test_server`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_test_server::HttpServer;
use mln_util::chrono::{Timestamp, now};

use super::{LocalRequests, TestLoop, req, temp_path};
use crate::database_file_source::{Cache, DatabaseFileSource};
use crate::loader::Loader;
use crate::network_status::{NetworkStatus, Status};
use crate::online_file_source::OnlineFileSource;
use crate::resource::{Kind, LoadingMethod, Resource, StoragePolicy};
use crate::resource_options::ResourceOptions;
use crate::resource_transform::FinishedCallback;
use crate::response::{Reason, Response};

/// A loader over an in-memory database and an online file source, and those
/// two.
struct TestSources {
    loader: Rc<Loader>,
    database: Arc<DatabaseFileSource>,
    online: Arc<OnlineFileSource>,
    /// The online file source's.
    network: Arc<NetworkStatus>,
}

impl TestSources {
    fn new() -> Self {
        let network = Arc::new(NetworkStatus::new());
        let online = Arc::new(OnlineFileSource::new(Arc::clone(&network)));
        let database = Arc::new(DatabaseFileSource::new(":memory:", Some(Arc::<OnlineFileSource>::clone(&online))));
        let loader = Loader::builder(&super::pool())
            .network(Arc::<OnlineFileSource>::clone(&online))
            .database(Arc::<DatabaseFileSource>::clone(&database))
            .build();
        TestSources { loader: Rc::new(loader), database, online, network }
    }
}

fn main_resource_loader(file_sources: &TestSources) -> Rc<Loader> {
    Rc::clone(&file_sources.loader)
}

fn database_file_source(file_sources: &TestSources) -> Arc<DatabaseFileSource> {
    Arc::clone(&file_sources.database)
}

fn online_file_source(file_sources: &TestSources) -> Arc<OnlineFileSource> {
    Arc::clone(&file_sources.online)
}

fn data(res: &Response) -> &[u8] {
    res.data.as_deref().map(Vec::as_slice).expect("data")
}

fn bytes(s: &str) -> crate::response::Data {
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
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let resource = Resource::new(Kind::Unknown, server.url("/cache"));

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
            assert!(res.expires.is_some());
            assert!(!res.must_revalidate);
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
                    assert_eq!(response.expires, res2.expires);
                    assert_eq!(response.must_revalidate, res2.must_revalidate);
                    assert_eq!(response.modified, res2.modified);
                    assert_eq!(response.etag, res2.etag);

                    stop.stop();
                }),
            ));
        }),
    ));

    run_loop.run();
}

// gtest: MainResourceLoader.VolatileStoragePolicy
#[test]
fn volatile_storage_policy() {
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let mut resource = Resource::new(Kind::Unknown, server.url("/cache"));
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

    run_loop.run();
}

/// Caches the response at `path`, then requests it again so that it is
/// revalidated with the server; `check` inspects the second response.
fn cache_revalidate(path: &str, check: fn(&Response)) {
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let revalidate = Resource::new(Kind::Unknown, server.url(path));
    let req1 = req();
    let req2 = req();
    let got_response = Rc::new(Cell::new(false));

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
            assert!(res.expires.is_none());
            assert!(res.must_revalidate);
            check(&res);

            // The first response is stored in the cache, but it has
            // 'must-revalidate' set. This means it can't return the cached
            // response right away and we must wait for the revalidation
            // request to complete. We can distinguish the cached response
            // from the revalided response because the latter has an
            // expiration date, while the cached response doesn't.
            let (r2c, stop, got_response) = (Rc::clone(&r2), stop.clone(), Rc::clone(&got_response));
            *r2.borrow_mut() = Some(fs2.request_local(
                &queue,
                &revalidate2,
                Box::new(move |res2: Response| {
                    if got_response.get() {
                        // The test server sends a Cache-Control header with
                        // a max-age of 1 second. This means that our
                        // OnlineFileSource implementation will request the
                        // tile again after 1 second. This time, our request
                        // already had a prior response, so we don't need to
                        // send the data again, and instead can actually
                        // forward the notModified flag.
                        r2c.borrow_mut().take();
                        assert_eq!(None, res2.error);
                        assert!(res2.not_modified);
                        assert!(res2.data.is_none());
                        assert!(res2.expires.is_some());
                        assert!(res2.must_revalidate);
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
                        assert!(res2.expires.is_some());
                        assert!(res2.must_revalidate);
                        check(&res2);
                    }
                }),
            ));
        }),
    ));

    run_loop.run();
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
        assert_eq!(Some(Timestamp::from_secs(1_420_070_400)), res.modified);
        assert!(res.etag.is_none());
    });
}

// gtest: MainResourceLoader.CacheRevalidateEtag
#[test]
fn cache_revalidate_etag() {
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let revalidate_etag = Resource::new(Kind::Unknown, server.url("/revalidate-etag"));
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
            assert!(res.expires.is_none());
            assert!(res.must_revalidate);
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
                    assert!(res2.expires.is_none());
                    assert!(res2.must_revalidate);
                    assert!(res2.modified.is_none());
                    assert_eq!(Some("response-2"), res2.etag.as_deref());

                    stop.stop();
                }),
            ));
        }),
    ));

    run_loop.run();
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
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = main_resource_loader(&file_sources);

    let resource = Resource::new(Kind::Unknown, server.url("/test"));

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
            assert!(res.expires.is_none());
            assert!(!res.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    ));

    run_loop.run();
}

/// A cached value is returned for a cache-only request whether it has
/// expired or not.
fn optional_cached(expires: Timestamp) {
    let file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let optional_resource = cache_only("http://127.0.0.1:3000/test");

    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.expires = Some(expires);

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
                    assert!(res.expires.is_some());
                    assert_eq!(expected.expires, res.expires);
                    assert!(!res.must_revalidate);
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

    let optional_resource = cache_only("http://127.0.0.1:3000/test");

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
            assert!(res.expires.is_none());
            assert!(!res.must_revalidate);
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
    let file_sources = TestSources::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.expires = Some(in_one_hour());
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

    run_loop.run();
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheRefreshEtagNotModified
#[test]
fn no_cache_refresh_etag_not_modified() {
    let server = HttpServer::new();
    let mut resource = Resource::new(Kind::Unknown, server.url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_etag = Some("snowfall".to_owned());

    // The 304 says max-age=1, counted from no earlier than the request.
    let requested = now();
    no_cache(&resource, move |res| {
        assert!(res.not_modified);
        assert!(res.data.is_none());
        assert!(res.expires.is_some());
        assert!(Some(Timestamp::from_secs(requested.secs_since_epoch() + 1)) <= res.expires);
        assert!(res.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheRefreshEtagModified
#[test]
fn no_cache_refresh_etag_modified() {
    let server = HttpServer::new();
    let mut resource = Resource::new(Kind::Unknown, server.url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_etag = Some("sunshine".to_owned());

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.expires.is_none());
        assert!(res.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that a network only request doesn't attempt to load data from the
// cache.
// gtest: MainResourceLoader.NoCacheFull
#[test]
fn no_cache_full() {
    let server = HttpServer::new();
    let mut resource = Resource::new(Kind::Unknown, server.url("/revalidate-same"));
    resource.loading_method = LoadingMethod::NetworkOnly;

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.expires.is_none());
        assert!(res.must_revalidate);
        assert!(res.modified.is_none());
        assert_eq!(Some("snowfall"), res.etag.as_deref());
    });
}

// Test that we can make a request with a Modified field that doesn't first
// try to load from cache like a regular request
// gtest: MainResourceLoader.NoCacheRefreshModifiedNotModified
#[test]
fn no_cache_refresh_modified_not_modified() {
    let server = HttpServer::new();
    let mut resource = Resource::new(Kind::Unknown, server.url("/revalidate-modified"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_modified = Some(Timestamp::from_secs(1_420_070_400)); // January 1, 2015

    // The 304 says max-age=1, counted from no earlier than the request.
    let requested = now();
    no_cache(&resource, move |res| {
        assert!(res.not_modified);
        assert!(res.data.is_none());
        assert!(res.expires.is_some());
        assert!(Some(Timestamp::from_secs(requested.secs_since_epoch() + 1)) <= res.expires);
        assert!(res.must_revalidate);
        assert_eq!(Some(Timestamp::from_secs(1_420_070_400)), res.modified);
        assert!(res.etag.is_none());
    });
}

// Test that we can make a request with a Modified field that doesn't first
// try to load from cache like a regular request
// gtest: MainResourceLoader.NoCacheRefreshModifiedModified
#[test]
fn no_cache_refresh_modified_modified() {
    let server = HttpServer::new();
    let mut resource = Resource::new(Kind::Unknown, server.url("/revalidate-modified"));
    resource.loading_method = LoadingMethod::NetworkOnly;
    resource.prior_modified = Some(Timestamp::from_secs(1_417_392_000)); // December 1, 2014

    no_cache(&resource, |res| {
        assert!(!res.not_modified);
        assert_eq!(b"Response", data(res));
        assert!(res.expires.is_none());
        assert!(res.must_revalidate);
        assert_eq!(Some(Timestamp::from_secs(1_420_070_400)), res.modified);
        assert!(res.etag.is_none());
    });
}

/// What the test's resource transform runs on the test's thread.
struct UrlTransform {
    test_url: String,
}

impl UrlTransform {
    /// Translates "<localhost://test>" to the test server's /test.
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
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let resource_loader = main_resource_loader(&file_sources);
    let onlinefs = online_file_source(&file_sources);

    // Translates the URL "localhost://test to http://127.0.0.1:3000/test
    // on the test's thread.
    let transform = Arc::new(UrlTransform { test_url: server.url("/test") });
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
        assert!(res.expires.is_none());
        assert!(!res.must_revalidate);
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

    run_loop.run();

    onlinefs.set_resource_transform(None);
    let resource2 = Resource::new(Kind::Unknown, server.url("/test"));

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

    run_loop.run();
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
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let resource = Rc::new(RefCell::new(Resource::new(Kind::Unknown, server.url("/revalidate-same"))));
    resource.borrow_mut().loading_method = LoadingMethod::CacheOnly;

    // Put an existing value in the cache that has expired, and has
    // must-revalidate set.
    let mut response = Response::new();
    response.data = Some(bytes("Cached value"));
    response.modified = Some(Timestamp::from_secs(1_417_392_000)); // December 1, 2014
    response.expires = Some(Timestamp::from_secs(1_417_392_000));
    response.must_revalidate = true;
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
                    let error = res.error.as_ref().expect("error");
                    assert_eq!(Reason::NotFound, error.reason);
                    assert_eq!("Cached resource is unusable", error.message);
                    assert!(!res.not_modified);
                    assert_eq!(b"Cached value", data(&res));
                    assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.expires);
                    assert!(res.must_revalidate);
                    assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.modified);
                    assert_eq!(Some("snowfall"), res.etag.as_deref());

                    let mut resource = resource2.borrow_mut();
                    resource.prior_etag = res.etag.clone();
                    resource.prior_modified = res.modified;
                    resource.prior_expires = res.expires;
                    resource.prior_data = res.data.clone();

                    stop.stop();
                }),
            ));
        }),
    );

    run_loop.run();

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
            assert!(res.expires.is_some());
            assert!(Some(now()) <= res.expires);
            assert!(res.must_revalidate);
            assert_eq!(Some(Timestamp::from_secs(1_417_392_000)), res.modified);
            assert_eq!(Some("snowfall"), res.etag.as_deref());
            stop.stop();
        }),
    ));

    run_loop.run();
}

// Test that requests for expired resources have lower priority than
// requests for new resources
// Flaky https://github.com/maplibre/maplibre-native/issues/1071
// gtest: MainResourceLoader.CachedResourceLowPriority
#[test]
fn cached_resource_low_priority() {
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = main_resource_loader(&file_sources);

    let mut response = Response::new();
    let online_response_counter = Rc::new(Cell::new(0usize));

    response.expires = Some(one_hour_ago());

    let dbfs = database_file_source(&file_sources);
    let online_fs = online_file_source(&file_sources);

    // Put existing values into the cache.
    let resource1 = Resource::with_tile_data(Kind::Unknown, server.url("/load/3"), None, LoadingMethod::All);
    response.data = Some(bytes("Cached Request 3"));
    dbfs.put(&resource1, &response);

    let resource2 = Resource::with_tile_data(Kind::Unknown, server.url("/load/4"), None, LoadingMethod::All);
    response.data = Some(bytes("Cached Request 4"));
    dbfs.put(&resource2, &response);

    online_fs.set_maximum_concurrent_requests(1);
    let paused = dbfs.pause();
    file_sources.network.set(Status::Offline);

    // Ensure that the online requests for new resources are processed first.
    let non_cached0 = Resource::with_tile_data(Kind::Unknown, server.url("/load/0"), None, LoadingMethod::All);
    let req0 = req();
    let (r, counter) = (Rc::clone(&req0), Rc::clone(&online_response_counter));
    *req0.borrow_mut() = Some(fs.request_local(
        &local,
        &non_cached0,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            assert_eq!(counter.get(), 0); // make sure this is responded first
            assert_eq!(b"Request 0", data(&res));
        }),
    ));

    let non_cached1 = Resource::with_tile_data(Kind::Unknown, server.url("/load/1"), None, LoadingMethod::All);
    let req1 = req();
    let (r, counter) = (Rc::clone(&req1), Rc::clone(&online_response_counter));
    *req1.borrow_mut() = Some(fs.request_local(
        &local,
        &non_cached1,
        Box::new(move |res: Response| {
            counter.set(counter.get() + 1);
            r.borrow_mut().take();
            assert_eq!(counter.get(), 1); // make sure this is responded second
            assert_eq!(b"Request 1", data(&res));
        }),
    ));

    let req3_cached_response_received = Rc::new(Cell::new(false));
    let req3 = req();
    let (r, counter, received) =
        (Rc::clone(&req3), Rc::clone(&online_response_counter), Rc::clone(&req3_cached_response_received));
    *req3.borrow_mut() = Some(fs.request_local(
        &local,
        &resource1,
        Box::new(move |res: Response| {
            // Offline callback is received first
            if received.get() {
                counter.set(counter.get() + 1);
                r.borrow_mut().take();
                assert_eq!(counter.get(), 3);
                assert_eq!(b"Request 3", data(&res));
            } else {
                assert_eq!(b"Cached Request 3", data(&res));
                received.set(true);
            }
        }),
    ));

    let req4_cached_response_received = Rc::new(Cell::new(false));
    let req4 = req();
    let (r, counter, received, stop) = (
        Rc::clone(&req4),
        Rc::clone(&online_response_counter),
        Rc::clone(&req4_cached_response_received),
        run_loop.clone(),
    );
    *req4.borrow_mut() = Some(fs.request_local(
        &local,
        &resource2,
        Box::new(move |res: Response| {
            // Offline callback is received first
            if received.get() {
                counter.set(counter.get() + 1);
                r.borrow_mut().take();
                assert_eq!(counter.get(), 4);
                assert_eq!(b"Request 4", data(&res));
                stop.stop();
            } else {
                assert_eq!(b"Cached Request 4", data(&res));
                received.set(true);
            }
        }),
    ));

    let non_cached2 = Resource::with_tile_data(Kind::Unknown, server.url("/load/2"), None, LoadingMethod::All);
    let req2 = req();
    let (r, counter) = (Rc::clone(&req2), Rc::clone(&online_response_counter));
    *req2.borrow_mut() = Some(fs.request_local(
        &local,
        &non_cached2,
        Box::new(move |res: Response| {
            counter.set(counter.get() + 1);
            r.borrow_mut().take();
            assert_eq!(counter.get(), 2); // make sure this is responded third
            assert_eq!(b"Request 2", data(&res));
        }),
    ));

    drop(paused);
    // The online requests all reach the online file source while it is
    // offline, so going online starts them in priority order: request 2 was
    // sent after the expired resources' requests, but its database lookup
    // could otherwise finish after requests 0 and 1 did, and the free slot
    // would go to request 3. The loader sends a request to the database, and
    // to the network once the database answers: the pauses wait for each
    // thread in turn to finish what it was sent.
    drop(dbfs.pause());
    drop(online_fs.pause());
    file_sources.network.set(Status::Online);

    run_loop.run();
}

// gtest: MainResourceLoader.NoDoubleDispatch
#[test]
fn no_double_dispatch() {
    let file_sources = TestSources::new();
    let server = HttpServer::new();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let queue = local.clone();
    let fs = main_resource_loader(&file_sources);

    let resource = Resource::new(Kind::Unknown, server.url("/revalidate-same"));
    let mut response = Response::new();
    response.data = Some(bytes("data"));
    response.etag = Some("snowfall".to_owned());

    let req = req();
    let response_count = Rc::new(Cell::new(0u32));
    // A second dispatch would answer within moments of the first, so the
    // check waits that long after the first answer, however long it took.
    let dbfs = database_file_source(&file_sources);
    let (r, fs2, resource2, count) = (Rc::clone(&req), Rc::clone(&fs), resource.clone(), Rc::clone(&response_count));
    let (stop, handle) = (run_loop.clone(), run_loop.handle());
    dbfs.put_then(
        &resource,
        &response,
        run_loop.dispatcher().callback(move || {
            *r.borrow_mut() = Some(fs2.request_local(
                &queue,
                &resource2,
                Box::new(move |res: Response| {
                    assert_eq!(None, res.error);
                    assert!(res.modified.is_none());
                    assert!(res.etag.is_some());
                    assert_eq!(Some("snowfall"), res.etag.as_deref());
                    if !res.not_modified {
                        assert_eq!(b"data", data(&res));
                        count.set(count.get() + 1);
                        if count.get() == 1 {
                            let (stop, count) = (stop.clone(), Rc::clone(&count));
                            let check = stop.dispatcher().callback(move || {
                                assert_eq!(1, count.get());
                                stop.stop();
                            });
                            handle.post_after(Duration::from_millis(100), move |_| check());
                        }
                    }
                }),
            ));
        }),
    );

    run_loop.run();
    assert_eq!(1, response_count.get());
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
