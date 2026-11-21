// The requests run in the online core (`OnlineCore`), which owns their state
// and runs the tasks of its queue (`OnlineQueue`): an `OnlineRequest` per
// caller request, started by a deadline (the first request at once, then
// revalidation when the response expires, or retries with backoff after
// errors), queued while `maximum_concurrent_requests` transfers are active
// (regular priority before low priority, and within a priority by the
// resource's `RequestOrder`), and performed by the HTTPFileSource.
// `requests` is keyed by id, so it iterates in id order.
//
// The resource transform's answers and the transfers' responses come back
// to the core as tasks, from whichever thread posts them, numbered with a
// generation of their request: an answer whose generation is no longer the
// one the request waits for is dropped, so a stale one can't end a transfer
// twice; they need the number because they may already be on their way
// when the core moves on. A deadline is a timed task of the core's own
// queue, which runs on the core's thread, so restarting or stopping a
// request's deadline, or cancelling the request, cancels that task instead.
//
// The core runs on a thread of its own, except in the browser, where the
// host's pump runs its queue (`OnlineFileSource::local`).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use mln_util::chrono::{Timestamp, now};
use mln_util::constants::{
    ASSET_PROTOCOL, CLOCK_SKEW_RETRY_TIMEOUT, DEFAULT_MAXIMUM_CONCURRENT_REQUESTS, FILE_PROTOCOL,
};
use mln_util::logging::{Event, Log};
use mln_util::queue::{Queue, TimedTask};
use mln_util::time::Instant;
use mln_vec_map::{VecMap, VecSet};

use crate::file_source::FileSource;
use crate::http_file_source::HTTPFileSource;
use crate::http_header_transform::HttpHeaderTransform;
use crate::http_timeout::{error_retry_timeout, expiration_timeout};
use crate::network_status::{NetworkStatus, Status, Subscription};
use crate::request::{Request, Responder};
use crate::resource::{Priority, RequestOrder, Resource};
use crate::resource_transform::{ResourceTransform, TransformResult};
use crate::response::{Reason, ResourceError, Response};

/// The queue of an [`OnlineCore`], whose tasks run in the order they were
/// posted. It stops once its source has dropped, and the queue's owner then
/// closes it.
pub type OnlineQueue = Queue<OnlineCore>;

struct OnlineRequest {
    resource: Resource,
    responder: Responder,
    /// The transfer in flight, and its generation.
    transfer: Option<(u64, Request)>,
    /// The generation of the resource transform's answer it waits for.
    transforming: Option<u64>,
    /// The task of the deadline that starts it again, while one runs.
    timer: Option<TimedTask>,

    /// Counts the number of times a response was already expired when
    /// received. We're using this to add a delay when making a new request
    /// so we don't keep retrying immediately in case of a server serving
    /// expired tiles.
    expired_requests: u32,

    /// Counts the number of subsequent failed requests. We're using this
    /// value for exponential backoff when retrying requests.
    failed_requests: u32,
    failed_request_reason: Reason,
    retry_after: Option<Timestamp>,

    /// Whether the resource transform failed the request. A retry calls the
    /// transform again instead of loading the original URL.
    transform_failed: bool,
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

impl OnlineRequest {
    /// When to request the resource again, or `None` for never.
    fn update_interval(&self, expires: Option<Timestamp>, connection_backoff: Option<Duration>) -> Option<Duration> {
        // Calculate a timeout that depends on how many consecutive errors
        // we've encountered, and on the expiration time, if present.
        let error_retry = match connection_backoff {
            Some(backoff) if self.failed_request_reason == Reason::Connection => Some(backoff),
            _ => error_retry_timeout(self.failed_request_reason, self.failed_requests, self.retry_after),
        };
        let minimum = self.resource.minimum_update_interval;
        let expiration = expiration_timeout(expires, self.expired_requests).map(|e| e.max(minimum));
        match (expiration, error_retry) {
            (Some(expiration), Some(error_retry)) => Some(expiration.min(error_retry)),
            (expiration, error_retry) => expiration.or(error_retry),
        }
    }
}

/// The state of an [`OnlineFileSource`]'s requests, which the tasks of its
/// [`OnlineQueue`] change on the thread that owns it.
pub struct OnlineCore {
    /// For the transform's answers, the transfers' responses and the
    /// deadlines.
    queue: OnlineQueue,
    /// The lifetime of a request is:
    ///
    /// 1. Waiting for its deadline (revalidation or retry)
    /// 2. Pending (waiting for room in the active set)
    /// 3. Active (open network connection)
    /// 4. Back to #1
    ///
    /// Requests in any state are in `requests`. Requests in the pending
    /// state are in `pending_requests`. Requests in the active state are in
    /// `active_requests`.
    requests: VecMap<u64, OnlineRequest>,
    pending_requests: PendingRequests,
    active_requests: VecSet<u64>,
    /// The last generation handed out.
    generation: u64,

