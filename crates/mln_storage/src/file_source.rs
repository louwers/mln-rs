use crate::async_request::AsyncRequest;
use crate::resource::Resource;
use crate::response::Response;

/// Receives a request's response. It runs on the thread that made the
/// request, so it need not be `Send`.
pub type Callback = Box<dyn FnMut(Response) + 'static>;

/// A source of resources. File sources are shared between threads, hence
/// `Send + Sync`. Each takes its configuration when it's made, or through
/// methods of its own type.
pub trait FileSource: Send + Sync {
    /// Requests a resource. The callback runs later, on the thread that made
    /// the request, which must have an active `RunLoop`. Dropping the
    /// returned request cancels it: the callback doesn't run after that.
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest>;

    /// Whether this file source loads `resource`.
    fn can_request(&self, resource: &Resource) -> bool;

    /// Whether this file source answers cache-only requests: ones that are
    /// worth answering only cheaply, from a local cache, to cover part of
    /// the map while it loads.
    fn supports_cache_only(&self) -> bool {
        false
    }
}
