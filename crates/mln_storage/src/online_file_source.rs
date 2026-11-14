// The requests run on the "OnlineFileSource" thread (`util::Thread`) as in
// C++: an `OnlineFileRequest` per caller request, scheduled by a
// `util::Timer` (first request at once, then revalidation when the response
// expires, or retries with backoff after errors), queued while
// `maximumConcurrentRequests` transfers are active (regular priority before
// low priority, and within a priority by the resource's `RequestOrder`), and
// performed by the HTTPFileSource.
//
// C++ keys its sets by `OnlineFileRequest*`; here every request has an id
// (the caller's request id), so `allRequests`/`activeRequests` iterate in id
// order rather than pointer order.
//
// `OnlineFileRequest::actor()` (the transformed URL's way back) is a mailbox
// on the online thread whose target finds the request by id.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use feature::Value;
use mln_actor::{ActorRef, AsyncTask, Mailbox, Thread, Timer, scheduler};
use mln_util::chrono::{Timestamp, now};
use mln_util::constants::{
    ASSET_PROTOCOL, CLOCK_SKEW_RETRY_TIMEOUT, DEFAULT_MAXIMUM_CONCURRENT_REQUESTS, FILE_PROTOCOL,
};
use mln_util::logging::{Event, Log};
use mln_vec_map::{VecMap, VecSet};
use parking_lot::Mutex;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource, MAX_CONCURRENT_REQUESTS_KEY};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::http_file_source::HTTPFileSource;
use crate::http_header_transform::HttpHeaderTransform;
use crate::http_timeout::{error_retry_timeout, expiration_timeout};
use crate::network_status::{NetworkStatus, Status};
use crate::resource::{Priority, RequestOrder, Resource};
use crate::resource_options::ResourceOptions;
use crate::resource_transform::{ResourceTransform, TransformResult};
use crate::response::{Reason, ResourceError, Response};

/// For testing only
pub(crate) const ONLINE_STATUS_KEY: &str = "online-status";

struct OnlineFileRequest {
    id: u64,
    state: Weak<ThreadState>,
    resource: RefCell<Resource>,
    request: RefCell<Option<Box<dyn AsyncRequest>>>,
    timer: Timer,
    callback: Box<dyn Fn(Response)>,

    /// Counts the number of times a response was already expired when
    /// received. We're using this to add a delay when making a new request
    /// so we don't keep retrying immediately in case of a server serving
    /// expired tiles.
    expired_requests: Cell<u32>,

    /// Counts the number of subsequent failed requests. We're using this
    /// value for exponential backoff when retrying requests.
    failed_requests: Cell<u32>,
    failed_request_reason: Cell<Reason>,
    retry_after: Cell<Option<Timestamp>>,

    /// Whether the resource transform is computing the URL to load.
    transforming: Cell<bool>,
    /// Whether the resource transform failed the request. A retry calls the
    /// transform again instead of loading the original URL.
    transform_failed: Cell<bool>,

    /// Lazily created for the `ResourceTransform`'s reply (`actor()`).
    mailbox: RefCell<Option<Arc<Mailbox>>>,
}

/// The target of an `OnlineFileRequest`'s mailbox.
struct OnlineFileRequestRef {
    state: Weak<ThreadState>,
    id: u64,
}

impl OnlineFileRequestRef {
    fn set_transform_result(&self, result: TransformResult) {
        if let Some(state) = self.state.upgrade()
            && let Some(req) = state.get(self.id)
        {
            req.set_transform_result(result);
        }
    }
}

impl Drop for OnlineFileRequest {
    fn drop(&mut self) {
        if let Some(mailbox) = self.mailbox.borrow_mut().take() {
            mailbox.close();
        }
    }
}

/// What waiting requests start by, lowest first: regular priority before
/// low priority, then the resource's [`RequestOrder`] as it is now.
fn start_key(priority: Priority, order: &RequestOrder) -> (bool, u32) {
    (priority == Priority::Low, order.get())
}

/// The requests waiting for room in the active set, in the order they
/// started to wait.
#[derive(Default)]
struct PendingRequests {
    requests: VecDeque<PendingRequest>,
}

struct PendingRequest {
    id: u64,
    priority: Priority,
    order: RequestOrder,
}

impl PendingRequests {
    fn remove(&mut self, id: u64) {
        self.requests.retain(|request| request.id != id);
    }