    online: bool,
    maximum_concurrent_requests: u32,
    http_file_source: HTTPFileSource,
    resource_transform: Option<ResourceTransform>,
    network: Arc<NetworkStatus>,
    /// A task that tells the core the network is reachable again is on its
    /// way.
    reachable: Arc<AtomicBool>,
    /// Replaces the backoff after a connection error, so that a test can
    /// tell the retries it starts apart from the backoff's.
    connection_backoff: Option<Duration>,
    /// The clock that the deadlines go by, in place of the system's
    /// (`crate::bench`).
    #[cfg(feature = "bench")]
    clock: Option<crate::bench::Clock>,
}

impl OnlineCore {
    /// The time that deadlines go by.
    fn now(&self) -> Instant {
        #[cfg(feature = "bench")]
        if let Some(clock) = &self.clock {
            return clock();
        }
        Instant::now()
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// The resource transform's answer for request `id`, which it takes
    /// unless it no longer waits for answer `generation`.
    fn transformed(&mut self, id: u64, generation: u64, result: TransformResult) {
        if self.requests.get(&id).is_some_and(|req| req.transforming == Some(generation)) {
            self.set_transform_result(id, result);
        }
    }

    /// A transfer's response for request `id`, which ends its transfer
    /// unless transfer `generation` is no longer the one in flight.
    fn transferred(&mut self, id: u64, generation: u64, response: Response) {
        let in_flight = self.requests.get(&id).and_then(|req| req.transfer.as_ref()).map(|(g, _)| *g);
        if in_flight == Some(generation) {
            self.transfer_done(id, response);
        }
    }

    /// Starts request `id` again: what its deadline's task does. Cancelling
    /// or replacing the request's deadline cancels that task, so it runs
    /// only for the deadline that the request waits for.
    fn deadline_passed(&mut self, id: u64) {
        let Some(req) = self.requests.get_mut(&id) else { return };
        req.timer = None;
        if req.transform_failed {
            self.transform(id);
        } else {
            self.activate_or_queue_request(id);
        }
    }

    fn set_online(&mut self, online: bool) {
        self.online = online;
        if online {
            self.network_is_reachable_again();
        }
    }

    fn request(&mut self, id: u64, resource: Resource, responder: Responder) {
        // A request cancelled before this ran had its cancel run first,
        // which found nothing to cancel.
        if responder.is_cancelled() {
            return;
        }
        self.requests.insert(
            id,
            OnlineRequest {
                resource,
                responder,
                transfer: None,
                transforming: None,
                timer: None,
                expired_requests: 0,
                failed_requests: 0,
                failed_request_reason: Reason::Success,
                retry_after: None,
                transform_failed: false,
            },
        );
        self.transform(id);
    }

    fn transform(&mut self, id: u64) {
        let generation = self.next_generation();
        let transform = self.resource_transform.clone();
        let Some(req) = self.requests.get_mut(&id) else { return };
        req.transform_failed = false;
        let Some(transform) = transform else {
            self.activate(id);
            return;
        };
        // Ask the resource transform for a new URL, and replace the
        // resource's URL with the transformed one before scheduling the
        // request.
        req.transforming = Some(generation);
        let (kind, url) = (req.resource.kind, req.resource.url.clone());
        let queue = self.queue.clone();
        transform(
            kind,
            &url,
            Box::new(move |result: TransformResult| {
                queue.post(move |core| core.transformed(id, generation, result));
            }),
        );
    }

    fn set_transform_result(&mut self, id: u64, result: TransformResult) {
        let Some(req) = self.requests.get_mut(&id) else { return };
        req.transforming = None;
        match result {
            Ok(url) => {
                req.resource.url = url;
                self.activate(id);
            }
            Err(error) => {
                req.transform_failed = true;
                let mut response = Response::new();
                response.error = Some(Box::new(error));
                self.completed(id, response);
            }
        }
    }

    fn cancel(&mut self, id: u64) {
        if self.active_requests.remove(&id) {
            self.activate_pending_request();
        } else {
            self.pending_requests.remove(id);
        }
        if let Some(req) = self.requests.remove(&id) {
            if let Some(timer) = req.timer {
                self.queue.cancel(timer);
            }
            drop(req);
        }
    }

    fn activate_or_queue_request(&mut self, id: u64) {
        let Some(req) = self.requests.get(&id) else { return };
        debug_assert!(!self.active_requests.contains(&id));
        debug_assert!(req.transfer.is_none());

        if self.active_requests.len() >= self.maximum_concurrent_requests as usize {
            self.pending_requests.insert(id, &req.resource);
        } else {
            self.activate_request(id);
        }
    }

    fn activate_request(&mut self, id: u64) {
        self.active_requests.insert(id);

        if self.online {
            let generation = self.next_generation();
            let queue = self.queue.clone();
            let Some(req) = self.requests.get_mut(&id) else { return };
            let (request, responder) = Request::new(move |response: Response| {
                queue.post(move |core| core.transferred(id, generation, response));
            });
            req.transfer = Some((generation, request));
            self.http_file_source.start(&req.resource, responder);
        } else {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Connection, "Online connectivity is disabled.")));
            self.transfer_done(id, response);
        }
    }

    /// Ends request `id`'s transfer with `response`, and starts a pending
    /// request in its place.
    fn transfer_done(&mut self, id: u64, response: Response) {
        self.active_requests.remove(&id);
        if let Some(req) = self.requests.get_mut(&id) {
            // The transfer is done; dropping its request is a no-op.
            let finished = req.transfer.take();
            drop(finished);
            self.completed(id, response);
        }
        self.activate_pending_request();
    }

    fn activate_pending_request(&mut self) {
        if let Some(id) = self.pending_requests.pop()
            && self.requests.contains_key(&id)
        {
            self.activate_request(id);
        }
    }

    fn network_is_reachable_again(&mut self) {
        let mut ids: Vec<u64> = self.requests.keys().copied().collect();
        // Notify the requests in the order they would start in.
        ids.sort_by_key(|id| {
            let resource = &self.requests[id].resource;
            start_key(resource.priority, &resource.order)
        });
        for id in ids {
            // We need all requests to fail at least once before we are
            // going to start retrying them, and we only immediately restart
            // request that failed due to connection issues.
            if self.requests.get(&id).is_some_and(|req| req.failed_request_reason == Reason::Connection) {
                self.schedule(id, Some(Duration::ZERO));
            }
        }
    }

    fn activate(&mut self, id: u64) {
        let Some(req) = self.requests.get(&id) else { return };
        // Force an immediate first request if we don't have an expiration time.
        let timeout = match req.resource.prior_expires {
            Some(prior_expires) => req.update_interval(Some(prior_expires), self.connection_backoff),
            None => Some(Duration::ZERO),
        };
        self.schedule(id, timeout);
    }

    /// Requests the resource again after `timeout`, or never for `None`.
    fn schedule(&mut self, id: u64, timeout: Option<Duration>) {
        let now = self.now();
        let in_progress = self.pending_requests.contains(id) || self.active_requests.contains(&id);
        let offline = self.network.get() == Status::Offline;
        let Some(req) = self.requests.get_mut(&id) else { return };
        if req.transforming.is_some() || in_progress {
            // There's already a request in progress; don't start another one.
            return;
        }

        let Some(timeout) = timeout else {
            return;
        };

        if let Some(timer) = req.timer.take() {
            self.queue.cancel(timer);
        }

        // Emulate a Connection error when the Offline mode is forced, and
        // don't retry: the request will get re-triggered when the
        // NetworkStatus is set back to Online.
        if offline {
            req.failed_request_reason = Reason::Connection;
            req.failed_requests = 1;
            return;
        }

        // A deadline past what `Instant` can hold never comes.
        if let Some(due) = now.checked_add(timeout) {
            req.timer = Some(self.queue.post_at(due, move |core| core.deadline_passed(id)));
        }
    }

    fn completed(&mut self, id: u64, mut response: Response) {
        let connection_backoff = self.connection_backoff;
        let Some(req) = self.requests.get_mut(&id) else { return };
        let resource = &mut req.resource;
        // If we didn't get various caching headers in the response, continue
        // using the previous values. Otherwise, update the previous values
        // to the new values.
        if response.modified.is_none() {
            response.modified = resource.prior_modified;
        } else {
            resource.prior_modified = response.modified;
        }

        if response.not_modified && resource.prior_data.is_some() {
            // When the priorData field is set, it indicates that we had to
            // revalidate the request and that the requestor hasn't gotten
            // data yet. If we get a 304 response, this means that we have
            // send the cached data to give the requestor a chance to
            // actually obtain the data.
            response.data = resource.prior_data.take();
            response.not_modified = false;
        }

        let mut is_expired = false;

        if let Some(expires) = response.expires {
            let prior = resource.prior_expires.replace(expires);
            response.expires = Some(interpolate_expiration(expires, prior, &mut is_expired));
        }

        if is_expired {
            req.expired_requests += 1;
        } else {
            req.expired_requests = 0;
        }

        let resource = &mut req.resource;
        if response.etag.is_none() {
            response.etag = resource.prior_etag.clone();
        } else {
            resource.prior_etag = response.etag.clone();
        }

        if let Some(error) = &response.error {
            if error.reason == Reason::NotFound {
                Log::error(Event::General, format!("The resource `{}` not found", sanitize_url(&resource.url)));
            }
            req.failed_requests += 1;
            req.failed_request_reason = error.reason;
            req.retry_after = error.retry_after;
        } else {
            req.failed_requests = 0;
            req.failed_request_reason = Reason::Success;
        }

        let interval = req.update_interval(response.expires, connection_backoff);
        self.schedule(id, interval);

        if let Some(req) = self.requests.get(&id) {
            req.responder.send(response);
        }
    }
}

