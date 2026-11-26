// The file source hands each request to its `HttpTransport`, which finishes
// it from whatever thread the transfer ends on, and the response goes to the
// request's responder. Dropping the request cancels it: the transport's
// cancel handler (`TransferRequest::set_cancel_handler`) stops an in-flight
// transfer if it has one.
//
// Request headers as in C++: `Range` for data ranges, `If-None-Match` from
// the prior etag, else `If-Modified-Since` from the prior modification date,
// user agent `USER_AGENT`, redirects followed. A tile or image
// request also sends an `Accept` naming what it decodes as (`accept_header`),
// so that a server that serves a tileset as either MVT or MLT, such as
// Martin, sends the format the source's `encoding` names. Then the host's
// headers from the `HttpHeaderTransform`, which a redirect to another origin
// drops; a host's `Accept` replaces the file source's. Response headers are
// read with case-insensitive names: `Last-Modified`, `ETag`, `Retry-After`,
// `X-Rate-Limit-Reset`, and the caching headers, which give the response's
// caching rules together (`CachingHeaders::freshness`) whatever their order:
// the `Cache-Control` field lines' directives, `Expires`, `Date` and `Age`.
// A vector tile whose `Content-Type` names the other vector format logs a
// warning and decodes as its tileset's format all the same. A 408 is a
// connection error, which is retried, and a 5xx takes its `Retry-After` as a
// 429 does.
//
// The transfer itself is an `HttpTransport`, the platform's HTTP stack where
// it has one: NSURLSession on Apple platforms, HttpURLConnection on Android,
// ureq elsewhere. In the browser the source calls fetch itself
// (`fetch_transport`), with the host's preloads, on the thread that owns them.
//
// ureq sends `Accept-Encoding: gzip` and decodes gzip. Dates are read in the
// three formats of RFC 9110 only.

#[cfg(not(target_family = "wasm"))]
use std::str::FromStr;
use std::sync::Arc;

use mln_util::chrono::{now, parse_http_date};
use mln_util::http_header::{CacheControl, CachingHeaders, parse_retry_headers};

#[cfg(not(target_family = "wasm"))]
use crate::file_source::FileSource;
use crate::http_header_transform::{HttpHeaderTransform, HttpHeaders};
use crate::log_target;
use crate::request::Responder;
use crate::resource::{Kind, Resource, TileFormat};
use crate::response::{Caching, Reason, ResourceError, Response};

/// The `User-Agent` of every network request
#[cfg(not(target_family = "wasm"))]
pub(crate) const USER_AGENT: &str = concat!("mln-rs/", env!("CARGO_PKG_VERSION"));

/// The value of `line` if it is the header `name`, matched as an
/// ASCII-case-insensitive prefix.
fn header_value<'a>(name: &str, header: &str, line: &'a str) -> Option<&'a str> {
    (name.eq_ignore_ascii_case(header)).then_some(line)
}

/// What a transport hands back: the status code, the response headers in
/// order, and the body, which only a 200 or 206 reads.
pub(crate) struct Transfer {
    pub(crate) status: i64,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Result<Vec<u8>, Box<ResourceError>>,
    /// Whether the response is a stored copy of unknown age, which the
    /// transport fell back on because the server couldn't be reached. It
    /// counts as expired, so the requester checks it with the server again.
    pub(crate) stale: bool,
}

/// A request an `HTTPFileSource` hands to its transport: the resource to
/// fetch, the host's headers, and where its response goes.
pub(crate) struct TransferRequest {
    resource: Resource,
    headers: HttpHeaders,
    responder: Responder,
}

impl TransferRequest {
    #[must_use]
    pub(crate) fn resource(&self) -> &Resource {
        &self.resource
    }

