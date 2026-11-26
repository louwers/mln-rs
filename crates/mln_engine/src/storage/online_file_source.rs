// The requests run in the online core (`OnlineCore`), which owns their state
// and runs the tasks of its queue (`OnlineQueue`): an `OnlineRequest` per
// caller request, started by a deadline (the first request at once, then
// revalidation when a response that came fresh expires, or retries with
// backoff after errors), queued while `maximum_concurrent_requests` transfers are active
// (regular priority before low priority, and within a priority by the
// resource's `RequestOrder`), and performed by the HTTPFileSource.
// `requests` is keyed by id, so it iterates in id order.
//
// Requests whose transfers would be the same, of one URL, byte range and
// validators, share one: a request due while that transfer is in flight, or
// waiting for room when it starts, joins it, and each request takes its
// response as its own. A shared transfer takes one place in the active set,
// and is cancelled when the last of its requests is.
//
// The resource transform's answers and the transfers' responses come back
// to the core as tasks, from whichever thread posts them, numbered with a
// generation: an answer whose generation is no longer the one its request
// waits for, or a response to a transfer no longer in flight, is dropped,
// so a stale one can't end a transfer twice; they need the number because
// they may already be on their way when the core moves on. A deadline is a timed task of the core's own
// queue, which runs on the core's thread, so restarting or stopping a
// request's deadline, or cancelling the request, cancels that task instead.
//
// The core runs on a thread of its own, except in the browser, where the
// host's pump runs its queue (`OnlineFileSource::local`).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use mln_style::time::Instant;
use mln_util::chrono::{Timestamp, now};
use mln_util::constants::{ASSET_PROTOCOL, DEFAULT_MAXIMUM_CONCURRENT_REQUESTS, FILE_PROTOCOL};
use mln_util::queue::{Queue, TimedTask};
use mln_vec_map::VecMap;
#[cfg(not(target_family = "wasm"))]
use parking_lot::Mutex;

use crate::log_target;
use crate::storage::file_source::FileSource;
use crate::storage::http_file_source::HTTPFileSource;
use crate::storage::http_header_transform::HttpHeaderTransform;
use crate::storage::http_timeout::{error_retry_timeout, until};
use crate::storage::network_status::{NetworkStatus, Status, Subscription};
use crate::storage::request::{Request, Responder};
use crate::storage::resource::{Priority, RequestOrder, Resource};
use crate::storage::resource_transform::{ResourceTransform, TransformResult};
use crate::storage::response::{Reason, ResourceError, Response};

/// The queue of an [`OnlineCore`], whose tasks run in the order they were
/// posted. It stops once its source has dropped, and the queue's owner then
/// closes it.
pub type OnlineQueue = Queue<OnlineCore>;

struct OnlineRequest {
    resource: Resource,
    responder: Responder,
    /// The generation of the transfer in flight that it waits for.
    transfer: Option<u64>,
    /// The generation of the resource transform's answer it waits for.
    transforming: Option<u64>,
    /// The task of the deadline that starts it again, while one runs.
    timer: Option<TimedTask>,

    /// Counts the number of subsequent failed requests. We're using this
    /// value for exponential backoff when retrying requests.
    failed_requests: u32,
    failed_request_reason: Reason,
    retry_after: Option<Timestamp>,

    /// Whether the resource transform failed the request. A retry calls the
    /// transform again instead of loading the original URL.
    transform_failed: bool,
}

/// A transfer in flight, for the requests that wait for it.
struct Transfer {
    key: TransferKey,
    /// Cancels the transfer when it drops.
    _request: Request,
    /// The requests that wait for it, in the order they joined.
    waiters: Vec<u64>,
}

/// What a transfer of a resource sends, which requests that share a
/// transfer have in common.
#[derive(PartialEq)]
struct TransferKey {
    kind: crate::storage::resource::Kind,
    url: String,
    data_range: Option<(u64, u64)>,
    prior_etag: Option<String>,
    prior_modified: Option<Timestamp>,
}