/// The `Network` file source: HTTP(S) requests, revalidated when they
/// expire and retried with a backoff after errors, with at most
/// [`maximum_concurrent_requests`](Self::maximum_concurrent_requests) in
/// flight. Its requests follow the [`NetworkStatus`] it was made with.
///
/// Outside the browser its requests run on a thread of its own, which its
/// drop waits for, unless it drops on that thread. When that thread can't
/// start, the source logs why and fails each request.
pub struct OnlineFileSource {
    maximum_concurrent_requests: AtomicU32,
    /// The id of the next request.
    next_request_id: AtomicU64,
    queue: OnlineQueue,
    _reachability: Subscription,
    #[cfg(not(target_family = "wasm"))]
    thread: Option<std::thread::JoinHandle<()>>,
    /// The caller runs the core, rather than a thread (`crate::bench`).
    #[cfg(all(feature = "bench", not(target_family = "wasm")))]
    driven: bool,
}

#[cfg(target_family = "wasm")]
impl OnlineCore {
    /// Sets where the browser's fetch looks for a response that the host
    /// fetched ahead before it fetches a whole resource: `take(url)`
    /// returns a promise of the `Response` fetched for exactly `url`, at
    /// most once for each, or undefined. A promise that rejects is fetched
    /// again. `None` stops looking.
    pub fn set_fetch_preloads(&mut self, take: Option<js_sys::Function>) {
        self.http_file_source.set_fetch_preloads(take);
    }
}

