// The tests run the HTTP client against a local HTTP server (httpmock).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use httpmock::{HttpMockResponse, MockServer};
use mln_util::chrono::{Timestamp, now, parse_timestamp_i32, rfc1123};
use parking_lot::Mutex;

use super::{Dispatcher, LocalRequests, Req, TestLoop, req};
use crate::storage::file_source::FileSource;
use crate::storage::http_file_source::{HTTPFileSource, same_origin};
use crate::storage::http_header_transform::HttpHeaderTransform;
use crate::storage::resource::{Kind, LoadingMethod, Resource, TileFormat};
use crate::storage::response::{Reason, Response};

fn http_file_source() -> HTTPFileSource {
    HTTPFileSource::new()
}

fn data(res: &Response) -> &[u8] {
    res.data.as_deref().map(Vec::as_slice).expect("data")
}

/// A server whose `path` answers with `status`, `headers` and `body`.
fn serving(path: &str, status: u16, headers: &[(&str, String)], body: &str) -> MockServer {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path(path);
        let then = headers.iter().fold(then.status(status), |then, (name, value)| then.header(*name, value));
        then.body(body);
    });
    server
}

/// A server whose `/test` answers "Hello World!" without caching headers.
fn hello() -> MockServer {
    serving("/test", 200, &[], "Hello World!")
}

// gtest: HTTPFileSource.Cancel
#[test]
fn cancel() {
    let server = hello();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    drop(fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/test")),
        Box::new(|_: Response| panic!("Callback should not be called")),
    ));

    run_loop.run_once();
}