    fn insert(&mut self, id: u64, resource: &Resource) {
        self.requests.push_back(PendingRequest { id, priority: resource.priority, order: resource.order.clone() });
    }

    /// Removes the request to start next: the one with the lowest
    /// [`start_key`], and of those the one that has waited longest. A
    /// request's order may change while it waits, so every call reads them
    /// all.
    fn pop(&mut self) -> Option<u64> {
        // `min_by_key` returns the first of equal elements.
        let (index, _) =
            self.requests.iter().enumerate().min_by_key(|(_, request)| start_key(request.priority, &request.order))?;
        self.requests.remove(index).map(|request| request.id)
    }

    fn contains(&self, id: u64) -> bool {
        self.requests.iter().any(|request| request.id == id)
    }
}

/// The members of `OnlineFileSourceThread`.
struct ThreadState {
    resource_options: RefCell<ResourceOptions>,

    /// The lifetime of a request is:
    ///
    /// 1. Waiting for timeout (revalidation or retry)
    /// 2. Pending (waiting for room in the active set)
    /// 3. Active (open network connection)
    /// 4. Back to #1
    ///
    /// Requests in any state are in `all_requests`. Requests in the pending
    /// state are in `pending_requests`. Requests in the active state are in
    /// `active_requests`.
    all_requests: RefCell<VecSet<u64>>,
    pending_requests: RefCell<PendingRequests>,
    active_requests: RefCell<VecSet<u64>>,

    online: Cell<bool>,
    maximum_concurrent_requests: Cell<u32>,
    http_file_source: HTTPFileSource,
    resource_transform: RefCell<Option<ResourceTransform>>,
    tasks: RefCell<VecMap<u64, Rc<OnlineFileRequest>>>,
    /// Replaces the backoff after a connection error, so that a test can
    /// tell the retries it starts apart from the backoff's.
    #[cfg(test)]
    connection_backoff: Cell<Option<Duration>>,
}

pub(crate) struct OnlineFileSourceThread {
    state: Rc<ThreadState>,
    _reachability: AsyncTask,
    reachability_id: u64,
}

fn interpolate_expiration(current: Timestamp, prior: Option<Timestamp>, expired: &mut bool) -> Timestamp {
    let now = now();
    if current > now {
        return current;
    }

    let Some(prior) = prior else {
        *expired = true;
        return current;
    };

    // Expiring date is going backwards, fallback to exponential backoff.
    if current < prior {
        *expired = true;
        return current;
    }

    let delta = current.secs_since_epoch() - prior.secs_since_epoch();

    // Server is serving the same expired resource over and over, fallback
    // to exponential backoff.
    if delta == 0 {
        *expired = true;
        return current;
    }

    // Assume that either the client or server clock is wrong and try to
    // interpolate a valid expiration date (from the client POV) observing a
    // minimum timeout.
    let minimum = CLOCK_SKEW_RETRY_TIMEOUT.as_secs() as i64;
    Timestamp::from_secs(now.secs_since_epoch() + delta.max(minimum))
}

/// The URL without its query string.
fn sanitize_url(url: &str) -> &str {
    match url.find('?') {
        Some(query_index) => &url[..query_index],
        None => url,
    }
}

impl ThreadState {
    fn get(&self, id: u64) -> Option<Rc<OnlineFileRequest>> {
        self.tasks.borrow().get(&id).cloned()
    }

    fn add(&self, req: &OnlineFileRequest) {
        self.all_requests.borrow_mut().insert(req.id);
        self.transform(req);
    }

    fn transform(&self, req: &OnlineFileRequest) {
        req.transform_failed.set(false);
        let transform = self.resource_transform.borrow().clone();
        if let Some(transform) = transform {
            // Request the ResourceTransform actor a new url and replace the
            // resource url with the transformed one before proceeding to
            // schedule the request.
            req.transforming.set(true);
            let r = req.actor();
            let (kind, url) = {
                let resource = req.resource.borrow();
                (resource.kind, resource.url.clone())
            };
            transform(
                kind,
                &url,
                Box::new(move |result: TransformResult| {
                    r.invoke(move |req: &OnlineFileRequestRef| req.set_transform_result(result));
                }),
            );
        } else {
            req.activate();
        }
    }

    fn remove(self: &Rc<Self>, id: u64) {
        self.all_requests.borrow_mut().remove(&id);
        let was_active = self.active_requests.borrow_mut().remove(&id);
        if was_active {
            self.activate_pending_request();
        } else {
            self.pending_requests.borrow_mut().remove(id);
        }
    }