impl OnlineFileSource {
    /// A source whose requests run on a thread of its own and follow
    /// `network`.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn new(network: Arc<NetworkStatus>) -> Self {
        let (mut source, core) = Self::with_core(network);
        let queue = source.queue.clone();
        // The core, and its HTTP client, are made on the thread.
        let thread = mln_util::thread::spawn("OnlineFileSource", move || {
            let mut core = core();
            queue.run(&mut core);
            // What comes once the source has dropped, such as a transfer's
            // late response, drops at once.
            queue.close();
            drop(core);
        });
        match thread {
            Ok(thread) => source.thread = Some(thread),
            Err(error) => {
                source.queue.close();
                Log::error(Event::General, format!("The online file source's thread didn't start: {error}"));
            }
        }
        source
    }

    /// A source whose requests follow `network` and run in the returned
    /// core, with the returned queue, which the caller runs on this thread
    /// with [`Queue::run_once`] until the queue
    /// [`is_stopped`](Queue::is_stopped), then closes. The queue's waker
    /// tells the caller when it has tasks, and its
    /// [`next_deadline`](Queue::next_deadline) when a retry or a
    /// revalidation is due.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn local(network: Arc<NetworkStatus>) -> (Self, OnlineQueue, OnlineCore) {
        let (source, core) = Self::with_core(network);
        let queue = source.queue.clone();
        (source, queue, core())
    }

    /// The source, and what makes its core.
    fn with_core(network: Arc<NetworkStatus>) -> (Self, impl FnOnce() -> OnlineCore + Send + 'static) {
        let queue = OnlineQueue::new();
        let reachable = Arc::new(AtomicBool::new(false));
        let reachability = {
            let (queue, reachable) = (queue.clone(), Arc::clone(&reachable));
            network.subscribe(move || {
                if !reachable.swap(true, Ordering::SeqCst) {
                    queue.post(|core| {
                        core.reachable.store(false, Ordering::SeqCst);
                        core.network_is_reachable_again();
                    });
                }
            })
        };
        let core_queue = queue.clone();
        let core = move || OnlineCore {
            queue: core_queue,
            requests: VecMap::new(),
            pending_requests: PendingRequests::default(),
            active_requests: VecSet::new(),
            generation: 0,
            online: true,
            maximum_concurrent_requests: DEFAULT_MAXIMUM_CONCURRENT_REQUESTS,
            http_file_source: HTTPFileSource::new(),
            resource_transform: None,
            network,
            reachable,
            connection_backoff: None,
            #[cfg(feature = "bench")]
            clock: None,
        };
        let source = OnlineFileSource {
            maximum_concurrent_requests: AtomicU32::new(DEFAULT_MAXIMUM_CONCURRENT_REQUESTS),
            next_request_id: AtomicU64::new(1),
            queue,
            _reachability: reachability,
            #[cfg(not(target_family = "wasm"))]
            thread: None,
            #[cfg(all(feature = "bench", not(target_family = "wasm")))]
            driven: false,
        };
        (source, core)
    }

    /// A source whose core the caller runs on its own thread, as
    /// [`local`](Self::local)'s, whose transfers go to `transport` and
    /// whose deadlines go by `clock` (`crate::bench`).
    #[cfg(all(feature = "bench", not(target_family = "wasm")))]
    pub(crate) fn driven(
        network: Arc<NetworkStatus>,
        transport: Arc<dyn crate::http_file_source::HttpTransport>,
        clock: crate::bench::Clock,
    ) -> (Self, OnlineQueue, OnlineCore) {
        let (mut source, core) = Self::with_core(network);
        source.driven = true;
        let mut core = core();
        core.http_file_source = HTTPFileSource::with_transport(transport);
        core.clock = Some(clock);
        let queue = source.queue.clone();
        (source, queue, core)
    }

    /// The most transfers in flight at once; the requests beyond wait.
    #[must_use]
    pub fn maximum_concurrent_requests(&self) -> u32 {
        self.maximum_concurrent_requests.load(Ordering::Relaxed)
    }

    /// Sets the most transfers in flight at once.
    pub fn set_maximum_concurrent_requests(&self, maximum: u32) {
        self.maximum_concurrent_requests.store(maximum, Ordering::Relaxed);
        self.queue.post(move |core| core.maximum_concurrent_requests = maximum);
    }

    /// Takes the source offline or back online. Offline, every transfer
    /// fails with a connection error; back online, the requests that failed
    /// that way retry at once.
    pub fn set_online(&self, online: bool) {
        self.queue.post(move |core| core.set_online(online));
    }

    /// Rewrites the URLs of requests before they load, or stops rewriting
    /// them for `None`.
    pub fn set_resource_transform(&self, transform: Option<ResourceTransform>) {
        self.queue.post(move |core| core.resource_transform = transform);
    }

    /// Sets the headers to add to each transfer.
    pub fn set_http_header_transform(&self, transform: HttpHeaderTransform) {
        self.queue.post(move |core| core.http_file_source.set_http_header_transform(transform));
    }

    /// Holds the core until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(|hold| self.queue.post(move |_| hold()))
    }

    /// Waits `backoff` to retry a request that failed with a connection
    /// error, in place of the exponential backoff.
    #[cfg(test)]
    pub(crate) fn set_connection_backoff(&self, backoff: Duration) {
        self.queue.post(move |core| core.connection_backoff = Some(backoff));
    }

    /// Whether a thread, or the caller, runs the source's core.
    #[cfg(not(target_family = "wasm"))]
    fn is_running(&self) -> bool {
        #[cfg(feature = "bench")]
        if self.driven {
            return true;
        }
        self.thread.is_some()
    }

    /// Whether a thread runs the source's core: in the browser the host's
    /// does.
    #[cfg(target_family = "wasm")]
    #[expect(clippy::unused_self, reason = "the threads of other builds can fail to start")]
    fn is_running(&self) -> bool {
        true
    }
}