// gtest: HTTPFileSource.HTTP200
#[test]
fn http200() {
    let server = hello();
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/test")),
        Box::new(move |res: Response| {
            assert_eq!(None, res.error);
            assert_eq!(b"Hello World!", data(&res));
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTP206
#[test]
fn http206() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/test").header("Range", "bytes=3-8");
        then.status(206).header("Content-Range", "bytes 3-8/12").body("lo Wor");
    });
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let mut resource = Resource::new(Kind::Unknown, server.url("/test"));
    resource.data_range = Some((3, 8));

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &resource,
        Box::new(move |res: Response| {
            assert_eq!(None, res.error);
            assert_eq!(b"lo Wor", data(&res));
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTP404
#[test]
fn http404() {
    let server = serving("/doesnotexist", 404, &[], "");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/doesnotexist")),
        Box::new(move |res: Response| {
            let error = res.error.as_ref().expect("error");
            assert_eq!(Reason::NotFound, error.reason);
            assert_eq!("HTTP status code 404", error.message);
            assert!(res.data.is_none());
            assert!(res.caching.expires.is_none());
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTPTile404
#[test]
fn http_tile404() {
    let server = serving("/doesnotexist", 404, &[], "");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Tile, server.url("/doesnotexist")),
        Box::new(move |res: Response| {
            assert!(res.no_content);
            assert!(res.tile_not_found);
            assert!(res.error.is_none());
            assert!(res.data.is_none());
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTP200EmptyData
#[test]
fn http200_empty_data() {
    let server = serving("/empty-data", 200, &[], "");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/empty-data")),
        Box::new(move |res: Response| {
            assert!(!res.no_content);
            assert!(res.error.is_none());
            assert_eq!(b"", data(&res));
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTP204
#[test]
fn http204() {
    let server = serving("/no-content", 204, &[], "");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/no-content")),
        Box::new(move |res: Response| {
            assert!(res.no_content);
            assert!(!res.tile_not_found);
            assert!(res.error.is_none());
            assert!(res.data.is_none());
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.HTTP500
#[test]
fn http500() {
    let server = serving("/permanent-error", 500, &[], "Server Error!");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/permanent-error")),
        Box::new(move |res: Response| {
            let error = res.error.as_ref().expect("error");
            assert_eq!(Reason::Server, error.reason);
            assert_eq!("HTTP status code 500", error.message);
            assert!(res.data.is_none());
            assert!(res.caching.expires.is_none());
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.ExpiresParsing
#[test]
fn expires_parsing() {
    let server = serving(
        "/test",
        200,
        &[
            ("Last-Modified", rfc1123(parse_timestamp_i32(1_420_794_326))),
            ("Expires", rfc1123(parse_timestamp_i32(2_000_000_000))),
            ("ETag", "foo".to_owned()),
        ],
        "Hello World!",
    );
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/test")),
        Box::new(move |res: Response| {
            assert_eq!(None, res.error);
            assert_eq!(b"Hello World!", data(&res));
            let expires = res.caching.expires.expect("expires").secs_since_epoch();
            assert!((expires - 2_000_000_000).abs() < 2, "expires at the Expires date");
            assert!(!res.caching.must_revalidate);
            assert_eq!(Some(Timestamp::from_secs(1_420_794_326)), res.modified);
            assert_eq!(Some("foo"), res.etag.as_deref());
            stop.stop();
        }),
    );

    run_loop.run();
}

// gtest: HTTPFileSource.CacheControlParsing
#[test]
fn cache_control_parsing() {
    let server = serving("/test", 200, &[("Cache-Control", "max-age=120".to_owned())], "Hello World!");
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs = http_file_source();

    let stop = run_loop.clone();
    let _req = fs.request_local(
        &local,
        &Resource::new(Kind::Unknown, server.url("/test")),
        Box::new(move |res: Response| {
            assert_eq!(None, res.error);
            assert_eq!(b"Hello World!", data(&res));
            let delta = res.caching.expires.expect("expires").secs_since_epoch() - now().secs_since_epoch() - 120;
            assert!(delta.abs() < 2, "Expiration date isn't about 120 seconds in the future");
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());
            stop.stop();
        }),
    );

    run_loop.run();
}

/// The shared state of the `Load` test: `reqs[concurrency]` and `number`.
struct LoadState {
    reqs: Vec<Req>,
    number: Cell<i32>,
}

/// The `req(i)` lambda of the `Load` test.
fn load_request(
    fs: &Rc<dyn FileSource>,
    state: &Rc<LoadState>,
    url: &Rc<dyn Fn(i32) -> String>,
    run_loop: &TestLoop,
    local: &Dispatcher,
    i: usize,
    max: i32,
) {
    let current = state.number.get();
    state.number.set(current + 1);
    let (fs2, state2, url2, run_loop2, local2) =
        (Rc::clone(fs), Rc::clone(state), Rc::clone(url), run_loop.clone(), local.clone());
    let request = fs.request_local(
        local,
        &Resource::new(Kind::Unknown, url(current)),
        Box::new(move |res: Response| {
            state2.reqs[i].borrow_mut().take();
            assert_eq!(None, res.error);
            assert_eq!(format!("Request {current}").as_bytes(), data(&res));
            // Without caching headers, the response is stale at once.
            assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
            assert!(!res.caching.must_revalidate);
            assert!(res.modified.is_none());
            assert!(res.etag.is_none());

            if state2.number.get() <= max {
                load_request(&fs2, &state2, &url2, &run_loop2, &local2, i, max);
            } else if current == max {
                run_loop2.stop();
            }
        }),
    );
    *state.reqs[i].borrow_mut() = Some(request);
}

// gtest: HTTPFileSource.Load
#[test]
fn load() {
    const CONCURRENCY: usize = 50;
    const MAX: i32 = 10000;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path_matches(r"^/load/\d+$");
        then.respond_with(|request| {
            let number = request.uri().path()["/load/".len()..].to_owned();
            HttpMockResponse::builder().status(200).body(format!("Request {number}")).build()
        });
    });
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let fs: Rc<dyn FileSource> = Rc::new(http_file_source());

    let state = Rc::new(LoadState { reqs: (0..CONCURRENCY).map(|_| req()).collect(), number: Cell::new(1) });
    let base = server.url("/load/");
    let url: Rc<dyn Fn(i32) -> String> = Rc::new(move |n| format!("{base}{n}"));

    for i in 0..CONCURRENCY {
        load_request(&fs, &state, &url, &run_loop, &local, i, MAX);
    }

    run_loop.run();
}

const TOKEN_HEADER: &str = "X-Test-Token";
const TOKEN_VALUE: &str = "secret-token";

/// Adds the token header to every request, recording the kind and URL of
/// each.
fn token_transform(seen: Arc<Mutex<Vec<(Kind, String)>>>) -> HttpHeaderTransform {
    HttpHeaderTransform::new(Some(Arc::new(move |kind: Kind, url: &str| {
        seen.lock().push((kind, url.to_owned()));
        vec![(TOKEN_HEADER.to_owned(), TOKEN_VALUE.to_owned())]
    })))
}

/// Loads `url` with `fs`, and returns the response.
fn load_once(fs: &HTTPFileSource, kind: Kind, url: String) -> Response {
    load_resource(fs, &Resource::new(kind, url))
}

/// Loads `resource` with `fs`, and returns the response.
fn load_resource(fs: &HTTPFileSource, resource: &Resource) -> Response {
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let result = Rc::new(RefCell::new(None));
    let (r, stop) = (Rc::clone(&result), run_loop.clone());
    let _req = fs.request_local(
        &local,
        resource,
        Box::new(move |res: Response| {
            *r.borrow_mut() = Some(res);
            stop.stop();
        }),
    );
    run_loop.run();
    result.take().expect("response")
}

#[test]
fn a_header_transform_adds_headers_until_cleared() {
    let server = MockServer::start();
    let with_token = server.mock(|when, then| {
        when.method("GET").path("/with-token.json").header(TOKEN_HEADER, TOKEN_VALUE);
        then.status(200).body("with token");
    });
    let after_clear = server.mock(|when, then| {
        when.method("GET").path("/after-clear.json").header_missing(TOKEN_HEADER);
        then.status(200).body("after clear");
    });
    let mut fs = http_file_source();
    let seen = Arc::new(Mutex::new(Vec::new()));
    fs.set_http_header_transform(token_transform(Arc::clone(&seen)));

    let res = load_once(&fs, Kind::Style, server.url("/with-token.json"));
    assert_eq!(None, res.error);
    assert_eq!(b"with token", data(&res));
    assert_eq!(vec![(Kind::Style, server.url("/with-token.json"))], *seen.lock());

    fs.set_http_header_transform(HttpHeaderTransform::default());
    let res = load_once(&fs, Kind::Style, server.url("/after-clear.json"));
    assert_eq!(None, res.error);
    assert_eq!(b"after clear", data(&res));
    assert_eq!(1, seen.lock().len());
    with_token.assert_calls(1);
    after_clear.assert_calls(1);
}

#[test]
fn a_redirect_keeps_headers_only_within_the_origin() {
    let other = MockServer::start();
    let elsewhere = other.mock(|when, then| {
        when.method("GET").path("/elsewhere.json").header_missing(TOKEN_HEADER);
        then.status(200).body("elsewhere");
    });
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/same-origin.json").header(TOKEN_HEADER, TOKEN_VALUE);
        then.status(302).header("Location", "/redirected.json");
    });
    let redirected = server.mock(|when, then| {
        when.method("GET").path("/redirected.json").header(TOKEN_HEADER, TOKEN_VALUE);
        then.status(200).body("redirected");
    });
    server.mock(|when, then| {
        when.method("GET").path("/cross-origin.json").header(TOKEN_HEADER, TOKEN_VALUE);
        then.status(302).header("Location", other.url("/elsewhere.json"));
    });
    let mut fs = http_file_source();
    fs.set_http_header_transform(token_transform(Arc::default()));

    let res = load_once(&fs, Kind::Style, server.url("/same-origin.json"));
    assert_eq!(None, res.error);
    assert_eq!(b"redirected", data(&res));
    redirected.assert_calls(1);

    let res = load_once(&fs, Kind::Style, server.url("/cross-origin.json"));
    assert_eq!(None, res.error);
    assert_eq!(b"elsewhere", data(&res));
    elsewhere.assert_calls(1);
}

const MLT_ACCEPT: &str = "application/vnd.maplibre-tile, */*;q=0.1";

fn mlt_tile(url: &str) -> Resource {
    Resource::tile(url, 1.0, 0, 0, 0, mln_style::tileset::Scheme::XYZ, TileFormat::Mlt, LoadingMethod::All)
}

#[test]
fn a_tile_accepts_its_format_past_a_redirect_that_drops_the_hosts_headers() {
    let other = MockServer::start();
    let elsewhere = other.mock(|when, then| {
        when.method("GET").path("/elsewhere.mlt").header("Accept", MLT_ACCEPT).header_missing(TOKEN_HEADER);
        then.status(200).body("elsewhere");
    });
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/tile.mlt").header("Accept", MLT_ACCEPT).header(TOKEN_HEADER, TOKEN_VALUE);
        then.status(302).header("Location", other.url("/elsewhere.mlt"));
    });
    let mut fs = http_file_source();
    fs.set_http_header_transform(token_transform(Arc::default()));

    let res = load_resource(&fs, &mlt_tile(&server.url("/tile.mlt")));
    assert_eq!(None, res.error);
    assert_eq!(b"elsewhere", data(&res));
    elsewhere.assert_calls(1);
}

#[test]
fn a_hosts_accept_replaces_a_tiles() {
    let server = MockServer::start();
    let custom = server.mock(|when, then| {
        when.method("GET").path("/tile.mlt").header("Accept", "application/x-custom");
        then.status(200).body("custom");
    });
    let mut fs = http_file_source();
    fs.set_http_header_transform(HttpHeaderTransform::new(Some(Arc::new(|_: Kind, _: &str| {
        vec![("Accept".to_owned(), "application/x-custom".to_owned())]
    }))));

    let res = load_resource(&fs, &mlt_tile(&server.url("/tile.mlt")));
    assert_eq!(None, res.error);
    assert_eq!(b"custom", data(&res));
    custom.assert_calls(1);
}

#[test]
fn origins_compare_scheme_host_and_effective_port() {
    assert!(same_origin("https://EXAMPLE.test/a", "https://example.test:443/b"));
    assert!(same_origin("http://example.test?a", "http://user@example.test:80#b"));
    assert!(same_origin("http://[::1]:8080/a", "http://[::1]:8080/b"));
    assert!(!same_origin("http://example.test/a", "https://example.test/b"));
    assert!(!same_origin("https://example.test/a", "https://other.test/b"));
    assert!(!same_origin("https://example.test:444/a", "https://example.test/b"));
    assert!(!same_origin("http://[::1]:8080/a", "http://[::1]:8081/b"));
    assert!(!same_origin("file:///a", "file:///a"));
    assert!(!same_origin("http:///a", "http:///a"));
}

/// A 503 is a server error that waits for its `Retry-After`, and a 408 a
/// connection error, which the online file source retries.
#[test]
fn unavailable_and_request_timeout_are_retried() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/unavailable");
        then.status(503).header("Retry-After", "120");
    });
    server.mock(|when, then| {
        when.method("GET").path("/timeout");
        then.status(408);
    });
    let fs = http_file_source();

    let res = load_once(&fs, Kind::Tile, server.url("/unavailable"));
    let error = res.error.expect("error");
    assert_eq!(Reason::Server, error.reason);
    let retry_after = error.retry_after.expect("retry after").secs_since_epoch() - now().secs_since_epoch();
    assert!((118..=120).contains(&retry_after), "{retry_after}");

    let res = load_once(&fs, Kind::Tile, server.url("/timeout"));
    assert_eq!(Reason::Connection, res.error.expect("error").reason);
}

/// A request with a prior copy asks the server whether it changed: by its
/// `ETag` if it has one, else by its modification date.
#[test]
fn a_request_with_a_prior_copy_is_conditional() {
    let modified = Timestamp::from_secs(1_420_070_400);
    let server = MockServer::start();
    let by_etag = server.mock(|when, then| {
        when.method("GET").path("/etag").header("If-None-Match", "\"v1\"").header_missing("If-Modified-Since");
        then.status(304);
    });
    let by_date = server.mock(|when, then| {
        when.method("GET")
            .path("/modified")
            .header("If-Modified-Since", rfc1123(modified))
            .header_missing("If-None-Match");
        then.status(304);
    });
    let fs = http_file_source();

    let mut resource = Resource::new(Kind::Unknown, server.url("/etag"));
    resource.prior_etag = Some("\"v1\"".to_owned());
    resource.prior_modified = Some(modified);
    let res = load_resource(&fs, &resource);
    assert_eq!(None, res.error);
    assert!(res.not_modified);
    by_etag.assert_calls(1);

    let mut resource = Resource::new(Kind::Unknown, server.url("/modified"));
    resource.prior_modified = Some(modified);
    let res = load_resource(&fs, &resource);
    assert_eq!(None, res.error);
    assert!(res.not_modified);
    by_date.assert_calls(1);
}

/// A refused connection is a connection error, which the online file
/// source retries with a backoff.
#[test]
fn a_refused_connection_is_a_connection_error() {
    // Port 1 is reserved, and nothing listens on it: a freed ephemeral
    // port could be taken by another test's server before the dial.
    let res = load_once(&http_file_source(), Kind::Unknown, "http://127.0.0.1:1/test".to_owned());
    let error = res.error.expect("error");
    assert_eq!(Reason::Connection, error.reason);
    assert!(res.data.is_none());
}
