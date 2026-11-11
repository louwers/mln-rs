//! NSURLSession as the `HttpTransport` on Apple platforms, as
//! platform/darwin/src/http_file_source.mm: the system's TLS, trust store,
//! proxy settings and HTTP/2 instead of ureq's.
//!
//! The session follows redirects itself and has no URL cache (the ambient
//! cache is the file source's own). It sends the request headers the ureq
//! transport sends (`Range`, `If-None-Match` or `If-Modified-Since`, user
//! agent "MapLibreNative/1.0"), plus `Accept-Encoding: identity` with a
//! `Range`, since a range of a compressed body can't be decompressed. The
//! response headers go to the `Transfer` in a fixed order, `Cache-Control`
//! before `Expires` as in the .mm (NSHTTPURLResponse doesn't keep the
//! server's order). Errors map to `Reason`s as in the .mm, with the
//! NSError's localized description as the message.

// Foundation through objc2 is the FFI here.
#![allow(unsafe_code)]

use std::sync::{Arc, Mutex};

use block2::RcBlock;
use mln_util::chrono::rfc1123;
use mln_util::sync::MutexExt;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSInteger, NSMutableURLRequest, NSString, NSURL, NSURLErrorBadServerResponse,
    NSURLErrorCallIsActive, NSURLErrorCannotConnectToHost, NSURLErrorCannotFindHost, NSURLErrorDNSLookupFailed,
    NSURLErrorDataNotAllowed, NSURLErrorInternationalRoamingOff, NSURLErrorNetworkConnectionLost,
    NSURLErrorNotConnectedToInternet, NSURLErrorTimedOut, NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
    NSURLSessionConfiguration, NSURLSessionDataTask,
};

use crate::http_file_source::{HttpTransport, Transfer, TransferRequest, range_header};
use crate::response::{Error, Reason};

/// How long a transfer may go without receiving data before it fails with
/// a timeout: bounds a stalled connection or handshake without capping a
/// slow download that still progresses (NSURLSession's default is 60 s).
const IDLE_TIMEOUT_SECONDS: f64 = 30.0;

/// MLNNetworkConfiguration's limit (NSURLSession's default is 6).
const MAXIMUM_CONNECTIONS_PER_HOST: NSInteger = 8;

/// The response headers a `Response` reads, in the order they go to the
/// `Transfer`.
const RESPONSE_HEADERS: [&str; 6] =
    ["Last-Modified", "ETag", "Cache-Control", "Expires", "Retry-After", "X-Rate-Limit-Reset"];

/// One NSURLSession for the file source's transfers, which run on the
/// session's own threads. Dropping the transport lets the transfers in
/// flight finish, then releases the session.
pub(crate) struct NSURLSessionTransport {
    session: Retained<NSURLSession>,
}

impl NSURLSessionTransport {
    pub(crate) fn new() -> Self {
        autoreleasepool(|_| {
            let configuration = NSURLSessionConfiguration::defaultSessionConfiguration();
            configuration.setURLCache(None);
            configuration.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
            configuration.setTimeoutIntervalForRequest(IDLE_TIMEOUT_SECONDS);
            configuration.setHTTPMaximumConnectionsPerHost(MAXIMUM_CONNECTIONS_PER_HOST);
            let session = NSURLSession::sessionWithConfiguration(&configuration);
            NSURLSessionTransport { session }
        })
    }
}

impl Drop for NSURLSessionTransport {
    fn drop(&mut self) {
        // A session stays alive until it is invalidated.
        self.session.finishTasksAndInvalidate();
    }
}

