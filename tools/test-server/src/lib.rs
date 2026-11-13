//! The HTTP server the tests load from, with the routes of the cpp-httplib
//! server the C++ unit tests start on 127.0.0.1:3000. It runs on an
//! [httpmock](https://github.com/httpmock/httpmock) `MockServer`: each C++
//! `server->Get(path, handler)` is a mock, in the same order.
//!
//! Differences from C++:
//! - Every test starts its own server (`HttpServer::new()`, on a free port;
//!   URLs from `url(path)`) instead of one per process on port 3000, so the
//!   routes' state (`/cache`'s counter, `/temporary-error`,
//!   `/style-fail-once-*`, ...) starts fresh for each test, as when a C++
//!   test runs alone.
//! - A route that sets no status answers 200 (httplib's `res.status == -1`
//!   default), and content is `text/plain`, as with httplib's `set_content`.
//!   `/test` also answers a `Range: bytes=a-b` request header with 206 and
//!   that slice, as httplib does for every route (no other test sends one).
//! - An unknown path is a 404 with an empty body, as in httplib (httpmock's
//!   own answer has a JSON error body).
//! - `/online/<file>` reads `test/fixtures/map/online/<file>` from this
//!   crate's testdata/ (C++: relative to the working directory). Its JSON
//!   files name `http://127.0.0.1:3000/`, the C++ server; they are served
//!   with this server's address instead, and
//!   [`HttpServer::online_fixture`] gives tests the same bytes.

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};
use mln_util::chrono::{Timestamp, now, parse_http_date, parse_timestamp_i32, rfc1123};

/// A route's response as it fills it, like an `httplib::Response`: `status`
/// is -1 until the route sets one.
struct Reply {
    status: i32,
    headers: Vec<(String, String)>,
    content: Option<Vec<u8>>,
}

impl Reply {
    fn new() -> Self {
        Reply { status: -1, headers: Vec::new(), content: None }
    }

    fn set_header(&mut self, name: &str, value: impl Into<String>) {
        self.headers.push((name.to_owned(), value.into()));
    }

    fn set_content(&mut self, content: impl Into<Vec<u8>>) {
        self.content = Some(content.into());
    }

    /// What httplib sends for it: 200 if no status was set, and a 206 slice
    /// for a `Range` header on a 200 with content.
    fn finish(mut self, req: &HttpMockRequest) -> HttpMockResponse {
        if self.status == -1 {
            self.status = 200;
        }
        if self.status == 200
            && let Some(content) = &self.content
            && !content.is_empty()
            && let Some(range) = header(req, "range")
            && let Some((start, end)) = parse_range(&range, content.len())
        {
            let len = content.len();
            self.content = Some(content[start..=end].to_vec());
            self.set_header("Content-Range", format!("bytes {start}-{end}/{len}"));
            self.status = 206;
        }
        let mut response = HttpMockResponse::builder().status(self.status as u16);
        for (name, value) in self.headers {
            response = response.header(name, value);
        }
        if let Some(content) = self.content {
            response = response.header("Content-Type", "text/plain").body(content);
        }
        response.build()
    }
}

/// `req.get_param_value(name)` (`None`: `!req.has_param(name)`), parsed as
/// httplib's `parse_query_text` does: it splits a pair on every '=', the first
/// piece is the key and the last one the value ("cachecontrol=max-age=120"
/// -> "120"), and the first pair with a key wins.
fn param(req: &HttpMockRequest, name: &str) -> Option<String> {
    let uri = req.uri();
    let query = uri.query().unwrap_or("");
    query.split('&').filter(|pair| !pair.is_empty()).find_map(|pair| {
        let mut pieces = pair.split('=');
        let key = pieces.next().unwrap_or("");
        let value = pieces.next_back().unwrap_or("");
        (decode_query_component(key) == name).then(|| decode_query_component(value))
    })
}

/// `%XX` and `+` decoding of a query component (httplib `decode_url(s, true)`).
fn decode_query_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `req.get_header_value(name)` (`None`: `!req.has_header(name)`).
fn header(req: &HttpMockRequest, name: &str) -> Option<String> {
    req.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
}

/// `Range: bytes=a-b` (one range, both ends given or open-ended).
fn parse_range(value: &str, len: usize) -> Option<(usize, usize)> {
    let range = value.trim().strip_prefix("bytes=")?;
    let (start, end) = range.split_once('-')?;
    let start: usize = start.trim().parse().ok()?;
    let end: usize = if end.trim().is_empty() { len.checked_sub(1)? } else { end.trim().parse().ok()? };
    (start <= end && start < len).then_some((start, end.min(len - 1)))
}

