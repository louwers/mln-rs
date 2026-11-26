//! How a runtime loads resources: a host's resource provider in front of the
//! network, and the transforms of request URLs and headers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::http_header_transform::HttpHeaders;
use mln_storage::resource::Kind;
use mln_storage::response::Reason;
use mln_storage::{FileSource, OnlineFileSource, Resource, ResourceError, Responder, Response};
use mln_style::error::Error;
use parking_lot::{Mutex, RwLock};

/// Serves a runtime's network requests in place of the library.
///
/// A closure `Fn(ResourceRequest)` is a provider that serves every request.
/// The library loads only `http` and `https`, so without a resource
/// transform, a request for another scheme that no provider serves fails at
/// once.
pub trait ResourceProvider: Send + Sync + 'static {
    /// Whether the provider serves a request for a resource of `kind` at
    /// `url`. The library loads the requests that the provider doesn't
    /// serve.
    fn serves(&self, _kind: Kind, _url: &str) -> bool {
        true
    }

    /// Serves `request`: answer it with [`ResourceRequest::respond`], or hand
    /// it back with [`ResourceRequest::pass_through`], now or later and on
    /// any thread. It runs on a thread of the library's, which waits for it,
    /// and so does a replacement of the provider, so it should hand slow
    /// work elsewhere.
    fn request(&self, request: ResourceRequest);
}

impl<F: Fn(ResourceRequest) + Send + Sync + 'static> ResourceProvider for F {
    fn request(&self, request: ResourceRequest) {
        self(request);
    }
}

/// A maintenance action on a runtime's cache database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CacheDatabaseOperation {
    /// Deletes the ambient cache's resources and frees their space. Offline
    /// regions keep theirs.
    Clear,
    /// Keeps the ambient cache's resources, but makes them stale: maps check
    /// each with its server when they next use it, and show it meanwhile
    /// unless the server requires checking it first. Offline regions are
    /// unaffected.
    Invalidate,
    /// Compacts the database file to free the space it no longer uses.
    Pack,
    /// Deletes the whole database, offline regions included, and starts an
    /// empty one.
    Reset,
}

/// The runtime's hooks that its network file source reads on each request.
#[derive(Default)]
pub(crate) struct ResourceHooks {
    /// A request reads the provider for as long as it routes to it and calls
    /// it, so that a replacement, which writes it, comes after any call to
    /// the provider it replaces, and drops that provider itself.
    provider: RwLock<Option<Box<dyn ResourceProvider>>>,
    /// Whether a resource transform is set, which may rewrite a URL of any
    /// scheme into one that the library loads.
    transform_set: AtomicBool,
}

impl ResourceHooks {
    /// Replaces the provider, once no request is calling the one before,
    /// and gives that one back.
    pub(crate) fn set_provider(
        &self,
        provider: Option<Box<dyn ResourceProvider>>,
    ) -> Option<Box<dyn ResourceProvider>> {
        std::mem::replace(&mut *self.provider.write(), provider)
    }

    pub(crate) fn set_transform_set(&self, set: bool) {
        self.transform_set.store(set, Ordering::Release);
    }
}

/// The runtime's network file source: the library's online file source,
/// behind the host's resource provider.
pub(crate) struct ProviderFileSource {
    online: Arc<OnlineFileSource>,
    hooks: Arc<ResourceHooks>,
}

/// Who loads a request.
enum Route<'a> {
    /// The provider.
    Provider(&'a dyn ResourceProvider),
    /// The library. `declined` says whether a provider is set, which the
    /// message of a request that the library can't load names.
    Native { declined: bool },
}

impl ProviderFileSource {
    pub(crate) fn new(online: Arc<OnlineFileSource>, hooks: Arc<ResourceHooks>) -> Self {
        ProviderFileSource { online, hooks }
    }

    /// Who loads `resource`, when `provider` is the runtime's provider.
    fn route<'a>(resource: &Resource, provider: Option<&'a dyn ResourceProvider>) -> Route<'a> {
        if !resource.has_loading_method(mln_storage::LoadingMethod::Network) {
            return Route::Native { declined: false };
        }
        let Some(provider) = provider else {
            return Route::Native { declined: false };
        };
        if provider.serves(resource.kind, &resource.url) {
            Route::Provider(provider)
        } else {
            Route::Native { declined: true }
        }
    }

    fn native(&self) -> Native {
        Native { online: Arc::clone(&self.online), hooks: Arc::clone(&self.hooks) }
    }
}

/// The library's own loading, which fails a request for a URL scheme that
/// it can't load at once, rather than sending it to the network.
#[derive(Clone)]
struct Native {
    online: Arc<OnlineFileSource>,
    hooks: Arc<ResourceHooks>,
}