impl TransferKey {
    fn of(resource: &Resource) -> TransferKey {
        TransferKey {
            kind: resource.kind,
            url: resource.url.clone(),
            data_range: resource.data_range,
            prior_etag: resource.prior_etag.clone(),
            prior_modified: resource.prior_modified,
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

    /// Removes the requests that `joins` picks, in the order they started
    /// to wait.
    fn take(&mut self, mut joins: impl FnMut(u64) -> bool) -> Vec<u64> {
        let mut taken = Vec::new();
        self.requests.retain(|request| {
            let join = joins(request.id);
            if join {
                taken.push(request.id);
            }
            !join
        });
        taken
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

/// The URL without its query string.
fn sanitize_url(url: &str) -> &str {
    match url.find('?') {
        Some(query_index) => &url[..query_index],
        None => url,
    }
}

impl OnlineRequest {
    /// When to request the resource again after a response: after an error,
    /// its retry's backoff; else when the data expires, if it came fresh. A
    /// response that came stale, as one without a freshness lifetime,
    /// isn't asked for again until it is requested anew.
    fn update_interval(&self, response: &Response) -> Option<Duration> {
        if response.error.is_some() {
            return error_retry_timeout(self.failed_request_reason, self.failed_requests, self.retry_after);
        }
        response
            .caching
            .expires
            .filter(|&expires| expires > now())
            .map(|expires| until(expires).max(self.resource.minimum_update_interval))
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
    /// 3. Active (waiting for a transfer in flight, maybe shared)
    /// 4. Back to #1
    ///
    /// Requests in any state are in `requests`. Requests in the pending
    /// state are in `pending_requests`. The transfers of requests in the
    /// active state are in `transfers`, by their generation: the active set.
    requests: VecMap<u64, OnlineRequest>,
    pending_requests: PendingRequests,
    transfers: VecMap<u64, Transfer>,
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
    /// The clock that the deadlines go by, in place of the system's
    /// (`crate::storage::simulated_network`).
    #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
    clock: Option<crate::storage::simulated_network::Clock>,
}

impl OnlineCore {
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

    /// Transfer `generation`'s response, which goes to each request that
    /// waits for it, unless the transfer is no longer in flight. Starts the
    /// pending requests that the room it leaves allows.
    fn transferred(&mut self, generation: u64, response: &Response) {
        let Some(transfer) = self.transfers.remove(&generation) else { return };
        for id in transfer.waiters {
            if let Some(req) = self.requests.get_mut(&id) {
                req.transfer = None;
                self.completed(id, response.clone());
            }
        }
        self.activate_pending_requests();
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
        self.pending_requests.remove(id);
        let Some(req) = self.requests.remove(&id) else { return };
        if let Some(timer) = req.timer {
            self.queue.cancel(timer);
        }
        // The transfer goes on while another request waits for it.
        if let Some(generation) = req.transfer
            && let Some(transfer) = self.transfers.get_mut(&generation)
        {
            transfer.waiters.retain(|&waiter| waiter != id);
            if transfer.waiters.is_empty() {
                self.transfers.remove(&generation);
                self.activate_pending_requests();
            }
        }
    }

    /// Starts request `id`: it joins the transfer in flight that it shares,
    /// or starts its own if the active set has room, or waits for room.
    fn activate_or_queue_request(&mut self, id: u64) {
        let Some(req) = self.requests.get(&id) else { return };
        debug_assert!(req.transfer.is_none());
        let key = TransferKey::of(&req.resource);
        let shared = self.transfers.iter().find(|(_, transfer)| transfer.key == key).map(|(&generation, _)| generation);
        if let Some(generation) = shared {
            if let Some(transfer) = self.transfers.get_mut(&generation) {
                transfer.waiters.push(id);
            }
            if let Some(req) = self.requests.get_mut(&id) {
                req.transfer = Some(generation);
            }
        } else if self.transfers.len() >= self.maximum_concurrent_requests as usize {
            self.pending_requests.insert(id, &req.resource);
        } else {
            self.activate_request(id);
        }
    }

    /// Starts request `id`'s transfer, which the pending requests that
    /// share it join.
    fn activate_request(&mut self, id: u64) {
        if !self.online {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Connection, "Online connectivity is disabled.")));
            self.completed(id, response);
            return;
        }
        let generation = self.next_generation();
        let queue = self.queue.clone();
        let Some(req) = self.requests.get_mut(&id) else { return };
        let (request, responder) = Request::new(move |response: Response| {
            queue.post(move |core| core.transferred(generation, &response));
        });
        req.transfer = Some(generation);
        let key = TransferKey::of(&req.resource);
        self.http_file_source.start(&req.resource, responder);
        let requests = &self.requests;
        let joined = self
            .pending_requests
            .take(|pending| requests.get(&pending).is_some_and(|pending| TransferKey::of(&pending.resource) == key));
        for &pending in &joined {
            if let Some(req) = self.requests.get_mut(&pending) {
                req.transfer = Some(generation);
            }
        }
        let mut waiters = vec![id];
        waiters.extend(joined);
        self.transfers.insert(generation, Transfer { key, _request: request, waiters });
    }

    /// Starts pending requests while the active set has room.
    fn activate_pending_requests(&mut self) {
        while self.transfers.len() < self.maximum_concurrent_requests as usize
            && let Some(id) = self.pending_requests.pop()
        {
            if self.requests.contains_key(&id) {
                self.activate_request(id);
            }
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
        // With the network forced offline, a request starts again once the
        // network is back. A requester that shows no copy of the resource
        // hears at once that it failed to connect; one that does keeps it
        // meanwhile.
        if self.network.get() == Status::Offline {
            let Some(req) = self.requests.get_mut(&id) else { return };
            req.failed_request_reason = Reason::Connection;
            req.failed_requests = 1;
            let resource = &req.resource;
            let has_copy = resource.prior_data.is_none()
                && (resource.prior_caching.expires.is_some()
                    || resource.prior_etag.is_some()
                    || resource.prior_modified.is_some());
            if !has_copy {
                let mut error = Response::new();
                error.error =
                    Some(Box::new(ResourceError::new(Reason::Connection, "Online connectivity is disabled.")));
                req.responder.send(error);
            }
            return;
        }
        let Some(req) = self.requests.get(&id) else { return };
        // A copy that is still fresh is revalidated once it expires, and not
        // before the resource's minimum update interval; anything else at once.
        let timeout = match req.resource.prior_caching.expires {
            Some(prior_expires) => until(prior_expires).max(req.resource.minimum_update_interval),
            None => Duration::ZERO,
        };
        self.schedule(id, Some(timeout));
    }

    /// Requests the resource again after `timeout`, or never for `None`.
    fn schedule(&mut self, id: u64, timeout: Option<Duration>) {
        // The time that deadlines go by.
        #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
        let now = self.clock.as_ref().map_or_else(Instant::now, |clock| clock());
        #[cfg(not(all(any(test, feature = "bench"), not(target_family = "wasm"))))]
        let now = Instant::now();
        let pending = self.pending_requests.contains(id);
        let offline = self.network.get() == Status::Offline;
        let Some(req) = self.requests.get_mut(&id) else { return };
        if req.transforming.is_some() || pending || req.transfer.is_some() {
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

    fn completed(&mut self, id: u64, response: Response) {
        let Some(req) = self.requests.get_mut(&id) else { return };
        if let Some(error) = &response.error {
            if error.reason == Reason::NotFound {
                log::error!(
                    target: log_target::GENERAL,
                    "The resource `{}` not found",
                    sanitize_url(&req.resource.url)
                );
            }
            req.failed_requests += 1;
            req.failed_request_reason = error.reason;
            req.retry_after = error.retry_after;
        } else {
            req.failed_requests = 0;
            req.failed_request_reason = Reason::Success;
        }
        // After an error, the request retries, even while the requester
        // shows a stale copy.
        let interval = req.update_interval(&response);

        let resource = &mut req.resource;
        let response = resource.deliver(response);
        if response.error.is_none() {
            // The next revalidation is of this copy.
            resource.set_prior(&response);
        }

        self.schedule(id, interval);

        if let Some(req) = self.requests.get(&id) {
            req.responder.send(response);
        }
    }
}

/// The `Network` file source: HTTP(S) requests, revalidated when they
/// expire and retried with a backoff after errors, with at most
/// [`maximum_concurrent_requests`](Self::maximum_concurrent_requests)
/// transfers in flight. Requests that would send the same transfer at once
/// share it. Its requests follow the [`NetworkStatus`] it was made with.
///
/// Outside the browser its requests run on a thread of its own, which its
/// drop or [`shut_down`](Self::shut_down) waits for, unless it runs on that
/// thread. When that thread can't start, the source logs why and fails each
/// request.
pub struct OnlineFileSource {
    maximum_concurrent_requests: AtomicU32,
    /// The id of the next request.
    next_request_id: AtomicU64,
    queue: OnlineQueue,
    _reachability: Subscription,
    /// The thread that runs the core, until the source shuts down.
    #[cfg(not(target_family = "wasm"))]
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The caller runs the core, rather than a thread
    /// (`crate::storage::simulated_network`).
    #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
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
            Ok(thread) => *source.thread.get_mut() = Some(thread),
            Err(error) => {
                source.queue.close();
                log::error!(target: log_target::GENERAL, "The online file source's thread didn't start: {error}");
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
            transfers: VecMap::new(),
            generation: 0,
            online: true,
            maximum_concurrent_requests: DEFAULT_MAXIMUM_CONCURRENT_REQUESTS,
            http_file_source: HTTPFileSource::new(),
            resource_transform: None,
            network,
            reachable,
            #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
            clock: None,
        };
        let source = OnlineFileSource {
            maximum_concurrent_requests: AtomicU32::new(DEFAULT_MAXIMUM_CONCURRENT_REQUESTS),
            next_request_id: AtomicU64::new(1),
            queue,
            _reachability: reachability,
            #[cfg(not(target_family = "wasm"))]
            thread: Mutex::new(None),
            #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
            driven: false,
        };
        (source, core)
    }

    /// A source whose core the caller runs on its own thread, as
    /// [`local`](Self::local)'s, whose transfers go to `transport` and
    /// whose deadlines go by `clock` (`crate::storage::simulated_network`).
    #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
    pub(crate) fn driven(
        network: Arc<NetworkStatus>,
        transport: Arc<dyn crate::storage::http_file_source::HttpTransport>,
        clock: crate::storage::simulated_network::Clock,
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

    /// Whether a thread, or the caller, runs the source's core.
    #[cfg(not(target_family = "wasm"))]
    fn is_running(&self) -> bool {
        #[cfg(all(any(test, feature = "bench"), not(target_family = "wasm")))]
        if self.driven {
            return true;
        }
        self.thread.lock().is_some()
    }

    /// Whether a thread runs the source's core: in the browser the host's
    /// does.
    #[cfg(target_family = "wasm")]
    #[expect(clippy::unused_self, reason = "the threads of other builds can fail to start")]
    fn is_running(&self) -> bool {
        true
    }

    /// Stops the source, as its drop does, for an owner that must know its
    /// thread has ended although the source may be held elsewhere. The first
    /// call waits for that thread, unless it runs on it; a later call
    /// returns at once, so one owner shuts it down. Requests in flight end
    /// without a response, and later ones fail as when the thread didn't
    /// start.
    pub fn shut_down(&self) {
        self.queue.stop();
        #[cfg(not(target_family = "wasm"))]
        {
            // Released before the join, as the thread may load from this
            // source meanwhile.
            let thread = self.thread.lock().take();
            if let Some(thread) = thread
                && thread.thread().id() != std::thread::current().id()
            {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for OnlineFileSource {
    fn drop(&mut self) {
        self.shut_down();
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
        resource.has_loading_method(crate::storage::resource::LoadingMethod::Network)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use parking_lot::Mutex;

    use super::*;
    use crate::storage::http_file_source::{HttpTransport, TransferRequest};
    use crate::storage::resource::Kind;
    use crate::storage::resource_transform::FinishedCallback;

    /// Keeps each transfer in flight, unanswered.
    #[derive(Default)]
    struct Unanswered(Mutex<Vec<TransferRequest>>);

    impl HttpTransport for Unanswered {
        fn start(&self, request: TransferRequest) {
            self.0.lock().push(request);
        }
    }

    /// A core that no thread runs, whose tasks the test runs or calls
    /// itself, whose transfers stay in flight, and the source whose drop
    /// ends it.
    fn core(maximum: u32) -> (OnlineFileSource, OnlineCore) {
        let (source, core) = OnlineFileSource::with_core(Arc::new(NetworkStatus::new()));
        let mut core = core();
        core.http_file_source = HTTPFileSource::with_transport(Arc::new(Unanswered::default()));
        core.maximum_concurrent_requests = maximum;
        (source, core)
    }

    /// The URL of `path` on a server that never answers.
    fn url(path: &str) -> String {
        format!("http://example.invalid{path}")
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
        core.requests[&id].transfer
    }

    /// A response to a transfer that ended, or that never was, ends
    /// nothing: it neither starts a waiting request twice nor starts one
    /// beyond the maximum.
    #[test]
    fn a_stale_transfer_response_is_dropped() {
        let (_source, mut core) = core(1);
        let _first = request(&mut core, 1, &url("/stale-1"));
        let _second = request(&mut core, 2, &url("/stale-2"));
        pass_deadline(&mut core, 1);
        pass_deadline(&mut core, 2);
        let first = transfer(&core, 1).expect("the first request's transfer");
        assert!(core.pending_requests.contains(2));

        core.transferred(first + 1000, &Response::new());
        assert_eq!(Some(first), transfer(&core, 1));
        assert!(core.pending_requests.contains(2));

        core.transferred(first, &Response::new());
        assert_eq!(None, transfer(&core, 1));
        let second = transfer(&core, 2).expect("the second request's transfer");
        assert_eq!(1, core.transfers.len());

        core.transferred(first, &Response::new());
        assert_eq!(Some(second), transfer(&core, 2));
        assert_eq!(1, core.transfers.len());
        assert!(!core.pending_requests.contains(2));
    }

    /// A request whose answers the test counts.
    fn counted(core: &mut OnlineCore, id: u64, resource: Resource) -> (Request, Arc<AtomicU32>) {
        let answers = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&answers);
        let (request, responder) = Request::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        core.request(id, resource, responder);
        (request, answers)
    }

    fn tile(url: &str) -> Resource {
        Resource::new(Kind::Tile, url)
    }

    fn data() -> Response {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"tile".to_vec()));
        response
    }

    /// Requests for the same resource at once make one transfer, which
    /// takes one place in the active set, and each takes its response.
    #[test]
    fn requests_for_one_resource_share_its_transfer() {
        let (_source, mut core) = core(1);
        let url = url("/shared");
        let (_first, first) = counted(&mut core, 1, tile(&url));
        let (_second, second) = counted(&mut core, 2, tile(&url));
        pass_deadline(&mut core, 1);
        pass_deadline(&mut core, 2);
        let generation = transfer(&core, 1).expect("the transfer");
        assert_eq!(Some(generation), transfer(&core, 2));
        assert_eq!(1, core.transfers.len());
        assert!(!core.pending_requests.contains(2));

        core.transferred(generation, &data());
        assert_eq!((1, 1), (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)));
        assert!(core.transfers.is_empty());
    }

    /// A shared transfer goes on while a request waits for it, and stops
    /// with the last.
    #[test]
    fn a_shared_transfer_stops_with_its_last_request() {
        let (_source, mut core) = core(2);
        let url = url("/shared");
        let (_first, _) = counted(&mut core, 1, tile(&url));
        let (_second, second) = counted(&mut core, 2, tile(&url));
        pass_deadline(&mut core, 1);
        pass_deadline(&mut core, 2);
        let generation = transfer(&core, 1).expect("the transfer");

        core.cancel(1);
        assert!(core.transfers.contains_key(&generation));
        core.cancel(2);
        assert!(core.transfers.is_empty());
        core.transferred(generation, &data());
        assert_eq!(0, second.load(Ordering::SeqCst));
    }

    /// Requests that wait for room join the transfer they share when it
    /// starts, rather than starting another after it.
    #[test]
    fn waiting_requests_join_the_transfer_they_share_when_it_starts() {
        let (_source, mut core) = core(1);
        let (_other, _) = counted(&mut core, 1, tile(&url("/other")));
        let shared = url("/shared");
        let (_first, first) = counted(&mut core, 2, tile(&shared));
        let (_second, second) = counted(&mut core, 3, tile(&shared));
        for id in 1..=3 {
            pass_deadline(&mut core, id);
        }
        let other = transfer(&core, 1).expect("the first transfer");
        assert!(core.pending_requests.contains(2) && core.pending_requests.contains(3));

        core.transferred(other, &data());
        let generation = transfer(&core, 2).expect("the shared transfer");
        assert_eq!(Some(generation), transfer(&core, 3));
        assert!(!core.pending_requests.contains(3));
        core.transferred(generation, &data());
        assert_eq!((1, 1), (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)));
    }

    /// Requests that would send different validators, or ask for different
    /// bytes, don't share a transfer.
    #[test]
    fn requests_of_other_validators_or_bytes_transfer_apart() {
        let (_source, mut core) = core(4);
        let url = url("/apart");
        let mut validated = tile(&url);
        validated.prior_etag = Some("\"v1\"".to_owned());
        let mut ranged = tile(&url);
        ranged.data_range = Some((0, 99));
        let _requests =
            [counted(&mut core, 1, tile(&url)), counted(&mut core, 2, validated), counted(&mut core, 3, ranged)];
        for id in 1..=3 {
            pass_deadline(&mut core, id);
        }
        assert_eq!(3, core.transfers.len());
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

        let url = url("/transformed");
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
        let mut resource = Resource::new(Kind::Tile, url("/forever"));
        resource.minimum_update_interval = Duration::MAX;
        let (_request, responder) = Request::new(|_| {});
        core.request(1, resource, responder);
        pass_deadline(&mut core, 1);
        let generation = transfer(&core, 1).expect("the request's transfer");

        let mut response = Response::new();
        response.data = Some(Arc::new(b"tile".to_vec()));
        response.caching.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 60));
        core.transferred(generation, &response);
        assert_eq!(None, core.requests[&1].timer);
        assert_eq!(None, core.queue.next_deadline());
    }

    /// Replacing a request's deadline, or cancelling the request, cancels
    /// the deadline's task, which leaves the queue.
    #[test]
    fn a_replaced_or_cancelled_deadline_leaves_the_queue() {
        let (_source, mut core) = core(1);
        let _request = request(&mut core, 1, &url("/retry"));
        pass_deadline(&mut core, 1);
        let generation = transfer(&core, 1).expect("the request's transfer");
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Connection, "refused")));
        core.transferred(generation, &response);
        let retry = core.queue.next_deadline().expect("the retry, after its backoff");

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
