use crate::storage::request::{Request, Responder};
use crate::storage::resource::Resource;
use crate::storage::response::Response;

/// A source of resources. File sources are shared between threads, hence
/// `Send + Sync`. Each takes its configuration when it's made, or through
/// methods of its own type.
pub trait FileSource: Send + Sync {
    /// Starts loading `resource`, and sends its responses to `responder`
    /// from any thread. It returns at once.
    fn load(&self, resource: &Resource, responder: Responder);

    /// Whether this file source loads `resource`.
    fn can_request(&self, resource: &Resource) -> bool;

    /// Whether this file source answers cache-only requests: ones that are
    /// worth answering only cheaply, from a local cache, to cover part of
    /// the map while it loads.
    fn supports_cache_only(&self) -> bool {
        false
    }
}

/// Requests through a [`FileSource`].
pub trait FileSourceExt: FileSource {
    /// Requests `resource`. `callback` receives the responses on the
    /// threads that send them (see [`Request`]), until the request drops.
    fn request(&self, resource: &Resource, callback: impl FnMut(Response) + Send + 'static) -> Request {
        let (request, responder) = Request::new(callback);
        self.load(resource, responder);
        request
    }
}

impl<T: FileSource + ?Sized> FileSourceExt for T {}