impl HttpTransport for NSURLSessionTransport {
    fn start(&self, request: TransferRequest) {
        if request.is_cancelled() {
            return;
        }
        autoreleasepool(|_| {
            let Some(url_request) = url_request(&request) else {
                request.finish(Err(Box::new(Error::new(Reason::Other, "Invalid URL"))));
                return;
            };

            // The task, for the cancel handler, which has to be set before
            // the completion block takes `request`.
            let slot = Arc::new(Mutex::new(TaskSlot::default()));
            request.set_cancel_handler({
                let slot = slot.clone();
                move || {
                    let mut slot = slot.lock_unpoisoned();
                    slot.cancelled = true;
                    if let Some(task) = slot.task.take() {
                        task.cancel();
                    }
                }
            });

            // The block is `Fn`, but the request finishes once.
            let pending = Mutex::new(Some(request));
            let completion =
                RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
                    let Some(request) = pending.lock_unpoisoned().take() else {
                        return;
                    };
                    // SAFETY: NSURLSession passes each object, or nil, valid
                    // for the duration of the call.
                    let (data, response, error) = unsafe { (data.as_ref(), response.as_ref(), error.as_ref()) };
                    // After a cancel (NSURLErrorCancelled), `finish` drops
                    // the result.
                    request.finish(transfer(data, response, error));
                });
            // SAFETY: the block is Send, as NSURLSession calls it on its
            // delegate queue: it holds a `Mutex` of a `TransferRequest`.
            let task = unsafe { self.session.dataTaskWithRequest_completionHandler(&url_request, &completion) };
            {
                let mut slot = slot.lock_unpoisoned();
                if slot.cancelled {
                    // Calls the completion block with NSURLErrorCancelled.
                    task.cancel();
                } else {
                    slot.task = Some(task.clone());
                }
            }
            task.resume();
        });
    }
}

/// A transfer's task, until its request is dropped.
#[derive(Default)]
struct TaskSlot {
    task: Option<Retained<NSURLSessionDataTask>>,
    cancelled: bool,
}

/// The request for `request`'s resource, with the headers it sends. `None`
/// when NSURL can't parse the URL.
fn url_request(request: &TransferRequest) -> Option<Retained<NSMutableURLRequest>> {
    let resource = request.resource();
    let url = NSURL::URLWithString(&NSString::from_str(&resource.url))?;
    let url_request = NSMutableURLRequest::requestWithURL(&url);
    let set_header = |name: &str, value: &str| {
        url_request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value)), &NSString::from_str(name));
    };
    if let Some(range) = range_header(resource) {
        set_header("Range", &range);
        set_header("Accept-Encoding", "identity");
    }
    // If there's already a response, set the correct etags/modified headers to
    // make sure we are getting a 304 response if possible. This avoids
    // redownloading unchanged data.
    if let Some(prior_etag) = &resource.prior_etag {
        set_header("If-None-Match", prior_etag);
    } else if let Some(prior_modified) = resource.prior_modified {
        set_header("If-Modified-Since", &rfc1123(prior_modified));
    }
    set_header("User-Agent", "MapLibreNative/1.0");
    Some(url_request)
}

/// The `Transfer` for what NSURLSession's completion handler got.
fn transfer(
    data: Option<&NSData>,
    response: Option<&NSURLResponse>,
    error: Option<&NSError>,
) -> Result<Transfer, Box<Error>> {
    if let Some(error) = error {
        return Err(Box::new(Error::new(reason(error.code()), error.localizedDescription().to_string())));
    }
    let Some(response) = response.and_then(|response| response.downcast_ref::<NSHTTPURLResponse>()) else {
        return Err(Box::new(Error::new(Reason::Other, "Response class is not NSHTTPURLResponse")));
    };
    let headers = RESPONSE_HEADERS
        .iter()
        .filter_map(|&name| {
            let value = response.valueForHTTPHeaderField(&NSString::from_str(name))?;
            Some((name.to_owned(), value.to_string()))
        })
        .collect();
    let status = i64::try_from(response.statusCode()).unwrap_or(0);
    let body = Ok(data.map(NSData::to_vec).unwrap_or_default());
    Ok(Transfer { status, headers, body })
}

/// The NSURLErrorDomain codes the .mm maps to `Reason::Connection`.
const CONNECTION_ERRORS: [NSInteger; 9] = [
    NSURLErrorNetworkConnectionLost,
    NSURLErrorCannotFindHost,
    NSURLErrorCannotConnectToHost,
    NSURLErrorDNSLookupFailed,
    NSURLErrorNotConnectedToInternet,
    NSURLErrorInternationalRoamingOff,
    NSURLErrorCallIsActive,
    NSURLErrorDataNotAllowed,
    NSURLErrorTimedOut,
];

/// The `Reason` for an NSURLErrorDomain code, as the .mm maps them.
fn reason(code: NSInteger) -> Reason {
    if code == NSURLErrorBadServerResponse {
        // 5xx errors
        Reason::Server
    } else if CONNECTION_ERRORS.contains(&code) {
        Reason::Connection
    } else {
        Reason::Other
    }
}