    fn activate_or_queue_request(self: &Rc<Self>, id: u64) {
        let Some(req) = self.get(id) else {
            return;
        };
        debug_assert!(self.all_requests.borrow().contains(&id));
        debug_assert!(!self.active_requests.borrow().contains(&id));
        debug_assert!(req.request.borrow().is_none());

        if self.active_requests.borrow().len() >= self.maximum_concurrent_requests.get() as usize {
            self.pending_requests.borrow_mut().insert(id, &req.resource.borrow());
        } else {
            self.activate_request(&req);
        }
    }

    fn activate_request(self: &Rc<Self>, req: &Rc<OnlineFileRequest>) {
        let weak = Rc::downgrade(self);
        let id = req.id;
        let callback = move |response: Response| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.active_requests.borrow_mut().remove(&id);
            if let Some(req) = this.get(id) {
                // The transfer is done; dropping its request is a no-op.
                let finished = req.request.borrow_mut().take();
                drop(finished);
                req.completed(response);
            }
            this.activate_pending_request();
        };

        self.active_requests.borrow_mut().insert(req.id);

        if self.online.get() {
            let resource = req.resource.borrow().clone();
            let request = self.http_file_source.request(&resource, Box::new(callback));
            *req.request.borrow_mut() = Some(request);
        } else {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Connection, "Online connectivity is disabled.")));
            callback(response);
        }
    }

    fn activate_pending_request(self: &Rc<Self>) {
        let next = self.pending_requests.borrow_mut().pop();
        if let Some(id) = next
            && let Some(req) = self.get(id)
        {
            self.activate_request(&req);
        }
    }

    fn is_pending(&self, id: u64) -> bool {
        self.pending_requests.borrow().contains(id)
    }

    fn is_active(&self, id: u64) -> bool {
        self.active_requests.borrow().contains(&id)
    }

    fn network_is_reachable_again(&self) {
        let mut requests: Vec<Rc<OnlineFileRequest>> =
            self.all_requests.borrow().iter().filter_map(|id| self.get(*id)).collect();
        // Notify the requests in the order they would start in.
        requests.sort_by_key(|req| {
            let resource = req.resource.borrow();
            start_key(resource.priority, &resource.order)
        });
        for req in &requests {
            req.network_is_reachable_again();
        }
    }
}

impl OnlineFileRequest {
    fn set_transform_result(&self, result: TransformResult) {
        self.transforming.set(false);
        match result {
            Ok(url) => {
                self.resource.borrow_mut().url = url;
                self.activate();
            }
            Err(error) => {
                self.transform_failed.set(true);
                let mut response = Response::new();
                response.error = Some(Box::new(error));
                self.completed(response);
            }
        }
    }

    fn actor(&self) -> ActorRef<OnlineFileRequestRef> {
        let mut mailbox = self.mailbox.borrow_mut();
        // Lazy constructed because this can be costly and the
        // ResourceTransform is not used by many apps.
        let mailbox = mailbox.get_or_insert_with(|| {
            let target = Rc::new(OnlineFileRequestRef { state: Weak::clone(&self.state), id: self.id });
            Mailbox::with_target(&scheduler::get_current(), target)
        });
        ActorRef::new(mailbox)
    }

    fn activate(&self) {
        // Force an immediate first request if we don't have an expiration time.
        let prior_expires = self.resource.borrow().prior_expires;
        let timeout = match prior_expires {
            Some(_) => self.get_update_interval(prior_expires),
            None => Some(Duration::ZERO),
        };
        self.schedule(timeout);
    }

