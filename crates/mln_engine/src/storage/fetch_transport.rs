//! The browser's `fetch`, which the `HTTPFileSource` starts its transfers
//! with. The browser's HTTP cache does the caching. A host can hand it
//! responses it fetched ahead (`OnlineCore::set_fetch_preloads`).
//!
//! A request for a resource the requester already has a copy of (one with
//! prior validators or an expiry) revalidates it: it asks the server, in
//! fetch's `no-cache` mode, rather than take the browser's copy, which may
//! be the same stale one under `stale-while-revalidate`.
//!
//! When the server can't be reached, a resource that the requester has no
//! copy of comes from the browser's stored copy, however old, unless the
//! server requires checking it first (`must-revalidate`, `no-cache`): the
//! native cache, too, shows a stale copy rather than none, where the
//! browser's cache would fail the request.
//!
//! A tile or image request sends its `Accept` (`accept_header`), which is
//! CORS-safelisted, so it needs no preflight, and follows redirects.
//!
//! The host's headers (`TransferRequest::headers`) go with the request, which
//! makes a cross-origin request one that the server must allow in its CORS
//! preflight. fetch follows a redirect with every header it was given, and
//! can't drop them when the redirect leaves the origin, so a request with the
//! host's headers doesn't follow redirects: a redirect fails it.
//!
//! Dropping a request aborts its fetch, which would otherwise keep loading
//! beside the fetches that are still wanted.

use std::future::poll_fn;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Poll, Waker};

use parking_lot::Mutex;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::storage::http_file_source::{self, Transfer, TransferRequest};
use crate::storage::http_header::CacheControl;
use crate::storage::response::{Reason, ResourceError};

#[wasm_bindgen]
extern "C" {
    /// The global `fetch`, a window's or a worker's.
    #[wasm_bindgen(js_name = fetch)]
    fn global_fetch(request: &web_sys::Request) -> js_sys::Promise;
}

/// The response the host fetched ahead for `url`, if it did and it came.
async fn preloaded(take: Option<js_sys::Function>, url: &str) -> Option<web_sys::Response> {
    let take = take?;
    let promise: js_sys::Promise = take.call1(&JsValue::NULL, &JsValue::from_str(url)).ok()?.dyn_into().ok()?;
    JsFuture::from(promise).await.ok()?.dyn_into().ok()
}

#[derive(Default)]
pub(crate) struct FetchTransport {
    /// Where the host's responses fetched ahead come from.
    preloads: Option<js_sys::Function>,
}

impl FetchTransport {
    /// Sets where the transport looks before it fetches a whole resource
    /// (one without a data range): `take(url)` returns a promise of the
    /// `Response` the host fetched ahead for exactly `url`, at most once for
    /// each, or undefined. A promise that rejects is fetched again, as if
    /// there had been none. Responses are used as they are, whatever their
    /// status, as a fetch of `url` would have answered.
    pub(crate) fn set_preloads(&mut self, take: Option<js_sys::Function>) {
        self.preloads = take;
    }

    /// Starts the transfer of `request` in a task of the browser's event
    /// loop, which calls `request.finish` once it is done, or aborts the
    /// fetch once the request is dropped.
    pub(crate) fn start(&self, request: TransferRequest) {
        let take = self.preloads.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if request.is_cancelled() {
                return;
            }
            let controller = web_sys::AbortController::new().ok();
            let signal = controller.as_ref().map(web_sys::AbortController::signal);
            match until_cancelled(&request, fetch(&request, take, signal.as_ref())).await {
                Some(transfer) => request.finish(transfer),
                None => {
                    if let Some(controller) = controller {
                        controller.abort();
                    }
                }
            }
        });
    }
}

/// The output of `transfer`, or `None` once `request` is dropped, which
/// drops `transfer`.
async fn until_cancelled<T>(request: &TransferRequest, transfer: impl Future<Output = T>) -> Option<T> {
    // The cancel handler must be `Send`, and the fetch's AbortController
    // isn't: the handler wakes this task, whose caller aborts the fetch.
    let waker: Arc<Mutex<Option<Waker>>> = Arc::default();
    request.set_cancel_handler({
        let waker = Arc::clone(&waker);
        move || {
            if let Some(waker) = waker.lock().take() {
                waker.wake();
            }
        }
    });
    let mut transfer = pin!(transfer);
    poll_fn(|cx| {
        *waker.lock() = Some(cx.waker().clone());
        if request.is_cancelled() {
            return Poll::Ready(None);
        }
        transfer.as_mut().poll(cx).map(Some)
    })
    .await
}

