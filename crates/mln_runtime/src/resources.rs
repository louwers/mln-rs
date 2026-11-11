//! How a runtime loads resources: a host's resource provider in front of the
//! network, and the transforms of request URLs and headers.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_storage::file_source::Callback;
use mln_storage::http_header_transform::HttpHeaders;
use mln_storage::resource::Kind;
use mln_storage::response::Reason;
use mln_storage::{AsyncRequest, FileSource, OnlineFileSource, Resource, ResourceError, ResourceOptions, Response};
use mln_util::error::Error;
use mln_util::mapbox;
use mln_util::sync::MutexExt;

/// Serves a runtime's network requests in place of the library.
///
/// A closure `Fn(ResourceRequest)` is a provider that serves every request.
pub trait ResourceProvider: Send + Sync + 'static {
    /// Whether the provider serves a request for a resource of `kind` at
    /// `url`, the URL after the library expands any shorthand in the style's
    /// URL. The library loads the requests that the provider doesn't serve.
    fn serves(&self, _kind: Kind, _url: &str) -> bool {
        true
    }

    /// Serves `request`: answer it with [`ResourceRequest::respond`], or hand
    /// it back with [`ResourceRequest::pass_through`], now or later and on
    /// any thread. It runs on a thread of the library's, which waits for it,
    /// so it should hand slow work elsewhere.
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
    /// Keeps the ambient cache's resources, but makes maps check each with
    /// its server before they use it again. Offline regions are unaffected.
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
    provider: Mutex<Option<Arc<dyn ResourceProvider>>>,
}

impl ResourceHooks {
    /// Replaces the provider, and gives back the one before.
    pub(crate) fn set_provider(
        &self,
        provider: Option<Arc<dyn ResourceProvider>>,
    ) -> Option<Arc<dyn ResourceProvider>> {
        std::mem::replace(&mut *self.provider.lock_unpoisoned(), provider)
    }

    fn provider(&self) -> Option<Arc<dyn ResourceProvider>> {
        self.provider.lock_unpoisoned().clone()
    }
}

/// The runtime's network file source: the library's online file source,
/// behind the host's resource provider.
pub(crate) struct ProviderFileSource {
    online: Arc<dyn FileSource>,
    hooks: Arc<ResourceHooks>,
}

impl ProviderFileSource {
    pub(crate) fn new(options: &ResourceOptions, hooks: Arc<ResourceHooks>) -> Self {
        ProviderFileSource { online: Arc::new(OnlineFileSource::new(options)), hooks }
    }

    /// The provider and the resolved URL, when the provider serves the
    /// request. No lock is held while the provider decides.
    fn provider_for(&self, resource: &Resource) -> Option<(Arc<dyn ResourceProvider>, String)> {
        if !resource.has_loading_method(mln_storage::LoadingMethod::Network) {
            return None;
        }
        let provider = self.hooks.provider()?;
        let url = resolved_url(&self.online.get_resource_options(), resource);
        provider.serves(resource.kind, &url).then_some((provider, url))
    }
}

/// The URL that the online file source would load for `resource`, or the
/// resource's own URL when it has no other form.
fn resolved_url(options: &ResourceOptions, resource: &Resource) -> String {
    let tile_server = options.tile_server_options();
    let api_key = options.api_key();
    let url = &resource.url;
    match resource.kind {
        Kind::Unknown | Kind::Image => url.clone(),
        Kind::Style => mapbox::normalize_style_url(&tile_server, url, api_key),
        Kind::Source => mapbox::normalize_source_url(&tile_server, url, api_key).unwrap_or_else(|_| url.clone()),
        Kind::Glyphs => mapbox::normalize_glyphs_url(&tile_server, url, api_key),
        Kind::SpriteImage | Kind::SpriteJSON => mapbox::normalize_sprite_url(&tile_server, url, api_key),
        Kind::Tile => mapbox::normalize_tile_url(&tile_server, url, api_key),
    }
}

impl FileSource for ProviderFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let Some((provider, resolved_url)) = self.provider_for(resource) else {
            return self.online.request(resource, callback);
        };
        let target = Rc::new(ProviderTarget {
            callback: RefCell::new(Some(callback)),
            online: self.online.clone(),
            resource: resource.clone(),
            native: RefCell::new(None),
        });
        let mailbox = Mailbox::with_target(&scheduler::get_current(), target as Rc<dyn Any>);
        let shared =
            Arc::new(RequestShared { state: Mutex::new(RequestState::default()), target: ActorRef::new(&mailbox) });
        let request = ResourceRequest { resource: resource.clone(), resolved_url, shared: Some(shared.clone()) };
        provider.request(request);
        Box::new(ProviderRequest { mailbox, shared })
    }

    fn forward(&self, resource: &Resource, response: &Response, callback: Option<Box<dyn FnOnce() + Send>>) {
        self.online.forward(resource, response, callback);
    }

    fn supports_cache_only_requests(&self) -> bool {
        self.online.supports_cache_only_requests()
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.provider_for(resource).is_some() || self.online.can_request(resource)
    }

    fn pause(&self) {
        self.online.pause();
    }

    fn resume(&self) {
        self.online.resume();
    }

    fn set_resource_transform(&self, transform: mln_storage::resource_transform::ResourceTransform) {
        self.online.set_resource_transform(transform);
    }

    fn set_http_header_transform(&self, transform: mln_storage::http_header_transform::HttpHeaderTransform) {
        self.online.set_http_header_transform(transform);
    }

    fn set_property(&self, key: &str, value: &feature::Value) {
        self.online.set_property(key, value);
    }

    fn get_property(&self, key: &str) -> feature::Value {
        self.online.get_property(key)
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        self.online.set_resource_options(options);
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.online.get_resource_options()
    }
}

