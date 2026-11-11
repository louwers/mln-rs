//! The browser's `fetch` as the `HttpTransport`. The browser's HTTP cache does
//! the caching. A host can hand it responses it fetched ahead
//! (`set_fetch_preloads`).

use std::cell::RefCell;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::http_file_source::{self, HttpTransport, Transfer, TransferRequest};
use crate::response::{Error, Reason};

#[wasm_bindgen]
extern "C" {
    /// The global `fetch`, a window's or a worker's.
    #[wasm_bindgen(js_name = fetch)]
    fn global_fetch(request: &web_sys::Request) -> js_sys::Promise;
}

thread_local! {
    static PRELOADS: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Sets where this instance's fetch transport looks before it fetches a
/// whole resource (one without a data range): `take(url)` returns a promise
/// of the `Response` the host fetched ahead for exactly `url`, at most once
/// for each, or undefined. A promise that rejects is fetched again, as if
/// there had been none. Responses are used as they are, whatever their
/// status, as a fetch of `url` would have answered.
pub fn set_fetch_preloads(take: Option<js_sys::Function>) {
    PRELOADS.with(|preloads| *preloads.borrow_mut() = take);
}

/// The response the host fetched ahead for `url`, if it did and it came.
async fn preloaded(url: &str) -> Option<web_sys::Response> {
    let take = PRELOADS.with(|preloads| preloads.borrow().clone())?;
    let promise: js_sys::Promise = take.call1(&JsValue::NULL, &JsValue::from_str(url)).ok()?.dyn_into().ok()?;
    JsFuture::from(promise).await.ok()?.dyn_into().ok()
}

pub(crate) struct FetchTransport;

impl HttpTransport for FetchTransport {
    fn start(&self, request: TransferRequest) {
        let url = request.resource().url.clone();
        let range = http_file_source::range_header(request.resource());
        wasm_bindgen_futures::spawn_local(async move {
            if request.is_cancelled() {
                return;
            }
            let transfer = fetch(&url, range.as_deref()).await.map_err(|error| {
                let message = error.as_string().unwrap_or_else(|| format!("{error:?}"));
                Box::new(Error::new(Reason::Connection, message))
            });
            request.finish(transfer);
        });
    }
}

async fn fetch(url: &str, range: Option<&str>) -> Result<Transfer, JsValue> {
    let early = match range {
        None => preloaded(url).await,
        Some(_) => None,
    };
    let response = match early {
        Some(response) => response,
        None => request(url, range).await?,
    };
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
            Err(error) => Err(Box::new(Error::new(Reason::Connection, format!("{error:?}")))),
        },
        Err(error) => Err(Box::new(Error::new(Reason::Connection, format!("{error:?}")))),
    };
    Ok(Transfer { status, headers, body })
}

async fn request(url: &str, range: Option<&str>) -> Result<web_sys::Response, JsValue> {
    let init = web_sys::RequestInit::new();
    if let Some(range) = range {
        let headers = web_sys::Headers::new()?;
        headers.set("Range", range)?;
        init.set_headers(&headers);
    }
    let request = web_sys::Request::new_with_str_and_init(url, &init)?;
    JsFuture::from(global_fetch(&request)).await?.dyn_into()
}
