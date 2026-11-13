//! Headers a host adds to the HTTP requests of the online file source, as
//! maplibre-native-ffi's HTTP file sources do for its
//! `mln_runtime_set_http_header_transform`. MapLibre Native has no
//! counterpart.

use std::sync::Arc;

use crate::resource::Kind;

/// Header names and values, in the order they are sent.
pub type HttpHeaders = Vec<(String, String)>;

/// Returns the headers for a request of `kind` to `url`. It runs on the
/// thread that starts the request, which waits for it, hence `Send + Sync`.
pub(crate) type HttpHeaderCallback = Arc<dyn Fn(Kind, &str) -> HttpHeaders + Send + Sync>;

/// The headers to add to each HTTP request: the callback gets the resource's
/// kind and the URL the request goes to, after any resource transform, and
/// runs again for every attempt of the request (retries and revalidations).
/// Only `http` and `https` URLs reach it.
///
/// The names are HTTP field names that neither the file source nor a
/// transport manages (not `Range`, `If-None-Match`, `If-Modified-Since`,
/// `User-Agent`, `Accept-Encoding`, `Host`, `Connection`, `Content-Length`,
/// `Transfer-Encoding`, ...), and the values are field values. A redirect to
/// another origin drops these headers.
#[derive(Clone, Default)]
pub struct HttpHeaderTransform {
    callback: Option<HttpHeaderCallback>,
}

impl HttpHeaderTransform {
    /// Adds the headers `callback` returns; `None` adds none.
    #[must_use]
    pub fn new(callback: Option<HttpHeaderCallback>) -> Self {
        HttpHeaderTransform { callback }
    }

    /// The headers for a request of `kind` to `url`: none for a URL that
    /// isn't `http` or `https`.
    #[must_use]
    pub(crate) fn headers(&self, kind: Kind, url: &str) -> HttpHeaders {
        match &self.callback {
            Some(callback) if is_http_url(url) => callback(kind, url),
            _ => HttpHeaders::new(),
        }
    }
}

/// Whether `url`'s scheme is `http` or `https`, in any letter case.
fn is_http_url(url: &str) -> bool {
    url.split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_get_headers() {
        let transform =
            HttpHeaderTransform::new(Some(Arc::new(|_, _| vec![("X-Token".to_owned(), "secret".to_owned())])));
        assert_eq!(1, transform.headers(Kind::Style, "http://example.test/style.json").len());
        assert_eq!(1, transform.headers(Kind::Style, "HTTPS://example.test/style.json").len());
        assert_eq!(transform.headers(Kind::Style, "file:///style.json"), HttpHeaders::new());
        assert_eq!(transform.headers(Kind::Style, "mbtiles:///tiles.mbtiles"), HttpHeaders::new());
        assert_eq!(transform.headers(Kind::Style, "httpx://example.test"), HttpHeaders::new());
        assert_eq!(HttpHeaderTransform::default().headers(Kind::Style, "http://example.test"), HttpHeaders::new());
    }
}