/// The requesting side of a provider's request, on the requesting thread.
struct ProviderTarget {
    callback: RefCell<Option<Callback>>,
    online: Arc<dyn FileSource>,
    resource: Resource,
    /// The online file source's request, once the provider passed through.
    native: RefCell<Option<Box<dyn AsyncRequest>>>,
}

impl ProviderTarget {
    fn respond(&self, response: Response) {
        let callback = self.callback.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback(response);
        }
    }

    fn pass_through(&self) {
        let callback = self.callback.borrow_mut().take();
        if let Some(callback) = callback {
            let native = self.online.request(&self.resource, callback);
            *self.native.borrow_mut() = Some(native);
        }
    }
}

#[derive(Default)]
struct RequestState {
    /// The requester dropped the request.
    cancelled: bool,
    /// The provider answered or passed through.
    answered: bool,
    on_cancel: Option<Box<dyn FnOnce() + Send>>,
}

struct RequestShared {
    state: Mutex<RequestState>,
    target: ActorRef<ProviderTarget>,
}

/// What the requester holds: dropping it cancels the provider's request.
struct ProviderRequest {
    mailbox: Arc<Mailbox>,
    shared: Arc<RequestShared>,
}

impl AsyncRequest for ProviderRequest {}

impl Drop for ProviderRequest {
    fn drop(&mut self) {
        let on_cancel = {
            let mut state = self.shared.state.lock_unpoisoned();
            state.cancelled = true;
            if state.answered { None } else { state.on_cancel.take() }
        };
        self.mailbox.close();
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }
}

/// A network request that a [`ResourceProvider`] serves.
///
/// Answer it once, with [`respond`](Self::respond) or
/// [`pass_through`](Self::pass_through). Dropping it unanswered fails the
/// request.
pub struct ResourceRequest {
    resource: Resource,
    resolved_url: String,
    shared: Option<Arc<RequestShared>>,
}

impl ResourceRequest {
    /// The resource as the style names it, with the cache's earlier copy in
    /// its `prior_*` fields.
    pub fn resource(&self) -> &Resource {
        &self.resource
    }

    /// The URL to load: the resource's after the library expands any
    /// shorthand in it, and before any resource transform.
    pub fn resolved_url(&self) -> &str {
        &self.resolved_url
    }

    /// Whether the library no longer needs the request.
    pub fn is_cancelled(&self) -> bool {
        self.shared.as_ref().is_none_or(|shared| shared.state.lock_unpoisoned().cancelled)
    }

    /// Calls `on_cancel`, on the requesting thread, if the library cancels
    /// the request before it is answered. Fails with
    /// [`ErrorKind::Cancelled`](mln_util::ErrorKind::Cancelled) when the
    /// library already cancelled it, and
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind::InvalidState) when
    /// the request has a cancel callback already.
    pub fn on_cancel(&self, on_cancel: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        let Some(shared) = &self.shared else {
            return Err(Error::invalid_state("the request was answered"));
        };
        let mut state = shared.state.lock_unpoisoned();
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
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind::InvalidState) when
    /// the library cancelled the request.
    pub fn respond(mut self, response: Response) -> Result<(), Error> {
        self.answer(move |target| target.respond(response))
    }

    /// Hands the request back to the library, which loads it from the
    /// network as it would without a provider. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind::InvalidState) when
    /// the library cancelled the request.
    pub fn pass_through(mut self) -> Result<(), Error> {
        self.answer(ProviderTarget::pass_through)
    }

    fn answer(&mut self, answer: impl FnOnce(&ProviderTarget) + Send + 'static) -> Result<(), Error> {
        let Some(shared) = self.shared.take() else {
            return Err(Error::invalid_state("the request was answered"));
        };
        let on_cancel = {
            let mut state = shared.state.lock_unpoisoned();
            if state.cancelled {
                return Err(Error::invalid_state("the library cancelled the request"));
            }
            state.answered = true;
            state.on_cancel.take()
        };
        drop(on_cancel);
        shared.target.invoke(answer);
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
            .field("resolved_url", &self.resolved_url)
            .finish()
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

/// `headers` without the ones the library manages, and with each name once:
/// a later value replaces an earlier one, whatever the letter case.
pub(crate) fn sanitize_headers(headers: HttpHeaders) -> HttpHeaders {
    let mut sanitized: HttpHeaders = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if MANAGED_HEADERS.contains(&lower.as_str()) {
            mln_util::logging::Log::warning(
                mln_util::logging::Event::HttpRequest,
                format!("a header transform can't set the managed header {name}"),
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