impl Native {
    fn load(&self, resource: &Resource, responder: Responder, declined: bool) {
        match self.unsupported_scheme(&resource.url) {
            Some(scheme) => responder.send(unsupported_scheme(scheme, &resource.url, declined)),
            None => self.online.load(resource, responder),
        }
    }

    /// The scheme of `url` when the online file source can't load it.
    fn unsupported_scheme<'a>(&self, url: &'a str) -> Option<&'a str> {
        if self.hooks.transform_set.load(Ordering::Acquire) {
            return None;
        }
        let scheme = url_scheme(url)?;
        let loadable = ["http", "https"].iter().any(|known| known.eq_ignore_ascii_case(scheme));
        (!loadable).then_some(scheme)
    }
}

/// The scheme of `url`, as RFC 3986 spells one: a letter, then letters,
/// digits, `+`, `-` and `.`, before a colon.
fn url_scheme(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    (first.is_ascii_alphabetic() && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte)))
        .then_some(scheme)
}

/// `url` without its query, fragment and user information, so that a
/// message never carries credentials.
fn redacted_url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let mut url = url[..end].to_owned();
    if let Some(separator) = url.find("://") {
        let authority = separator + 3;
        let authority_end = url[authority..].find('/').map_or(url.len(), |end| authority + end);
        if let Some(at) = url[authority..authority_end].rfind('@') {
            url.replace_range(authority..=authority + at, "");
        }
    }
    url
}

fn unsupported_scheme(scheme: &str, url: &str, declined: bool) -> Response {
    let advice = if declined {
        "the registered resource provider declined this request; update it to serve this scheme"
    } else {
        "register a resource provider with mln_runtime_set_resource_provider() or Runtime::set_resource_provider to \
         serve this scheme"
    };
    let message =
        format!("unsupported URL scheme \"{scheme}\" for network request \"{}\"; {advice}", redacted_url(url));
    // A reason that the library doesn't retry, since no retry can succeed.
    Response { error: Some(Box::new(ResourceError::new(Reason::Other, message))), ..Response::default() }
}

impl FileSource for ProviderFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let hooked = self.hooks.provider.read();
        let provider = match Self::route(resource, hooked.as_deref()) {
            Route::Provider(provider) => provider,
            Route::Native { declined } => {
                drop(hooked);
                return self.native().load(resource, responder, declined);
            }
        };
        let shared = Arc::new(RequestShared {
            state: Mutex::new(RequestState::default()),
            native: self.native(),
            resource: resource.clone(),
            responder: responder.clone(),
        });
        let cancelled = Arc::downgrade(&shared);
        responder.on_cancel(move || {
            if let Some(shared) = cancelled.upgrade() {
                shared.cancel();
            }
        });
        let request = ResourceRequest { resource: resource.clone(), shared: Some(Arc::clone(&shared)) };
        shared.state.lock().in_call = true;
        provider.request(request);
        drop(hooked);
        // An answer that came during the call goes out now, so that the
        // requester never sees it before the provider's call has returned.
        let deferred = {
            let mut state = shared.state.lock();
            state.in_call = false;
            state.deferred.take()
        };
        if let Some(answer) = deferred {
            answer(&shared);
        }
    }

    fn supports_cache_only(&self) -> bool {
        self.online.supports_cache_only()
    }

    fn can_request(&self, resource: &Resource) -> bool {
        let hooked = self.hooks.provider.read();
        matches!(Self::route(resource, hooked.as_deref()), Route::Provider(..)) || self.online.can_request(resource)
    }
}

#[derive(Default)]
struct RequestState {
    /// The requester dropped the request.
    cancelled: bool,
    /// The provider answered or passed through.
    answered: bool,
    on_cancel: Option<Box<dyn FnOnce() + Send>>,
    /// The provider's `request` call is running.
    in_call: bool,
    /// The answer given during that call, which goes out once it returns.
    deferred: Option<Answer>,
}

/// What a provider's answer does with the request.
type Answer = Box<dyn FnOnce(&RequestShared) + Send>;

/// A provider's request, which its [`ResourceRequest`] and the requester's
/// cancellation share.
struct RequestShared {
    state: Mutex<RequestState>,
    native: Native,
    resource: Resource,
    responder: Responder,
}

impl RequestShared {
    /// The requester dropped the request: the host's cancel callback runs
    /// unless the provider answered.
    fn cancel(&self) {
        let on_cancel = {
            let mut state = self.state.lock();
            state.cancelled = true;
            if state.answered { None } else { state.on_cancel.take() }
        };
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }

    fn respond(&self, response: Response) {
        // A revalidation may withhold the cached copy from the requester
        // until a "not modified" answer delivers it, as the online file
        // source's do.
        let response = self.resource.deliver(response);
        self.responder.send(response);
    }

    fn pass_through(&self) {
        self.native.load(&self.resource, self.responder.clone(), true);
    }
}

