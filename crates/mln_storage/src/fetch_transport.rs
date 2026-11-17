//! The browser's `fetch`, which the `HTTPFileSource` starts its transfers
//! with. The browser's HTTP cache does the caching. A host can hand it
//! responses it fetched ahead (`OnlineCore::set_fetch_preloads`).
//!
//! The host's headers (`TransferRequest::headers`) go with the request, which
//! makes a cross-origin request one that the server must allow in its CORS
//! preflight. fetch follows a redirect with every header it was given, and
//! can't drop them when the redirect leaves the origin, so a request with the
//! host's headers doesn't follow redirects: a redirect fails it.

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::http_file_source::{self, Transfer, TransferRequest};
use crate::response::{Reason, ResourceError};

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
    /// loop, which calls `request.finish` once it is done.
    pub(crate) fn start(&self, request: TransferRequest) {
        let take = self.preloads.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if request.is_cancelled() {
                return;
            }
            let transfer = fetch(&request, take).await;
            request.finish(transfer);
        });
    }
}

async fn fetch(request: &TransferRequest, take: Option<js_sys::Function>) -> Result<Transfer, Box<ResourceError>> {
    let connection_error = |error: JsValue| {
        let message = error.as_string().unwrap_or_else(|| format!("{error:?}"));
        Box::new(ResourceError::new(Reason::Connection, message))
    };
    let url = &request.resource().url;
    let range = http_file_source::range_header(request.resource());
    let early = match range {
        None => preloaded(take, url).await,
        Some(_) => None,
    };
    let response = match early {
        Some(response) => response,
        None => send(url, range.as_deref(), request.headers()).await.map_err(connection_error)?,
    };
    if response.type_() == web_sys::ResponseType::Opaqueredirect {
        return Err(Box::new(ResourceError::new(
            Reason::Other,
            "A request with the host's HTTP headers can't follow a redirect in the browser",
        )));
    }
    transfer(&response).await.map_err(connection_error)
}

async fn transfer(response: &web_sys::Response) -> Result<Transfer, JsValue> {
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
    Ok(Transfer { status, headers, body })
}

async fn send(url: &str, range: Option<&str>, extra: &[(String, String)]) -> Result<web_sys::Response, JsValue> {
    let init = web_sys::RequestInit::new();
    if range.is_some() || !extra.is_empty() {
        let headers = web_sys::Headers::new()?;
        if let Some(range) = range {
            headers.set("Range", range)?;
        }
        for (name, value) in extra {
            headers.set(name, value)?;
        }
        init.set_headers(&headers);
    }
    if !extra.is_empty() {
        init.set_redirect(web_sys::RequestRedirect::Manual);
    }
    let request = web_sys::Request::new_with_str_and_init(url, &init)?;
    JsFuture::from(global_fetch(&request)).await?.dyn_into()
}
