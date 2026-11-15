use mln_actor::SchedulerPtr;

use crate::request::{Request, Responder};
use crate::resource::Resource;
use crate::response::Response;

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
    /// Requests `resource`. `callback` receives the responses in tasks of
    /// `queue`, which must run on this thread, until the request drops.
    fn request(&self, queue: &SchedulerPtr, resource: &Resource, callback: impl FnMut(Response) + 'static) -> Request {
        let (request, responder) = Request::new(queue, callback);
        self.load(resource, responder);
        request
    }
}

impl<T: FileSource + ?Sized> FileSourceExt for T {}