/// Serves the routes until dropped.
pub struct HttpServer {
    server: MockServer,
}

impl HttpServer {
    /// Starts a server with the routes, on a free port.
    #[must_use]
    pub fn new() -> Self {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method("GET").path("/test");
            then.respond_with(|req| {
                let mut res = Reply::new();
                let mut content = b"Hello World!".to_vec();
                if let Some(modified) = param(req, "modified") {
                    res.set_header("Last-Modified", rfc1123(parse_timestamp_i32(modified.parse().unwrap_or(0))));
                }
                if let Some(expires) = param(req, "expires") {
                    res.set_header("Expires", rfc1123(parse_timestamp_i32(expires.parse().unwrap_or(0))));
                }
                if let Some(etag) = param(req, "etag") {
                    res.set_header("ETag", etag);
                }
                if let Some(cachecontrol) = param(req, "cachecontrol") {
                    res.set_header("Cache-Control", format!("max-age={cachecontrol}"));
                }
                if let Some(range) = param(req, "range") {
                    let range = &range["bytes=".len().min(range.len())..];
                    let (start, end) = range.split_once('-').unwrap_or((range, ""));
                    let start: usize = start.parse().unwrap_or(0);
                    let end: usize = end.parse().unwrap_or(0);
                    content = content[start.min(content.len())..(end + 1).min(content.len())].to_vec();
                    res.status = 206;
                }
                res.set_content(content);
                res.finish(req)
            });
        });

        server.mock(|when, then| {
            when.method("GET").path("/stale");
            // Never respond.
            then.delay(Duration::from_hours(24));
        });

        let cache_counter = Arc::new(AtomicI32::new(0));
        server.mock(|when, then| {
            when.method("GET").path("/cache");
            then.respond_with(move |req| {
                let mut res = Reply::new();
                res.set_header("Cache-Control", "max-age=30"); // Allow caching for 30 seconds
                let n = cache_counter.fetch_add(1, Ordering::SeqCst) + 1;
                res.set_content(format!("Response {n}"));
                res.finish(req)
            });
        });

        server.mock(|when, then| {
            when.method("GET").path("/revalidate-same");
            then.respond_with(|req| {
                let mut res = Reply::new();
                if header(req, "if-none-match").as_deref() == Some("snowfall") {
                    // Second request can be cached for 1 second.
                    res.set_header("Cache-Control", "max-age=1, must-revalidate");
                    res.status = 304;
                } else {
                    // First request must always be revalidated.
                    res.set_header("ETag", "snowfall");
                    res.set_header("Cache-Control", "must-revalidate");
                    res.status = 200;
                    res.set_content("Response");
                }
                res.finish(req)
            });
        });

        let expired_once = Arc::new(AtomicBool::new(true));
        server.mock(|when, then| {
            when.method("GET").path("/clockskew");
            then.respond_with(move |req| {
                let mut res = Reply::new();
                let date = if expired_once.swap(false, Ordering::SeqCst) {
                    "Fri, 01 Jan 2010 10:00:00 GMT"
                } else {
                    "Fri, 01 Jan 2010 10:01:00 GMT"
                };
                res.set_header("Expires", date);
                res.status = 200;
                res.set_content("Response");
                res.finish(req)
            });
        });

        server.mock(|when, then| {
            when.method("GET").path("/revalidate-modified");
            then.respond_with(|req| {
                let mut res = Reply::new();
                // 2015-01-01 00:00:00 UTC.
                let jan1 = Timestamp::from_secs(1_420_070_400);

                if let Some(modified_since) = header(req, "if-modified-since")
                    && parse_http_date(&modified_since).is_some_and(|modified_since| modified_since >= jan1)
                {
                    res.set_header("Cache-Control", "max-age=1, must-revalidate");
                    res.status = 304;
                    return res.finish(req);
                }

                // First request must always be revalidated.
                res.set_header("Last-Modified", rfc1123(jan1));
                res.set_header("Cache-Control", "must-revalidate");
                res.status = 200;
                res.set_content("Response");
                res.finish(req)
            });
        });

        let revalidate_etag_counter = Arc::new(AtomicI32::new(1));
        server.mock(|when, then| {
            when.method("GET").path("/revalidate-etag");
            then.respond_with(move |req| {
                let mut res = Reply::new();
                let n = revalidate_etag_counter.fetch_add(1, Ordering::SeqCst);
                res.set_header("ETag", format!("response-{n}"));
                res.set_header("Cache-Control", "must-revalidate");

                res.status = 200;
                res.set_content(format!("Response {n}"));
                res.finish(req)
            });
        });

        server.mock(|when, then| {
            when.method("GET").path("/empty-data");
            then.status(200);
        });

        server.mock(|when, then| {
            when.method("GET").path("/no-content");
            then.status(204);
        });

        server.mock(|when, then| {
            when.method("GET").path("/not-found");
            then.status(404).header("Content-Type", "text/plain").body("Not Found!");
        });

        server.mock(|when, then| {
            when.method("GET").path("/permanent-error");
            then.status(500).header("Content-Type", "text/plain").body("Server Error!");
        });

        // The first request fails, the others succeed (`temporaryErrorCounter`).
        server.mock(|when, then| {
            when.method("GET").path("/temporary-error");
            then.status(500).up_to_n_times(1);
        });
        server.mock(|when, then| {
            when.method("GET").path("/temporary-error");
            then.status(200).header("Content-Type", "text/plain").body("Hello World!");
        });

        server.mock(|when, then| {
            when.method("GET").path("/rate-limit");
            then.respond_with(|req| {
                let mut res = Reply::new();
                if param(req, "std").is_some() {
                    res.set_header("Retry-After", "1");
                } else if param(req, "mbx").is_some() {
                    res.set_header("x-rate-limit-reset", (now().secs_since_epoch() + 1).to_string());
                }
                res.status = 429;
                res.finish(req)
            });
        });

        // The `styleFailOnce*` routes: the first request fails, the others
        // get the style.
        for (path, status, cache_control, name) in [
            ("/style-fail-once-500", 500, false, "Test Style"),
            ("/style-fail-once-404", 404, false, "Test Style"),
            ("/style-fail-once-404-cache", 404, true, "Teste Style"),
        ] {
            server.mock(|when, then| {
                when.method("GET").path(path);
                let then = if cache_control { then.header("Cache-Control", "max-age=30") } else { then };
                then.status(status).header("Content-Type", "text/plain").body("Not found!").up_to_n_times(1);
            });
            server.mock(|when, then| {
                when.method("GET").path(path);
                then.status(200)
                    .header("Content-Type", "text/plain")
                    .body(format!(r#"{{ "version": 8, "name": "{name}" }}"#));
            });
        }

        server.mock(|when, then| {
            when.method("GET").path("/delayed");
            then.delay(Duration::from_millis(200)).status(200).header("Content-Type", "text/plain").body("Response");
        });

        server.mock(|when, then| {
            when.method("GET").path_matches(r"^/load/\d+$");
            then.respond_with(|req| {
                let mut res = Reply::new();
                let path = req.uri().path().to_owned();
                res.set_content(format!("Request {}", &path["/load/".len()..]));
                res.finish(req)
            });
        });

        let base = base_url(server.port());
        server.mock(|when, then| {
            when.method("GET").path_prefix("/online/");
            then.respond_with(move |req| {
                let mut res = Reply::new();
                res.status = 200;
                let uri = req.uri();
                res.set_content(online_fixture(&base, &uri.path()["/online/".len()..]));
                res.finish(req)
            });
        });

        // httplib's answer to a path without a route.
        server.mock(|when, then| {
            when.any_request();
            then.status(404);
        });

        HttpServer { server }
    }

    #[must_use]
    pub(crate) fn port(&self) -> u16 {
        self.server.port()
    }

    /// The URL of `path` on this server, `http://127.0.0.1:<port><path>`
    /// (C++: `http://127.0.0.1:3000<path>`).
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port())
    }

    /// The bytes of `test/fixtures/map/online/<file>` as `/online/` serves
    /// it, with this server's address in the JSON files.
    #[must_use]
    pub fn online_fixture(&self, file: &str) -> Vec<u8> {
        online_fixture(&base_url(self.port()), file)
    }
}

impl Default for HttpServer {
    fn default() -> Self {
        Self::new()
    }
}

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// The bytes of `test/fixtures/map/online/<file>`, with `base` for the C++
/// server's address in the JSON files. Empty for a missing file, where C++'s
/// `util::read_file` throws; the route then answers with no content.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the JSON fixtures end in a lower-case `.json`; other names are served as they are"
)]
fn online_fixture(base: &str, file: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test/fixtures/map/online").join(file);
    let content = std::fs::read(path).unwrap_or_default();
    if file.ends_with(".json") {
        String::from_utf8_lossy(&content).replace("http://127.0.0.1:3000/", base).into_bytes()
    } else {
        content
    }
}

/// A URL on a port nothing listens on (C++ uses 127.0.0.1:3001).
#[must_use]
pub fn closed_port_url(path: &str) -> String {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("local address").port()
    };
    format!("http://127.0.0.1:{port}{path}")
}