async fn fetch(
    request: &TransferRequest,
    take: Option<js_sys::Function>,
    signal: Option<&web_sys::AbortSignal>,
) -> Result<Transfer, Box<ResourceError>> {
    let connection_error = |error: JsValue| {
        let message = error.as_string().unwrap_or_else(|| format!("{error:?}"));
        Box::new(ResourceError::new(Reason::Connection, message))
    };
    let resource = request.resource();
    let url = &resource.url;
    let range = http_file_source::range_header(resource);
    let revalidate =
        resource.prior_caching.expires.is_some() || resource.prior_etag.is_some() || resource.prior_modified.is_some();
    let early = match range {
        None if !revalidate => preloaded(take, url).await,
        _ => None,
    };
    let cache = if revalidate { web_sys::RequestCache::NoCache } else { web_sys::RequestCache::Default };
    let (response, stale) = match early {
        Some(response) => (response, false),
        None => match send(request, range.as_deref(), signal, cache).await {
            Ok(response) => (response, false),
            // A requester with a copy keeps showing it.
            Err(error) if revalidate => return Err(connection_error(error)),
            Err(error) => match stored_copy(request, range.as_deref(), signal).await {
                Some(response) => (response, true),
                None => return Err(connection_error(error)),
            },
        },
    };
    if response.type_() == web_sys::ResponseType::Opaqueredirect {
        return Err(Box::new(ResourceError::new(
            Reason::Other,
            "A request with the host's HTTP headers can't follow a redirect in the browser",
        )));
    }
    transfer(&response, stale).await.map_err(connection_error)
}

/// The browser's stored copy of the resource, of any age, for a request that
/// couldn't reach the server, unless the server requires checking the copy
/// before each use.
async fn stored_copy(
    request: &TransferRequest,
    range: Option<&str>,
    signal: Option<&web_sys::AbortSignal>,
) -> Option<web_sys::Response> {
    if signal.is_some_and(web_sys::AbortSignal::aborted) {
        return None;
    }
    let response = send(request, range, signal, web_sys::RequestCache::ForceCache).await.ok()?;
    // A copy of the whole resource isn't the range the request asked for.
    if range.is_some() && response.status() != 206 {
        return None;
    }
    let cache_control = response.headers().get("Cache-Control").ok().flatten();
    let cache_control = cache_control.as_deref().map(CacheControl::parse).unwrap_or_default();
    (!cache_control.must_revalidate && !cache_control.no_cache).then_some(response)
}

async fn transfer(response: &web_sys::Response, stale: bool) -> Result<Transfer, JsValue> {
    let mut headers = Vec::new();
    if let Some(entries) = js_sys::try_iter(&response.headers())? {
        for entry in entries {
            let entry: js_sys::Array = entry?.dyn_into()?;
            headers.push((entry.get(0).as_string().unwrap_or_default(), entry.get(1).as_string().unwrap_or_default()));
        }
    }
    let status = i64::from(response.status());
    let body = match response.array_buffer() {
        Ok(promise) => match JsFuture::from(promise).await {
            Ok(buffer) => Ok(js_sys::Uint8Array::new(&buffer).to_vec()),
            Err(error) => Err(Box::new(ResourceError::new(Reason::Connection, format!("{error:?}")))),
        },
        Err(error) => Err(Box::new(ResourceError::new(Reason::Connection, format!("{error:?}")))),
    };
    Ok(Transfer { status, headers, body, stale })
}

/// Fetches `request`'s resource, its `range` of it if any.
async fn send(
    request: &TransferRequest,
    range: Option<&str>,
    signal: Option<&web_sys::AbortSignal>,
    cache: web_sys::RequestCache,
) -> Result<web_sys::Response, JsValue> {
    let (resource, extra) = (request.resource(), request.headers());
    let accept = http_file_source::accept_header(resource, extra);
    let init = web_sys::RequestInit::new();
    init.set_signal(signal);
    init.set_cache(cache);
    if range.is_some() || accept.is_some() || !extra.is_empty() {
        let headers = web_sys::Headers::new()?;
        if let Some(range) = range {
            headers.set("Range", range)?;
        }
        if let Some(accept) = accept {
            headers.set("Accept", accept)?;
        }
        for (name, value) in extra {
            headers.set(name, value)?;
        }
        init.set_headers(&headers);
    }
    if !extra.is_empty() {
        init.set_redirect(web_sys::RequestRedirect::Manual);
    }
    let request = web_sys::Request::new_with_str_and_init(&resource.url, &init)?;
    JsFuture::from(global_fetch(&request)).await?.dyn_into()
}