    /// The host's headers (`HttpHeaderTransform`), which the transport sends
    /// besides its own, and drops when a redirect leaves the origin of the
    /// URL that sent them. A transport that can't drop them fails a
    /// redirect instead of following it.
    #[must_use]
    pub(crate) fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Whether the request was dropped, so that nobody waits for the
    /// transfer any more: a transport can skip it.
    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.responder.is_cancelled()
    }

    /// Sets what stops the transfer when the request is dropped before it
    /// finishes, such as cancelling the HTTP stack's task. It runs once, on
    /// the thread that drops the request, or right away if the request is
    /// already dropped; `finish` discards it.
    pub(crate) fn set_cancel_handler(&self, handler: impl FnOnce() + Send + 'static) {
        self.responder.on_cancel(handler);
    }

    /// Finishes the request with the transfer's result (a transfer that
    /// failed before a response, such as a connection error, is an `Err`):
    /// builds the response, as the header callback and
    /// `handleResult(CURLcode)` do, and sends it to the request's callback,
    /// unless the request was dropped. Any thread may call it.
    pub(crate) fn finish(self, transfer: Result<Transfer, Box<ResourceError>>) {
        self.responder.on_cancel(|| {});
        if !self.is_cancelled() {
            self.responder.send(build_response(&self.resource, transfer));
        }
    }
}

#[cfg(all(test, not(any(target_family = "wasm", target_vendor = "apple", target_os = "android"))))]
impl TransferRequest {
    /// A request for `resource` whose response goes nowhere, until the
    /// returned request drops (tests).
    pub(crate) fn unanswered(resource: Resource) -> (TransferRequest, crate::request::Request) {
        Self::answered(resource, |_| {})
    }

    /// A request for `resource` whose response goes to `callback`, until
    /// the returned request drops (tests).
    pub(crate) fn answered(
        resource: Resource,
        callback: impl FnMut(Response) + Send + 'static,
    ) -> (TransferRequest, crate::request::Request) {
        let (request, responder) = crate::request::Request::new(callback);
        (TransferRequest { resource, headers: HttpHeaders::new(), responder }, request)
    }
}

/// How an `HTTPFileSource` performs its requests outside the browser, whose
/// fetch it calls itself.
#[cfg(not(target_family = "wasm"))]
pub(crate) trait HttpTransport: Send + Sync {
    /// Starts the transfer of `request` without blocking the caller, and
    /// calls `request.finish` once it is done, from any thread (ureq: one
    /// of its worker threads; `NSURLSession`: its delegate queue;
    /// `HttpURLConnection`: one of the transport's worker threads). A request
    /// dropped without `finish` never answers, as if it had been cancelled.
    fn start(&self, request: TransferRequest);
}

#[cfg(target_vendor = "apple")]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::nsurlsession_transport::NSURLSessionTransport::new())
}

#[cfg(target_os = "android")]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::http_url_connection_transport::Transport::new())
}

#[cfg(not(any(target_family = "wasm", target_vendor = "apple", target_os = "android")))]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::ureq_transport::Transport::new())
}