impl Drop for OnlineFileSource {
    fn drop(&mut self) {
        self.queue.stop();
        #[cfg(not(target_family = "wasm"))]
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

impl FileSource for OnlineFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        if !self.is_running() {
            let mut response = Response::new();
            response.error =
                Some(Box::new(ResourceError::new(Reason::Other, "The online file source's thread isn't running.")));
            responder.send(response);
            return;
        }
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let queue = self.queue.clone();
        responder.on_cancel(move || queue.post(move |core| core.cancel(id)));
        let resource = resource.clone();
        self.queue.post(move |core| core.request(id, resource, responder));
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(crate::resource::LoadingMethod::Network)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use mln_test_server::closed_port_url;
    use parking_lot::Mutex;

    use super::*;
    use crate::resource::Kind;
    use crate::resource_transform::FinishedCallback;

    /// A core that no thread runs, whose tasks the test runs or calls
    /// itself, and the source whose drop ends it.
    fn core(maximum: u32) -> (OnlineFileSource, OnlineCore) {
        let (source, core) = OnlineFileSource::with_core(Arc::new(NetworkStatus::new()));
        let mut core = core();
        core.maximum_concurrent_requests = maximum;
        (source, core)
    }

    fn request(core: &mut OnlineCore, id: u64, url: &str) -> Request {
        let (request, responder) = Request::new(|_| {});
        core.request(id, Resource::new(Kind::Unknown, url), responder);
        request
    }

    /// Passes request `id`'s deadline, as the queue does once it's due: it
    /// takes the deadline's task and runs it. The test calls this rather
    /// than running the queue, which would also run the responses of the
    /// transfers that it starts.
    fn pass_deadline(core: &mut OnlineCore, id: u64) {
        let timer = core.requests[&id].timer.expect("a deadline");
        core.queue.cancel(timer);
        core.deadline_passed(id);
    }

    /// The generation of request `id`'s transfer in flight.
    fn transfer(core: &OnlineCore, id: u64) -> Option<u64> {
        core.requests[&id].transfer.as_ref().map(|(generation, _)| *generation)
    }

    /// A response to a transfer that ended, or that never was, ends
    /// nothing: it neither starts a waiting request twice nor starts one
    /// beyond the maximum.
    #[test]
    fn a_stale_transfer_response_is_dropped() {
        let (_source, mut core) = core(1);
        let url = closed_port_url("/stale");
        let _first = request(&mut core, 1, &url);
        let _second = request(&mut core, 2, &url);
        pass_deadline(&mut core, 1);
        pass_deadline(&mut core, 2);
        let first = transfer(&core, 1).expect("the first request's transfer");
        assert!(core.pending_requests.contains(2));

        core.transferred(1, first + 1000, Response::new());
        assert_eq!(Some(first), transfer(&core, 1));
        assert!(core.pending_requests.contains(2));

        core.transferred(1, first, Response::new());
        assert_eq!(None, transfer(&core, 1));
        let second = transfer(&core, 2).expect("the second request's transfer");
        assert_eq!(1, core.active_requests.len());

        core.transferred(1, first, Response::new());
        assert_eq!(Some(second), transfer(&core, 2));
        assert_eq!(1, core.active_requests.len());
        assert!(!core.pending_requests.contains(2));
    }

    /// An answer of the resource transform that the request no longer waits
    /// for changes nothing.
    #[test]
    fn a_stale_transform_answer_is_dropped() {
        let (_source, mut core) = core(1);
        let answers: Arc<Mutex<Vec<FinishedCallback>>> = Arc::default();
        let saved = Arc::clone(&answers);
        core.resource_transform = Some(Arc::new(move |_, _: &str, finished| saved.lock().push(finished)));
        let original = "http://example.invalid/original";
        let _request = request(&mut core, 1, original);
        let generation = core.requests[&1].transforming.expect("a transform");

        core.transformed(1, generation - 1, Ok("stale".to_owned()));
        assert_eq!(Some(generation), core.requests[&1].transforming);
        assert_eq!(original, core.requests[&1].resource.url);

        let url = closed_port_url("/transformed");
        let finished = answers.lock().pop().expect("the transform's callback");
        finished(Ok(url.clone()));
        let queue = core.queue.clone();
        assert!(queue.run_once(&mut core), "the transform's answer");
        assert_eq!(None, core.requests[&1].transforming);
        assert_eq!(url, core.requests[&1].resource.url);
    }

    /// A response that expires, for a resource whose minimum update
    /// interval is longer than `Instant` can count, schedules no update.
    #[test]
    fn an_update_interval_past_instants_range_never_fires() {
        let (_source, mut core) = core(1);
        let mut resource = Resource::new(Kind::Tile, closed_port_url("/forever"));
        resource.minimum_update_interval = Duration::MAX;
        let (_request, responder) = Request::new(|_| {});
        core.request(1, resource, responder);
        pass_deadline(&mut core, 1);
        let generation = transfer(&core, 1).expect("the request's transfer");

        let mut response = Response::new();
        response.data = Some(Arc::new(b"tile".to_vec()));
        response.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 60));
        core.transferred(1, generation, response);
        assert_eq!(None, core.requests[&1].timer);
        assert_eq!(None, core.queue.next_deadline());
    }

    /// Replacing a request's deadline, or cancelling the request, cancels
    /// the deadline's task, which leaves the queue.
    #[test]
    fn a_replaced_or_cancelled_deadline_leaves_the_queue() {
        let (_source, mut core) = core(1);
        core.connection_backoff = Some(Duration::from_secs(3600));
        let _request = request(&mut core, 1, &closed_port_url("/retry"));
        pass_deadline(&mut core, 1);
        let generation = transfer(&core, 1).expect("the request's transfer");
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Connection, "refused")));
        core.transferred(1, generation, response);
        let retry = core.queue.next_deadline().expect("the retry");
        assert!(retry > Instant::now() + Duration::from_secs(60));

        core.network_is_reachable_again();
        let again = core.queue.next_deadline().expect("the retry, at once");
        assert!(again < retry);

        core.cancel(1);
        assert_eq!(None, core.queue.next_deadline());
    }

    /// The last owner of a source may drop it on the source's own thread.
    #[test]
    fn a_source_dropped_on_its_own_thread_ends() {
        let source = OnlineFileSource::new(Arc::new(NetworkStatus::new()));
        let queue = source.queue.clone();
        let (dropped, done) = mpsc::channel();
        queue.post(move |_| {
            drop(source);
            dropped.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(10)).expect("the drop returns");
    }

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