    /// `schedule(Duration timeout)`: requests the resource again after
    /// `timeout`, or never for `None`.
    fn schedule(&self, timeout: Option<Duration>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        if self.transforming.get() || state.is_pending(self.id) || state.is_active(self.id) {
            // There's already a request in progress; don't start another one.
            return;
        }

        let Some(timeout) = timeout else {
            return;
        };

        // Emulate a Connection error when the Offline mode is forced, and
        // don't retry: the request will get re-triggered when the
        // NetworkStatus is set back to Online.
        if NetworkStatus::get() == Status::Offline {
            self.failed_request_reason.set(Reason::Connection);
            self.failed_requests.set(1);
            self.timer.stop();
            return;
        }

        let weak = Weak::clone(&self.state);
        let id = self.id;
        self.timer.start(timeout, Duration::ZERO, move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            match state.get(id) {
                Some(req) if req.transform_failed.get() => state.transform(&req),
                _ => state.activate_or_queue_request(id),
            }
        });
    }

    /// When to request the resource again, or `None` for never.
    fn get_update_interval(&self, expires: Option<Timestamp>) -> Option<Duration> {
        // Calculate a timeout that depends on how many consecutive errors
        // we've encountered, and on the expiration time, if present.
        let error_retry =
            error_retry_timeout(self.failed_request_reason.get(), self.failed_requests.get(), self.retry_after.get());
        #[cfg(test)]
        let error_retry = match self.state.upgrade().and_then(|state| state.connection_backoff.get()) {
            Some(backoff) if self.failed_request_reason.get() == Reason::Connection => Some(backoff),
            _ => error_retry,
        };
        let minimum = self.resource.borrow().minimum_update_interval;
        let expiration = expiration_timeout(expires, self.expired_requests.get()).map(|e| e.max(minimum));
        match (expiration, error_retry) {
            (Some(expiration), Some(error_retry)) => Some(expiration.min(error_retry)),
            (expiration, error_retry) => expiration.or(error_retry),
        }
    }

    /// `completed(Response)`
    fn completed(&self, mut response: Response) {
        // If we didn't get various caching headers in the response, continue
        // using the previous values. Otherwise, update the previous values
        // to the new values.
        {
            let mut resource = self.resource.borrow_mut();
            if response.modified.is_none() {
                response.modified = resource.prior_modified;
            } else {
                resource.prior_modified = response.modified;
            }

            if response.not_modified && resource.prior_data.is_some() {
                // When the priorData field is set, it indicates that we had
                // to revalidate the request and that the requestor hasn't
                // gotten data yet. If we get a 304 response, this means that
                // we have send the cached data to give the requestor a
                // chance to actually obtain the data.
                response.data = resource.prior_data.take();
                response.not_modified = false;
            }
        }

        let mut is_expired = false;

        if let Some(expires) = response.expires {
            let prior = self.resource.borrow().prior_expires;
            self.resource.borrow_mut().prior_expires = Some(expires);
            response.expires = Some(interpolate_expiration(expires, prior, &mut is_expired));
        }

        if is_expired {
            self.expired_requests.set(self.expired_requests.get() + 1);
        } else {
            self.expired_requests.set(0);
        }

        {
            let mut resource = self.resource.borrow_mut();
            if response.etag.is_none() {
                response.etag = resource.prior_etag.clone();
            } else {
                resource.prior_etag = response.etag.clone();
            }
        }

        if let Some(error) = &response.error {
            if error.reason == Reason::NotFound {
                let url = self.resource.borrow().url.clone();
                Log::error(Event::General, format!("The resource `{}` not found", sanitize_url(&url)));
            }
            self.failed_requests.set(self.failed_requests.get() + 1);
            self.failed_request_reason.set(error.reason);
            self.retry_after.set(error.retry_after);
        } else {
            self.failed_requests.set(0);
            self.failed_request_reason.set(Reason::Success);
        }

        self.schedule(self.get_update_interval(response.expires));

        // Calling the callback may result in `this` being deleted. It needs
        // to be done last.
        (self.callback)(response);
    }

    fn network_is_reachable_again(&self) {
        // We need all requests to fail at least once before we are going to
        // start retrying them, and we only immediately restart request that
        // failed due to connection issues.
        if self.failed_request_reason.get() == Reason::Connection {
            self.schedule(Some(Duration::ZERO));
        }
    }
}

impl OnlineFileSourceThread {
    fn new(resource_options: ResourceOptions) -> Self {
        let http_file_source = HTTPFileSource::new(&resource_options);
        let state = Rc::new(ThreadState {
            resource_options: RefCell::new(resource_options),
            all_requests: RefCell::new(VecSet::new()),
            pending_requests: RefCell::new(PendingRequests::default()),
            active_requests: RefCell::new(VecSet::new()),
            online: Cell::new(true),
            maximum_concurrent_requests: Cell::new(DEFAULT_MAXIMUM_CONCURRENT_REQUESTS),
            http_file_source,
            resource_transform: RefCell::new(None),
            tasks: RefCell::new(VecMap::new()),
            #[cfg(test)]
            connection_backoff: Cell::new(None),
        });
        let weak = Rc::downgrade(&state);
        let reachability = AsyncTask::new(move || {
            if let Some(state) = weak.upgrade() {
                state.network_is_reachable_again();
            }
        });
        let reachability_id = NetworkStatus::subscribe(reachability.sender());
        OnlineFileSourceThread { state, _reachability: reachability, reachability_id }
    }

