//! The browser's `fetch` as the `HttpTransport`. The browser's HTTP cache does
//! the caching.

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
    let init = web_sys::RequestInit::new();
    if let Some(range) = range {
        let headers = web_sys::Headers::new()?;
        headers.set("Range", range)?;
        init.set_headers(&headers);
    }
    let request = web_sys::Request::new_with_str_and_init(url, &init)?;
    let response: web_sys::Response = JsFuture::from(global_fetch(&request)).await?.dyn_into()?;
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