/// The response half of `HTTPRequest`, from a finished transfer: the header
/// callback and `handleResult(CURLcode)`.
fn build_response(resource: &Resource, transfer: Result<Transfer, Box<ResourceError>>) -> Response {
    let mut response = Response::new();
    let transfer = match transfer {
        Ok(transfer) => transfer,
        Err(error) => {
            response.error = Some(error);
            return response;
        }
    };

    let mut retry_after: Option<String> = None;
    let mut x_rate_limit_reset: Option<String> = None;
    let mut cache_control: Option<String> = None;
    let mut expires: Option<&str> = None;
    let mut date = None;
    let mut age = None;
    let mut content_type = None;
    for (name, value) in &transfer.headers {
        let (name, value) = (name.as_str(), value.as_str());
        if let Some(value) = header_value(name, "last-modified", value) {
            response.modified = parse_http_date(value);
        } else if let Some(value) = header_value(name, "etag", value) {
            response.etag = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "cache-control", value) {
            // A field's lines are one comma-separated list (RFC 9110 §5.3).
            match &mut cache_control {
                Some(list) => {
                    list.push(',');
                    list.push_str(value);
                }
                None => cache_control = Some(value.to_owned()),
            }
        } else if let Some(value) = header_value(name, "expires", value) {
            expires = Some(value);
        } else if let Some(value) = header_value(name, "date", value) {
            date = parse_http_date(value);
        } else if let Some(value) = header_value(name, "age", value) {
            // Delta-seconds too large to hold are as old as can be.
            let value = value.trim();
            age = (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
                .then(|| value.parse::<u64>().unwrap_or(u64::MAX));
        } else if let Some(value) = header_value(name, "retry-after", value) {
            retry_after = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "x-rate-limit-reset", value) {
            x_rate_limit_reset = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "content-type", value) {
            content_type = Some(value);
        }
    }
    if matches!(transfer.status, 200 | 206)
        && let Some(content_type) = content_type
    {
        warn_of_other_vector_format(resource, content_type);
    }

    // A response the cache may keep, and a 304 that renews the cached copy,
    // carry caching rules; an error doesn't.
    let not_modified = transfer.status == 304;
    if not_modified && cache_control.is_none() && expires.is_none() && resource.prior_caching.must_revalidate {
        // A 304 carries the caching headers its 200 would (RFC 9110
        // §15.4.5), so without them the 200 had none either, and the
        // heuristic below applies. A copy that must revalidate, though, came
        // with `Cache-Control`: it stays one, stale at once.
        response.caching = Caching { expires: Some(now()), must_revalidate: true };
    } else if matches!(transfer.status, 200 | 204 | 206 | 304)
        || (transfer.status == 404 && resource.kind == Kind::Tile)
    {
        let cache_control = cache_control.as_deref().map(CacheControl::parse).unwrap_or_default();
        // A 304's heuristic counts from when the copy it renews changed.
        let last_modified =
            if not_modified { response.modified.or(resource.prior_modified) } else { response.modified };
        let headers = CachingHeaders { cache_control: &cache_control, expires, date, age, last_modified };
        let freshness = headers.freshness(now());
        response.caching = Caching { expires: Some(freshness.expires), must_revalidate: freshness.must_revalidate };
        response.no_store = freshness.no_store;
    }
    if transfer.stale {
        let now = now();
        response.caching.expires = Some(response.caching.expires.map_or(now, |expires| expires.min(now)));
    }

    let response_code = transfer.status;
    if response_code == 200 || response_code == 206 {
        match transfer.body {
            Ok(data) => response.data = Some(Arc::new(data)),
            // curl reports a failed transfer as an error code, not a status.
            Err(error) => response.error = Some(error),
        }
    } else if response_code == 204 {
        response.no_content = true;
    } else if response_code == 404 && resource.kind == Kind::Tile {
        response.no_content = true;
        response.tile_not_found = true;
    } else if response_code == 304 {
        response.not_modified = true;
    } else if response_code == 404 {
        response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "HTTP status code 404")));
    } else if response_code == 429 {
        response.error = Some(Box::new(ResourceError::with_retry_after(
            Reason::RateLimit,
            "HTTP status code 429",
            parse_retry_headers(&retry_after, &x_rate_limit_reset),
        )));
    } else if response_code == 408 {
        // The server timed out waiting for the request, which may be sent again.
        response.error = Some(Box::new(ResourceError::new(Reason::Connection, "HTTP status code 408")));
    } else if (500..600).contains(&response_code) {
        // A 503's `Retry-After` says how long the server expects to be unavailable.
        response.error = Some(Box::new(ResourceError::with_retry_after(
            Reason::Server,
            format!("HTTP status code {response_code}"),
            parse_retry_headers(&retry_after, &None),
        )));
    } else {
        response.error = Some(Box::new(ResourceError::new(Reason::Other, format!("HTTP status code {response_code}"))));
    }
    response
}

/// The `Range` request header for a resource's data range, for transports
/// (`TransferRequest::resource`).
#[must_use]
pub(crate) fn range_header(resource: &Resource) -> Option<String> {
    resource.data_range.map(|(first, second)| format!("bytes={first}-{second}"))
}

/// The `Accept` request header of a request for `resource` that sends the
/// host's `headers`: the media types of what the resource decodes as, then,
/// less wanted, any other, so that a server that offers none of them doesn't
/// refuse the request. `None` for a resource that decodes whatever comes, or
/// when the host's headers set their own `Accept`. Each value is one that a
/// browser's fetch sends without a CORS preflight.
#[must_use]
pub(crate) fn accept_header(resource: &Resource, headers: &[(String, String)]) -> Option<&'static str> {
    if headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("accept")) {
        return None;
    }
    let format = match resource.kind {
        Kind::Tile => resource.tile_data.as_ref()?.format,
        Kind::Image | Kind::SpriteImage => TileFormat::Image,
        Kind::Unknown | Kind::Style | Kind::Source | Kind::Glyphs | Kind::SpriteJSON => return None,
    };
    Some(match format {
        TileFormat::Mvt => "application/vnd.mapbox-vector-tile, application/x-protobuf, */*;q=0.1",
        TileFormat::Mlt => "application/vnd.maplibre-tile, */*;q=0.1",
        TileFormat::Image => "image/png, image/webp, image/jpeg, */*;q=0.1",
    })
}