    pub(crate) fn request(&self, req: u64, resource: Resource, r: &ActorRef<FileSourceRequestState>) {
        let r = r.clone();
        let callback = move |res: Response| {
            r.invoke(move |request: &FileSourceRequestState| request.set_response(res));
        };
        let request = Rc::new(OnlineFileRequest {
            id: req,
            state: Rc::downgrade(&self.state),
            resource: RefCell::new(resource),
            request: RefCell::new(None),
            timer: Timer::new(),
            callback: Box::new(callback),
            expired_requests: Cell::new(0),
            failed_requests: Cell::new(0),
            failed_request_reason: Cell::new(Reason::Success),
            retry_after: Cell::new(None),
            transforming: Cell::new(false),
            transform_failed: Cell::new(false),
            mailbox: RefCell::new(None),
        });
        self.state.tasks.borrow_mut().insert(req, Rc::clone(&request));
        self.state.add(&request);
    }

    pub(crate) fn cancel(&self, req: u64) {
        self.state.remove(req);
        let task = self.state.tasks.borrow_mut().remove(&req);
        drop(task);
    }

    fn set_resource_transform(&self, transform: Option<ResourceTransform>) {
        *self.state.resource_transform.borrow_mut() = transform;
    }

    fn set_http_header_transform(&self, transform: HttpHeaderTransform) {
        self.state.http_file_source.set_http_header_transform(transform);
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.state.resource_options.borrow_mut() = options;
    }

    fn set_online_status(&self, status: bool) {
        self.state.online.set(status);
        if status {
            self.state.network_is_reachable_again();
        }
    }

    fn set_maximum_concurrent_requests(&self, maximum: u32) {
        self.state.maximum_concurrent_requests.set(maximum);
    }
}

impl Drop for OnlineFileSourceThread {
    fn drop(&mut self) {
        NetworkStatus::unsubscribe(self.reachability_id);
        // Requests hold timers on this thread; drop them here.
        self.state.tasks.borrow_mut().clear();
    }
}

/// The `Network` file source.
pub struct OnlineFileSource {
    cached_resource_options: Mutex<ResourceOptions>,
    cached_maximum_concurrent_requests: Mutex<u32>,
    thread: Thread<OnlineFileSourceThread>,
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

impl OnlineFileSource {
    #[must_use]
    pub fn new(resource_options: &ResourceOptions) -> Self {
        let ro = resource_options.clone();
        let thread = Thread::new("OnlineFileSource", move |_| OnlineFileSourceThread::new(ro));
        OnlineFileSource {
            cached_resource_options: Mutex::new(resource_options.clone()),
            cached_maximum_concurrent_requests: Mutex::new(DEFAULT_MAXIMUM_CONCURRENT_REQUESTS),
            thread,
        }
    }

    /// Holds the online thread until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(&self.thread.actor())
    }

    /// Waits `backoff` to retry a request that failed with a connection
    /// error, in place of the exponential backoff.
    #[cfg(test)]
    pub(crate) fn set_connection_backoff(&self, backoff: Duration) {
        self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.state.connection_backoff.set(Some(backoff)));
    }
}

