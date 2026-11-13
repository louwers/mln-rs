// C++ drives a curl multi handle from the requesting thread's RunLoop (socket
// watches and a timer). Here the file source hands each request to its
// `HttpTransport`, which finishes it from whatever thread the transfer ends
// on, and the result goes back to the requesting thread through a mailbox,
// where `handleResult` runs the callback as in C++. Dropping the request
// cancels it: the mailbox closes, so the callback never runs, and the
// transport's cancel handler (`TransferRequest::set_cancel_handler`) stops
// an in-flight transfer if it has one.
//
// Request headers as in C++: `Range` for data ranges, `If-None-Match` from
// the prior etag, else `If-Modified-Since` from the prior modification date,
// user agent "MapLibreNative/1.0", redirects followed. Then the host's
// headers from the `HttpHeaderTransform`, which a redirect to another origin
// drops, as maplibre-native-ffi's HTTP file sources do. Response headers are
// read in order with curl's header callback rules (case-insensitive names;
// `Last-Modified`/`Expires` as HTTP dates, `Cache-Control`,
// `ETag`, `Retry-After`, `X-Rate-Limit-Reset`). Status codes map to the same
// Response fields and errors.
//
// The transfer itself is an `HttpTransport`, the platform's HTTP stack where
// it has one: NSURLSession on Apple platforms, the browser's fetch on wasm,
// ureq elsewhere.
//
// Differences (ureq): ureq sends `Accept-Encoding: gzip` and
// decodes gzip (curl: "gzip, deflate"); a connection failure's message is
// ureq's text instead of curl's `curl_easy_strerror: errorbuffer`; dates are
// read in the three formats of RFC 9110 only, where libcurl's `curl_getdate`
// also takes other zones and free-form dates.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_util::chrono::parse_http_date;
use mln_util::http_header::{CacheControl, parse_retry_headers};
use parking_lot::Mutex;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::http_header_transform::{HttpHeaderTransform, HttpHeaders};
use crate::resource::{Kind, Resource};
use crate::resource_options::ResourceOptions;
use crate::response::{Reason, ResourceError, Response};

/// The part of `HTTPRequest` that lives on the requesting thread.
pub(crate) struct HTTPRequestState {
    callback: RefCell<Callback>,
}

impl HTTPRequestState {
    /// The callback half of `handleResult(CURLcode)`.
    fn handle_result(&self, response: Response) {
        // Calling `callback` may result in deleting `this`; the mailbox keeps
        // the state alive while this message runs.
        (self.callback.borrow_mut())(response);
    }
}

/// `HTTPRequest`
struct HTTPRequest {
    mailbox: Arc<Mailbox>,
    cancellation: Arc<Cancellation>,
}

impl AsyncRequest for HTTPRequest {}

impl Drop for HTTPRequest {
    fn drop(&mut self) {
        self.mailbox.close();
        self.cancellation.cancel();
    }
}

/// Whether a request was dropped, and what stops its transfer then.
#[derive(Default)]
struct Cancellation {
    cancelled: AtomicBool,
    handler: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Cancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let handler = self.handler.lock().take();
        if let Some(handler) = handler {
            handler();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    #[cfg(target_vendor = "apple")]
    fn set_handler(&self, handler: Box<dyn FnOnce() + Send>) {
        *self.handler.lock() = Some(handler);
        // `cancel` may have run before the handler was there.
        if self.is_cancelled() {
            let handler = self.handler.lock().take();
            if let Some(handler) = handler {
                handler();
            }
        }
    }

    fn clear_handler(&self) {
        drop(self.handler.lock().take());
    }
}

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
}

/// A request an `HTTPFileSource` hands to its transport: the resource to
/// fetch, the host's headers, and where its response goes.
pub(crate) struct TransferRequest {
    resource: Resource,
    headers: HttpHeaders,
    reply: ActorRef<HTTPRequestState>,
    cancellation: Arc<Cancellation>,
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
        self.cancellation.is_cancelled()
    }

    /// Sets what stops the transfer when the request is dropped before it
    /// finishes, such as cancelling the HTTP stack's task. It runs once, on
    /// the thread that drops the request, or right away if the request is
    /// already dropped; `finish` discards it.
    #[cfg(target_vendor = "apple")]
    pub(crate) fn set_cancel_handler(&self, handler: impl FnOnce() + Send + 'static) {
        self.cancellation.set_handler(Box::new(handler));
    }

    /// Finishes the request with the transfer's result (a transfer that
    /// failed before a response, such as a connection error, is an `Err`):
    /// builds the response, as the header callback and
    /// `handleResult(CURLcode)` do, and hands it to the requesting thread,
    /// unless the request was dropped. Any thread may call it.
    pub(crate) fn finish(self, transfer: Result<Transfer, Box<ResourceError>>) {
        self.cancellation.clear_handler();
        let response = build_response(&self.resource, transfer);
        if !self.is_cancelled() {
            self.reply.invoke(move |request: &HTTPRequestState| request.handle_result(response));
        }
    }
}