/// The vector tile format that a `Content-Type` value names, if it names one.
fn vector_tile_format(content_type: &str) -> Option<TileFormat> {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    let is = |media_type: &str| essence.eq_ignore_ascii_case(media_type);
    if is("application/vnd.maplibre-tile") || is("application/vnd.maplibre-vector-tile") {
        Some(TileFormat::Mlt)
    } else if is("application/vnd.mapbox-vector-tile") || is("application/x-protobuf") {
        Some(TileFormat::Mvt)
    } else {
        None
    }
}

/// Warns when `resource` is a vector tile and its response's `content_type`
/// names the other vector format: the source's `encoding` is likely wrong,
/// or the server ignored the `Accept` header. The tile still decodes as its
/// tileset's format.
fn warn_of_other_vector_format(resource: &Resource, content_type: &str) {
    let Some(expected) = resource.tile_data.as_ref().map(|tile| tile.format) else {
        return;
    };
    let name = match expected {
        TileFormat::Mvt => "MVT",
        TileFormat::Mlt => "MLT",
        TileFormat::Image => return,
    };
    if vector_tile_format(content_type).is_some_and(|sent| sent != expected) {
        log::warn!(
            target: log_target::HTTP_REQUEST,
            "{}: the tileset's tiles are {name}, but the response is {content_type}",
            resource.url
        );
    }
}

/// URL scheme for HTTP(S) requests.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UrlScheme {
    Http,
    Https,
}

#[cfg(not(target_family = "wasm"))]
impl FromStr for UrlScheme {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            _ => Err(()),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl UrlScheme {
    fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

/// Whether two absolute `http` or `https` URLs have the same origin: scheme,
/// host and port, the scheme's default port when there's none, ignoring
/// letter case. False when either isn't such a URL.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn same_origin(left: &str, right: &str) -> bool {
    matches!((origin(left), origin(right)), (Some(left), Some(right)) if left == right)
}

/// The lowercased scheme and host, and the port, of an `http` or `https` URL.
#[cfg(not(target_family = "wasm"))]
fn origin(url: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme_lower = scheme.to_ascii_lowercase();
    let url_scheme = scheme_lower.parse::<UrlScheme>().ok()?;
    let default_port = url_scheme.default_port();
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host_and_port = authority.rsplit_once('@').map_or(authority, |(_, host_and_port)| host_and_port);
    let (host, port) = match host_and_port.strip_prefix('[') {
        // An IPv6 address.
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match host_and_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_and_port, None),
        },
    };
    let port = match port {
        None | Some("") => default_port,
        Some(port) => port.parse().ok()?,
    };
    (!host.is_empty()).then(|| (scheme_lower, host.to_ascii_lowercase(), port))
}

/// Plain HTTP(S) requests, used by the `OnlineFileSource`.
pub(crate) struct HTTPFileSource {
    /// The platform's HTTP stack, set up at the first transfer. Setting one
    /// up isn't free (`NSURLSession` reads the main bundle's directory, ureq
    /// starts its threads), and many sources never transfer: a host's
    /// provider or the cache can serve everything.
    #[cfg(not(target_family = "wasm"))]
    transport: std::sync::OnceLock<Arc<dyn HttpTransport>>,
    /// The browser's fetch, which is the transport.
    #[cfg(target_family = "wasm")]
    fetch: crate::fetch_transport::FetchTransport,
    header_transform: HttpHeaderTransform,
}