/// A network request that a [`ResourceProvider`] serves.
///
/// Answer it once, with [`respond`](Self::respond) or
/// [`pass_through`](Self::pass_through). Dropping it unanswered fails the
/// request.
pub struct ResourceRequest {
    resource: Resource,
    shared: Option<Arc<RequestShared>>,
}

impl ResourceRequest {
    /// The resource as the style names it, with the cache's earlier copy in
    /// its `prior_*` fields.
    #[must_use]
    pub fn resource(&self) -> &Resource {
        &self.resource
    }

    /// Whether the library no longer needs the request.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shared.as_ref().is_none_or(|shared| shared.state.lock().cancelled)
    }

    /// Calls `on_cancel`, on the requesting thread, if the library cancels
    /// the request before it is answered. Fails with
    /// [`ErrorKind::Cancelled`](mln_style::error::ErrorKind::Cancelled) when the
    /// library already cancelled it, and
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind::InvalidState) when
    /// the request has a cancel callback already.
    pub fn on_cancel(&self, on_cancel: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        let Some(shared) = &self.shared else {
            return Err(Error::invalid_state("the request was answered"));
        };
        let mut state = shared.state.lock();
        if state.cancelled {
            return Err(Error::cancelled("the library cancelled the request"));
        }
        if state.on_cancel.is_some() {
            return Err(Error::invalid_state("the request has a cancel callback already"));
        }
        state.on_cancel = Some(Box::new(on_cancel));
        Ok(())
    }

    /// Answers the request with `response`. Fails with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind::InvalidState) when
    /// the library cancelled the request.
    pub fn respond(mut self, response: Response) -> Result<(), Error> {
        self.answer(move |target| target.respond(response))
    }

    /// Hands the request back to the library, which loads it from the
    /// network as it would without a provider. Fails with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind::InvalidState) when
    /// the library cancelled the request.
    pub fn pass_through(mut self) -> Result<(), Error> {
        self.answer(RequestShared::pass_through)
    }

    fn answer(&mut self, answer: impl FnOnce(&RequestShared) + Send + 'static) -> Result<(), Error> {
        let Some(shared) = self.shared.take() else {
            return Err(Error::invalid_state("the request was answered"));
        };
        let (on_cancel, answer) = {
            let mut state = shared.state.lock();
            if state.cancelled {
                return Err(Error::invalid_state("the library cancelled the request"));
            }
            state.answered = true;
            let answer = if state.in_call {
                state.deferred = Some(Box::new(answer));
                None
            } else {
                Some(answer)
            };
            (state.on_cancel.take(), answer)
        };
        drop(on_cancel);
        if let Some(answer) = answer {
            answer(&shared);
        }
        Ok(())
    }
}

impl Drop for ResourceRequest {
    fn drop(&mut self) {
        if self.shared.is_some() {
            let mut response = Response::new();
            response.error =
                Some(Box::new(ResourceError::new(Reason::Other, "resource request was released without a response")));
            let _ = self.answer(move |target| target.respond(response));
        }
    }
}

impl std::fmt::Debug for ResourceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRequest")
            .field("kind", &self.resource.kind)
            .field("url", &self.resource.url)
            .finish_non_exhaustive()
    }
}

/// The headers that the library or the platform manages, which a header
/// transform can't set.
const MANAGED_HEADERS: [&str; 15] = [
    "accept-encoding",
    "connection",
    "content-length",
    "host",
    "if-modified-since",
    "if-none-match",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "range",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
];

/// Whether the library or the platform manages the HTTP header `name`, in
/// any letter case, so that a header transform can't set it.
#[must_use]
pub fn is_managed_http_header(name: &str) -> bool {
    MANAGED_HEADERS.iter().any(|managed| managed.eq_ignore_ascii_case(name))
}

/// `headers` without the ones the library manages, and with each name once:
/// a later value replaces an earlier one, whatever the letter case.
pub(crate) fn sanitize_headers(headers: HttpHeaders) -> HttpHeaders {
    let mut sanitized: HttpHeaders = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        if is_managed_http_header(&name) {
            log::warn!(
                target: crate::log_target::HTTP_REQUEST,
                "a header transform can't set the managed header {name}"
            );
            continue;
        }
        match sanitized.iter_mut().find(|(existing, _)| existing.eq_ignore_ascii_case(&name)) {
            Some(existing) => existing.1 = value,
            None => sanitized.push((name, value)),
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_and_repeated_headers_are_dropped() {
        let headers = vec![
            ("X-Token".to_owned(), "a".to_owned()),
            ("Range".to_owned(), "bytes=0-1".to_owned()),
            ("x-token".to_owned(), "b".to_owned()),
            ("User-Agent".to_owned(), "me".to_owned()),
        ];
        assert_eq!(vec![("X-Token".to_owned(), "b".to_owned())], sanitize_headers(headers));
    }
}