/// How an `HTTPFileSource` performs its requests.
pub(crate) trait HttpTransport: Send + Sync {
    /// Starts the transfer of `request` without blocking the caller, and
    /// calls `request.finish` once it is done, from any thread (ureq: one
    /// of its worker threads; `NSURLSession`: its delegate queue; a browser's
    /// fetch: the event loop). A request
    /// dropped without `finish` never answers, as if it had been cancelled.
    fn start(&self, request: TransferRequest);
}

#[cfg(target_vendor = "apple")]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::nsurlsession_transport::NSURLSessionTransport::new())
}

#[cfg(not(any(target_family = "wasm", target_vendor = "apple")))]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::ureq_transport::Transport::new())
}

#[cfg(target_family = "wasm")]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(crate::fetch_transport::FetchTransport)
}

/// The response half of `HTTPRequest`, from a finished transfer: the header
/// callback and `handleResult(CURLcode)`.
fn build_response(resource: &Resource, transfer: Result<Transfer, Box<ResourceError>>) -> Response {
    // Make sure a response object exists in case we haven't got any headers or content.
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
    for (name, value) in &transfer.headers {
        let (name, value) = (name.as_str(), value.as_str());
        if let Some(value) = header_value(name, "last-modified", value) {
            // Always overwrite the modification date; We might already have
            // a value here from the Date header, but this one is more accurate.
            response.modified = parse_http_date(value);
        } else if let Some(value) = header_value(name, "etag", value) {
            response.etag = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "cache-control", value) {
            let cc = CacheControl::parse(value);
            response.expires = cc.to_time_point();
            response.must_revalidate = cc.must_revalidate;
        } else if let Some(value) = header_value(name, "expires", value) {
            // An invalid date, such as "0", means already expired (RFC 9111 §5.3).
            response.expires = Some(parse_http_date(value).unwrap_or_default());
        } else if let Some(value) = header_value(name, "retry-after", value) {
            retry_after = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "x-rate-limit-reset", value) {
            x_rate_limit_reset = Some(value.to_owned());
        }
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
    } else if (500..600).contains(&response_code) {
        response.error =
            Some(Box::new(ResourceError::new(Reason::Server, format!("HTTP status code {response_code}"))));
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
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
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
    (!host.is_empty()).then(|| (scheme, host.to_ascii_lowercase(), port))
}

/// Plain HTTP(S) requests, used by the `OnlineFileSource`.
pub(crate) struct HTTPFileSource {
    transport: Arc<dyn HttpTransport>,
    header_transform: Mutex<HttpHeaderTransform>,
    resource_options: Mutex<ResourceOptions>,
}

impl HTTPFileSource {
    /// An HTTP file source over the platform's HTTP stack.
    #[must_use]
    pub(crate) fn new(resource_options: &ResourceOptions) -> Self {
        HTTPFileSource {
            transport: default_transport(),
            header_transform: Mutex::new(HttpHeaderTransform::default()),
            resource_options: Mutex::new(resource_options.clone()),
        }
    }

    /// `request(const Resource&, Callback)`. The callback runs on the
    /// calling thread's `RunLoop`.
    pub(crate) fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let state = Rc::new(HTTPRequestState { callback: RefCell::new(callback) });
        let mailbox = Mailbox::with_target(&scheduler::get_current(), state);
        let cancellation = Arc::new(Cancellation::default());
        let header_transform = self.header_transform.lock().clone();
        let headers = header_transform.headers(resource.kind, &resource.url);
        self.transport.start(TransferRequest {
            resource: resource.clone(),
            headers,
            reply: ActorRef::new(&mailbox),
            cancellation: Arc::clone(&cancellation),
        });
        Box::new(HTTPRequest { mailbox, cancellation })
    }
}

impl FileSource for HTTPFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        HTTPFileSource::request(self, resource, callback)
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn set_http_header_transform(&self, transform: HttpHeaderTransform) {
        *self.header_transform.lock() = transform;
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.resource_options.lock() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock().clone()
    }
}