impl HTTPFileSource {
    /// An HTTP file source over the platform's HTTP stack, which it sets up
    /// at its first transfer.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub(crate) fn new() -> Self {
        HTTPFileSource { transport: std::sync::OnceLock::new(), header_transform: HttpHeaderTransform::default() }
    }

    /// An HTTP file source over `transport` (`crate::simulated_network`).
    #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
    #[must_use]
    pub(crate) fn with_transport(transport: Arc<dyn HttpTransport>) -> Self {
        HTTPFileSource {
            transport: std::sync::OnceLock::from(transport),
            header_transform: HttpHeaderTransform::default(),
        }
    }

    /// An HTTP file source over the browser's fetch.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub(crate) fn new() -> Self {
        HTTPFileSource {
            fetch: crate::fetch_transport::FetchTransport::default(),
            header_transform: HttpHeaderTransform::default(),
        }
    }

    /// Sets the responses that the host fetched ahead (`FetchTransport`).
    #[cfg(target_family = "wasm")]
    pub(crate) fn set_fetch_preloads(&mut self, take: Option<js_sys::Function>) {
        self.fetch.set_preloads(take);
    }

    /// Sets the headers to add to requests.
    pub(crate) fn set_http_header_transform(&mut self, transform: HttpHeaderTransform) {
        self.header_transform = transform;
    }

    /// Starts the transfer of `resource`, whose response goes to
    /// `responder`, without blocking.
    pub(crate) fn start(&self, resource: &Resource, responder: Responder) {
        let headers = self.header_transform.headers(resource.kind, &resource.url);
        let request = TransferRequest { resource: resource.clone(), headers, responder };
        #[cfg(not(target_family = "wasm"))]
        self.transport.get_or_init(default_transport).start(request);
        #[cfg(target_family = "wasm")]
        self.fetch.start(request);
    }
}