impl FileSource for OnlineFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let mut req = FileSourceRequest::new(callback);
        let req_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let actor_ref = self.thread.actor();
        req.on_cancel(Box::new(move || {
            actor_ref.invoke(move |t: &OnlineFileSourceThread| t.cancel(req_id));
        }));
        let req_ref = req.actor();
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.request(req_id, resource, &req_ref));
        Box::new(req)
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(crate::resource::LoadingMethod::Network)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }

    fn set_property(&self, key: &str, value: &Value) {
        if key == MAX_CONCURRENT_REQUESTS_KEY {
            if let Value::Uint(maximum) = value {
                debug_assert!(*maximum < u64::from(u32::MAX));
                let maximum = *maximum as u32;
                self.thread
                    .actor()
                    .invoke(move |t: &OnlineFileSourceThread| t.set_maximum_concurrent_requests(maximum));
                *self.cached_maximum_concurrent_requests.lock() = maximum;
            } else {
                Log::error(Event::General, "Invalid max-concurrent-requests property value type.");
            }
        } else if key == ONLINE_STATUS_KEY {
            // For testing only
            if let Value::Bool(status) = value {
                let status = *status;
                self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.set_online_status(status));
            }
        } else {
            Log::error(Event::General, format!("Resource provider does not support property {key}"));
        }
    }

    fn get_property(&self, key: &str) -> Value {
        if key == MAX_CONCURRENT_REQUESTS_KEY {
            Value::Uint(u64::from(*self.cached_maximum_concurrent_requests.lock()))
        } else {
            Log::error(Event::General, format!("Resource provider does not support property {key}"));
            Value::Null
        }
    }

    fn set_resource_transform(&self, transform: Option<ResourceTransform>) {
        self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.set_resource_transform(transform));
    }

    fn set_http_header_transform(&self, transform: HttpHeaderTransform) {
        self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.set_http_header_transform(transform));
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        let o = options.clone();
        self.thread.actor().invoke(move |t: &OnlineFileSourceThread| t.set_resource_options(o));
        *self.cached_resource_options.lock() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.cached_resource_options.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::Kind;

    fn resource(priority: Priority, order: u32) -> Resource {
        let mut resource = Resource::new(Kind::Tile, "");
        resource.set_priority(priority);
        resource.order.set(order);
        resource
    }

    fn drain(pending: &mut PendingRequests) -> Vec<u64> {
        std::iter::from_fn(|| pending.pop()).collect()
    }

    #[test]
    fn pending_requests_start_in_arrival_order_by_default() {
        let mut pending = PendingRequests::default();
        for id in [3, 1, 2] {
            pending.insert(id, &Resource::new(Kind::Unknown, ""));
        }
        assert_eq!(drain(&mut pending), [3, 1, 2]);
    }

    #[test]
    fn pending_requests_start_by_priority_then_order_then_arrival() {
        let mut pending = PendingRequests::default();
        pending.insert(1, &resource(Priority::Low, 0));
        pending.insert(2, &resource(Priority::Regular, 7));
        pending.insert(3, &resource(Priority::Regular, 5));
        pending.insert(4, &resource(Priority::Low, 9));
        pending.insert(5, &resource(Priority::Regular, 7));
        pending.insert(6, &resource(Priority::Low, 0));
        assert_eq!(drain(&mut pending), [3, 2, 5, 1, 6, 4]);
    }

    /// A requester changes a waiting request's order through the resource it
    /// kept, a clone of the one it requested with.
    #[test]
    fn pending_request_moves_when_its_order_changes() {
        let mut pending = PendingRequests::default();
        let resources =
            [resource(Priority::Regular, 1), resource(Priority::Regular, 2), resource(Priority::Regular, 3)];
        for (id, resource) in (1..).zip(&resources) {
            pending.insert(id, &resource.clone());
        }

        resources[2].order.set(0);
        assert_eq!(pending.pop(), Some(3));

        resources[0].order.set(u32::MAX);
        assert_eq!(drain(&mut pending), [2, 1]);
    }

    /// A low priority request stays behind every regular one, whatever its
    /// order.
    #[test]
    fn pending_order_does_not_cross_priorities() {
        let mut pending = PendingRequests::default();
        let low = resource(Priority::Low, 5);
        pending.insert(1, &low);
        pending.insert(2, &resource(Priority::Regular, u32::MAX));
        low.order.set(0);
        assert_eq!(drain(&mut pending), [2, 1]);
    }

    /// Requests that leave their order alone, as those for styles, sources,
    /// sprites and glyphs do, start before every request that set one,
    /// however many of those came first.
    #[test]
    fn pending_requests_without_an_order_start_first() {
        let mut pending = PendingRequests::default();
        for id in 1..=100 {
            pending.insert(id, &resource(Priority::Regular, 1000 - id as u32));
        }
        pending.insert(101, &Resource::new(Kind::Glyphs, ""));
        pending.insert(102, &Resource::new(Kind::SpriteImage, ""));
        assert_eq!(pending.pop(), Some(101));
        assert_eq!(pending.pop(), Some(102));
        assert_eq!(pending.pop(), Some(100));
    }

    #[test]
    fn pending_request_removed_does_not_start() {
        let mut pending = PendingRequests::default();
        for (id, order) in [(1, 2), (2, 1), (3, 3)] {
            pending.insert(id, &resource(Priority::Regular, order));
        }
        assert!(pending.contains(2));
        pending.remove(2);
        assert!(!pending.contains(2));
        assert_eq!(drain(&mut pending), [1, 3]);
    }
}