#[cfg(not(target_family = "wasm"))]
impl FileSource for HTTPFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.start(resource, responder);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use mln_util::chrono::{Timestamp, now, rfc1123};

    use super::*;

    fn response(status: i64, headers: &[(&str, &str)], resource: &Resource) -> Response {
        let transfer = Transfer {
            status,
            headers: headers.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect(),
            body: Ok(b"tile".to_vec()),
            stale: false,
        };
        build_response(resource, Ok(transfer))
    }

    #[test]
    fn a_stored_copy_of_unknown_age_counts_as_expired() {
        let resource = Resource::new(Kind::Tile, "http://example.com/tile.pbf".to_owned());
        let headers = &[("Cache-Control", "max-age=43200")];
        let before = now();
        let fresh = response(200, headers, &resource);
        assert!(fresh.caching.expires.is_some_and(|expires| expires > before));
        let transfer = Transfer {
            status: 200,
            headers: vec![("Cache-Control".to_owned(), "max-age=43200".to_owned())],
            body: Ok(b"tile".to_vec()),
            stale: true,
        };
        let stale = build_response(&resource, Ok(transfer));
        assert!(stale.caching.expires.is_some_and(|expires| expires <= now()));
        assert!(!stale.caching.must_revalidate);
        assert!(stale.data.is_some() && stale.error.is_none());
    }

    fn tile() -> Resource {
        Resource::new(Kind::Tile, "https://example.com/0/0/0.png")
    }

    fn tile_of(format: TileFormat) -> Resource {
        let scheme = mln_style::tileset::Scheme::XYZ;
        Resource::tile("https://example.com/{z}/{x}/{y}", 1.0, 0, 0, 0, scheme, format, crate::LoadingMethod::All)
    }

    #[test]
    fn tiles_and_images_accept_what_they_decode_as_first() {
        let accept = |resource: &Resource| accept_header(resource, &[]);
        assert_eq!(
            Some("application/vnd.mapbox-vector-tile, application/x-protobuf, */*;q=0.1"),
            accept(&tile_of(TileFormat::Mvt))
        );
        assert_eq!(Some("application/vnd.maplibre-tile, */*;q=0.1"), accept(&tile_of(TileFormat::Mlt)));
        let images = Some("image/png, image/webp, image/jpeg, */*;q=0.1");
        assert_eq!(images, accept(&tile_of(TileFormat::Image)));
        assert_eq!(images, accept(&Resource::image("https://example.com/image")));
        assert_eq!(images, accept(&Resource::sprite_image("https://example.com/sprite", 1.0)));
        assert_eq!(None, accept(&Resource::style("https://example.com/style.json")));
        assert_eq!(None, accept(&Resource::sprite_json("https://example.com/sprite", 1.0)));
    }

    #[test]
    fn a_hosts_accept_replaces_the_file_sources() {
        let headers = [("accept".to_owned(), "application/x-custom".to_owned())];
        assert_eq!(None, accept_header(&tile_of(TileFormat::Mlt), &headers));
        let other = [("X-Token".to_owned(), "a".to_owned())];
        assert_eq!(Some("application/vnd.maplibre-tile, */*;q=0.1"), accept_header(&tile_of(TileFormat::Mlt), &other));
    }

    /// A browser preflights a cross-origin request whose `Accept` isn't
    /// CORS-safelisted (Fetch standard, "CORS-safelisted request-header").
    #[test]
    fn every_accept_is_cors_safelisted() {
        let resources = [
            tile_of(TileFormat::Mvt),
            tile_of(TileFormat::Mlt),
            tile_of(TileFormat::Image),
            Resource::image("https://example.com/image"),
        ];
        for resource in &resources {
            let accept = accept_header(resource, &[]).unwrap();
            let unsafe_byte = |b: u8| (b < 0x20 && b != b'\t') || b == 0x7f || b"\"():<>?@[\\]{}".contains(&b);
            assert!(accept.len() <= 128 && !accept.bytes().any(unsafe_byte), "{accept}");
        }
    }

    #[test]
    fn content_types_name_vector_tile_formats() {
        assert_eq!(Some(TileFormat::Mlt), vector_tile_format("application/vnd.maplibre-tile"));
        assert_eq!(Some(TileFormat::Mlt), vector_tile_format("application/vnd.maplibre-vector-tile"));
        assert_eq!(Some(TileFormat::Mvt), vector_tile_format("Application/X-Protobuf; charset=binary"));
        assert_eq!(Some(TileFormat::Mvt), vector_tile_format("application/vnd.mapbox-vector-tile"));
        assert_eq!(None, vector_tile_format("application/octet-stream"));
    }

    /// The HTTP request messages logged while `responses` builds responses.
    fn logged(responses: impl FnOnce()) -> Vec<String> {
        let log = crate::tests::FixtureLog::new();
        responses();
        let records = log.unchecked().into_iter();
        records.filter(|record| record.target == log_target::HTTP_REQUEST).map(|record| record.msg).collect()
    }

    #[test]
    fn a_vector_tile_of_the_other_format_warns_and_still_delivers() {
        let mlt = tile_of(TileFormat::Mlt);
        let messages = logged(|| {
            let res = response(200, &[("Content-Type", "application/x-protobuf")], &mlt);
            assert!(res.data.is_some() && res.error.is_none());
        });
        assert_eq!(
            vec!["https://example.com/0/0/0: the tileset's tiles are MLT, but the response is application/x-protobuf"],
            messages
        );
    }

    #[test]
    fn a_vector_tile_of_its_own_or_no_named_format_doesnt_warn() {
        let messages = logged(|| {
            response(200, &[("Content-Type", "application/vnd.maplibre-tile")], &tile_of(TileFormat::Mlt));
            response(200, &[("Content-Type", "application/octet-stream")], &tile_of(TileFormat::Mlt));
            response(200, &[("Content-Type", "application/x-protobuf")], &tile_of(TileFormat::Mvt));
            response(200, &[("Content-Type", "application/x-protobuf")], &tile_of(TileFormat::Image));
            response(404, &[("Content-Type", "application/x-protobuf")], &tile_of(TileFormat::Mlt));
        });
        assert!(messages.is_empty(), "{messages:?}");
    }

    /// Seconds from now until the response expires.
    fn lifetime(res: &Response) -> i64 {
        res.caching.expires.expect("expires").secs_since_epoch() - now().secs_since_epoch()
    }

    fn assert_about(expected: i64, actual: i64) {
        assert!((actual - expected).abs() <= 1, "{actual}s, not about {expected}s");
    }

    // The servers' headers, as tile.openstreetmap.org, tiles.openfreemap.org
    // and demotiles.maplibre.org sent them.

    #[test]
    fn max_age_wins_over_an_expires_after_it_and_counts_from_the_age() {
        let res = response(
            200,
            &[
                ("ETag", "\"a6d2\""),
                ("Cache-Control", "max-age=88823, stale-while-revalidate=604800, stale-if-error=604800"),
                ("Expires", "Wed, 18 Nov 2026 02:37:39 GMT"),
                ("Age", "67981"),
            ],
            &tile(),
        );
        assert_about(88823 - 67981, lifetime(&res));
        assert!(!res.caching.must_revalidate);
    }

    #[test]
    fn max_age_wins_over_an_expires_before_it() {
        // NSURLSession's order on Apple platforms.
        let res = response(
            200,
            &[("Cache-Control", "max-age=600"), ("Expires", "Fri, 20 Nov 2026 22:59:19 GMT"), ("Age", "530")],
            &tile(),
        );
        assert_about(70, lifetime(&res));
        let res = response(
            200,
            &[("Expires", "Fri, 20 Nov 2026 22:59:19 GMT"), ("Cache-Control", "max-age=600"), ("Age", "530")],
            &tile(),
        );
        assert_about(70, lifetime(&res));
    }

    #[test]
    fn cache_control_lines_are_one_list() {
        let res = response(
            200,
            &[
                ("Expires", "Sat, 21 Nov 2026 00:21:36 GMT"),
                ("Cache-Control", "max-age=3600"),
                ("cache-control", "public"),
                ("Age", "1134"),
            ],
            &tile(),
        );
        assert_about(3600 - 1134, lifetime(&res));
    }

    #[test]
    fn expires_counts_from_the_date() {
        let date = Timestamp::from_secs(1_000_000_000);
        let expires = Timestamp::from_secs(1_000_000_300);
        let res = response(200, &[("Date", &rfc1123(date)), ("Expires", &rfc1123(expires))], &tile());
        assert_about(300, lifetime(&res));
    }

    #[test]
    fn no_cache_must_revalidate_and_no_store_isnt_stored() {
        let res = response(200, &[("Cache-Control", "no-cache, max-age=600")], &tile());
        assert!(res.caching.must_revalidate && lifetime(&res) <= 0);
        assert!(response(200, &[("Cache-Control", "no-store")], &tile()).no_store);
    }

    #[test]
    fn a_304_renews_the_copy_with_a_heuristic_from_its_modified_date() {
        let mut resource = tile();
        resource.prior_modified = Some(Timestamp::from_secs(now().secs_since_epoch() - 1000));
        let res = response(304, &[("Cache-Control", "public")], &resource);
        assert!(res.not_modified);
        assert_about(100, lifetime(&res));
        // A 200 without `Last-Modified` gets no heuristic from the copy's.
        assert!(lifetime(&response(200, &[("Cache-Control", "public")], &resource)) <= 0);
    }

    #[test]
    fn a_304_without_caching_headers_keeps_must_revalidate_and_is_stale() {
        let mut resource = tile();
        resource.prior_modified = Some(Timestamp::from_secs(now().secs_since_epoch() - 1000));
        resource.prior_caching.must_revalidate = true;
        let res = response(304, &[("ETag", "\"a\"")], &resource);
        assert!(res.caching.must_revalidate);
        assert!(lifetime(&res) <= 0);
    }

    #[test]
    fn a_304_without_caching_headers_gets_the_heuristic_lifetime() {
        let mut resource = tile();
        resource.prior_modified = Some(Timestamp::from_secs(now().secs_since_epoch() - 1000));
        let res = response(304, &[("ETag", "\"a\"")], &resource);
        assert!(!res.caching.must_revalidate);
        assert_about(100, lifetime(&res));
    }

    #[test]
    fn an_age_too_large_to_hold_is_as_old_as_can_be() {
        let res = response(200, &[("Cache-Control", "max-age=600"), ("Age", "99999999999999999999999")], &tile());
        assert!(lifetime(&res) <= 0);
    }

    #[test]
    fn an_error_has_no_caching_rules() {
        let res = response(503, &[("Cache-Control", "max-age=600")], &tile());
        assert!(res.error.is_some());
        assert_eq!(None, res.caching.expires);
    }
}
