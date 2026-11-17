// The requests run in the online core (`Core`), which owns their state and
// takes messages from a channel: an `OnlineRequest` per caller request,
// started by a deadline (the first request at once, then revalidation when
// the response expires, or retries with backoff after errors), queued while
// `maximum_concurrent_requests` transfers are active (regular priority
// before low priority, and within a priority by the resource's
// `RequestOrder`), and performed by the HTTPFileSource. `requests` is keyed
// by id, so it iterates in id order.
//
// The resource transform's answers and the transfers' responses come back
// to the core as messages, from whichever thread sends them, numbered with
// a generation of their request: an answer whose generation is no longer
// the one the request waits for is dropped, so a stale one can't end a
// transfer twice. A deadline is numbered the same way, so restarting or
// stopping a request's deadline only takes a new number.
//
// The core runs on a thread of its own, except in the browser, where the
// host's pump runs it (`OnlineCore::run_due`).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mln_util::chrono::{Timestamp, now};
use mln_util::constants::{
    ASSET_PROTOCOL, CLOCK_SKEW_RETRY_TIMEOUT, DEFAULT_MAXIMUM_CONCURRENT_REQUESTS, FILE_PROTOCOL,
};
use mln_util::logging::{Event, Log};
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

/// What reaches the core, in the order it was sent.
enum Message {
    Request {
        id: u64,
        resource: Resource,
        responder: Responder,
    },
    Cancel(u64),
    /// The resource transform's answer for request `id`.
    Transformed {
        id: u64,
        generation: u64,
        result: TransformResult,
    },
    /// A transfer's response for request `id`.
    TransferDone {
        id: u64,
        generation: u64,
        response: Response,
    },
    /// The network is reachable again.
    Reachable,
    SetMaximumConcurrentRequests(u32),
    SetOnline(bool),
    SetResourceTransform(Option<ResourceTransform>),
    SetHttpHeaderTransform(HttpHeaderTransform),
    #[cfg(test)]
    Run(Box<dyn FnOnce(&mut Core) + Send>),
    /// Ends the core, after the messages sent before.
    Stop,
}

/// The way into the core: its channel, and what wakes whoever runs it.
#[derive(Clone)]
struct Inbox {
    sender: mpsc::Sender<Message>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Inbox {
    /// Sends `message`, which the core drops if it has ended.
    fn send(&self, message: Message) {
        let _ = self.try_send(message);
    }

    /// Sends `message`, or gives it back when the core has ended.
    fn try_send(&self, message: Message) -> Result<(), Box<Message>> {
        self.sender.send(message).map_err(|mpsc::SendError(message)| Box::new(message))?;
        if let Some(wake) = &self.wake {
            wake();
        }
        Ok(())
    }
}

struct OnlineRequest {
    resource: Resource,
    responder: Responder,
    /// The transfer in flight, and its generation.
    transfer: Option<(u64, Request)>,
    /// The generation of the resource transform's answer it waits for.
    transforming: Option<u64>,
    /// The generation of the deadline that starts it again, while one runs.
    timer: Option<u64>,

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

/// The online file source's state, which the messages of its channel
/// change.
struct Core {
    receiver: mpsc::Receiver<Message>,
    /// For the transform's answers and the transfers' responses.
    inbox: Inbox,
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
    /// The requests' deadlines: when each starts again, with the deadline's
    /// generation, which also orders deadlines that are due at once.
    deadlines: BinaryHeap<Reverse<(Instant, u64, u64)>>,
    /// The last generation handed out.
    generation: u64,

    online: bool,
    maximum_concurrent_requests: u32,
    http_file_source: Arc<HTTPFileSource>,
    resource_transform: Option<ResourceTransform>,
    network: Arc<NetworkStatus>,
    /// A [`Message::Reachable`] is on its way.
    reachable: Arc<AtomicBool>,
    /// Replaces the backoff after a connection error, so that a test can
    /// tell the retries it starts apart from the backoff's.
    connection_backoff: Option<Duration>,
}

impl Core {
    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Handles `message`. Returns `false` for [`Message::Stop`].
    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Request { id, resource, responder } => {
                // A request cancelled before this ran had its cancel run
                // first, which found nothing to cancel.
                if !responder.is_cancelled() {
                    self.request(id, resource, responder);
                }
            }
            Message::Cancel(id) => self.cancel(id),
            Message::Transformed { id, generation, result } => {
                if self.requests.get(&id).is_some_and(|req| req.transforming == Some(generation)) {
                    self.set_transform_result(id, result);
                }
            }
            Message::TransferDone { id, generation, response } => {
                let in_flight = self.requests.get(&id).and_then(|req| req.transfer.as_ref()).map(|(g, _)| *g);
                if in_flight == Some(generation) {
                    self.transfer_done(id, response);
                }
            }
            Message::Reachable => {
                self.reachable.store(false, Ordering::SeqCst);
                self.network_is_reachable_again();
            }
            Message::SetMaximumConcurrentRequests(maximum) => self.maximum_concurrent_requests = maximum,
            Message::SetOnline(online) => {
                self.online = online;
                if online {
                    self.network_is_reachable_again();
                }
            }
            Message::SetResourceTransform(transform) => self.resource_transform = transform,
            Message::SetHttpHeaderTransform(transform) => self.http_file_source.set_http_header_transform(transform),
            #[cfg(test)]
            Message::Run(run) => run(self),
            Message::Stop => return false,
        }
        true
    }

    /// Handles the messages that have arrived, then starts the requests
    /// whose deadlines are due. Returns `false` once the core has ended.
    fn run_due(&mut self) -> bool {
        loop {
            match self.receiver.try_recv() {
                Ok(message) => {
                    if !self.handle(message) {
                        return false;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
        self.fire_due();
        true
    }

    /// Handles messages and deadlines until the core ends.
    #[cfg(not(target_family = "wasm"))]
    fn run(mut self) {
        loop {
            self.fire_due();
            let message = match self.next_deadline() {
                Some(deadline) => {
                    match self.receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(message) => message,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match self.receiver.recv() {
                    Ok(message) => message,
                    Err(mpsc::RecvError) => return,
                },
            };
            if !self.handle(message) || !self.run_due() {
                return;
            }
        }
    }

    /// The earliest deadline that would start a request.
    #[cfg(not(target_family = "wasm"))]
    fn next_deadline(&mut self) -> Option<Instant> {
        while let Some(Reverse((due, generation, id))) = self.deadlines.peek().copied() {
            if self.requests.get(&id).is_some_and(|req| req.timer == Some(generation)) {
                return Some(due);
            }
            self.deadlines.pop();
        }
        None
    }

    /// Starts the requests whose deadlines were due when this was called,
    /// earliest first, and of those due at once, in the order they were set.
    fn fire_due(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        while let Some(&Reverse((at, generation, id))) = self.deadlines.peek() {
            if at > now {
                break;
            }
            self.deadlines.pop();
            due.push((generation, id));
        }
        for (generation, id) in due {
            let Some(req) = self.requests.get_mut(&id) else { continue };
            if req.timer != Some(generation) {
                continue;
            }
            req.timer = None;
            if req.transform_failed {
                self.transform(id);
            } else {
                self.activate_or_queue_request(id);
            }
        }
    }

    fn request(&mut self, id: u64, resource: Resource, responder: Responder) {
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
        let inbox = self.inbox.clone();
        transform(
            kind,
            &url,
            Box::new(move |result: TransformResult| inbox.send(Message::Transformed { id, generation, result })),
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
        let req = self.requests.remove(&id);
        drop(req);
        // Deadlines of cancelled requests wait in the heap until they are
        // due; drop them once they outnumber the live ones.
        if self.deadlines.len() > 2 * self.requests.len() + 64 {
            let requests = &self.requests;
            self.deadlines.retain(|Reverse((_, generation, id))| {
                requests.get(id).is_some_and(|req| req.timer == Some(*generation))
            });
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
            let inbox = self.inbox.clone();
            let Some(req) = self.requests.get_mut(&id) else { return };
            let (request, responder) = Request::new(move |response: Response| {
                inbox.send(Message::TransferDone { id, generation, response });
            });
            req.transfer = Some((generation, request));
            self.http_file_source.load(&req.resource, responder);
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
        let in_progress = self.pending_requests.contains(id) || self.active_requests.contains(&id);
        let generation = self.next_generation();
        let offline = self.network.get() == Status::Offline;
        let Some(req) = self.requests.get_mut(&id) else { return };
        if req.transforming.is_some() || in_progress {
            // There's already a request in progress; don't start another one.
            return;
        }

        let Some(timeout) = timeout else {
            return;
        };

        // Emulate a Connection error when the Offline mode is forced, and
        // don't retry: the request will get re-triggered when the
        // NetworkStatus is set back to Online.
        if offline {
            req.failed_request_reason = Reason::Connection;
            req.failed_requests = 1;
            req.timer = None;
            return;
        }

        // A deadline past what `Instant` can hold never comes.
        req.timer = None;
        if let Some(due) = Instant::now().checked_add(timeout) {
            req.timer = Some(generation);
            self.deadlines.push(Reverse((due, generation, id)));
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
    inbox: Inbox,
    /// The core's, for the fetch preloads.
    #[cfg(target_family = "wasm")]
    http_file_source: Arc<HTTPFileSource>,
    _reachability: Subscription,
    #[cfg(not(target_family = "wasm"))]
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The online core of an [`OnlineFileSource`] in the browser, which the
/// host runs on the source's thread.
#[cfg(target_family = "wasm")]
pub struct OnlineCore(Core);

#[cfg(target_family = "wasm")]
impl OnlineCore {
    /// Handles what the source sent since the last call, and starts the
    /// requests whose deadlines are due. Returns `false` once the source
    /// has dropped, after which the core does nothing.
    pub fn run_due(&mut self) -> bool {
        self.0.run_due()
    }
}

impl OnlineFileSource {
    /// A source whose requests run on a thread of its own and follow
    /// `network`.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn new(network: Arc<NetworkStatus>) -> Self {
        let (mut source, core) = Self::with_core(network, None);
        match mln_util::thread::spawn("OnlineFileSource", move || core.run()) {
            Ok(thread) => source.thread = Some(thread),
            Err(error) => Log::error(Event::General, format!("The online file source's thread didn't start: {error}")),
        }
        source
    }

    /// A source whose requests follow `network` and run in the returned
    /// core, which the caller runs on this thread: `wake` runs, on any
    /// thread, whenever the core has work for
    /// [`run_due`](OnlineCore::run_due).
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn local(network: Arc<NetworkStatus>, wake: Arc<dyn Fn() + Send + Sync>) -> (Self, OnlineCore) {
        let (source, core) = Self::with_core(network, Some(wake));
        (source, OnlineCore(core))
    }

    fn with_core(network: Arc<NetworkStatus>, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> (Self, Core) {
        let (sender, receiver) = mpsc::channel();
        let inbox = Inbox { sender, wake };
        let reachable = Arc::new(AtomicBool::new(false));
        let reachability = {
            let (inbox, reachable) = (inbox.clone(), Arc::clone(&reachable));
            network.subscribe(move || {
                if !reachable.swap(true, Ordering::SeqCst) {
                    inbox.send(Message::Reachable);
                }
            })
        };
        let http_file_source = Arc::new(HTTPFileSource::new());
        let core = Core {
            receiver,
            inbox: inbox.clone(),
            requests: VecMap::new(),
            pending_requests: PendingRequests::default(),
            active_requests: VecSet::new(),
            deadlines: BinaryHeap::new(),
            generation: 0,
            online: true,
            maximum_concurrent_requests: DEFAULT_MAXIMUM_CONCURRENT_REQUESTS,
            http_file_source: Arc::clone(&http_file_source),
            resource_transform: None,
            network,
            reachable,
            connection_backoff: None,
        };
        let source = OnlineFileSource {
            maximum_concurrent_requests: AtomicU32::new(DEFAULT_MAXIMUM_CONCURRENT_REQUESTS),
            next_request_id: AtomicU64::new(1),
            inbox,
            #[cfg(target_family = "wasm")]
            http_file_source,
            _reachability: reachability,
            #[cfg(not(target_family = "wasm"))]
            thread: None,
        };
        (source, core)
    }

    /// The most transfers in flight at once; the requests beyond wait.
    #[must_use]
    pub fn maximum_concurrent_requests(&self) -> u32 {
        self.maximum_concurrent_requests.load(Ordering::Relaxed)
    }

    /// Sets the most transfers in flight at once.
    pub fn set_maximum_concurrent_requests(&self, maximum: u32) {
        self.maximum_concurrent_requests.store(maximum, Ordering::Relaxed);
        self.inbox.send(Message::SetMaximumConcurrentRequests(maximum));
    }

    /// Takes the source offline or back online. Offline, every transfer
    /// fails with a connection error; back online, the requests that failed
    /// that way retry at once.
    pub fn set_online(&self, online: bool) {
        self.inbox.send(Message::SetOnline(online));
    }

    /// Rewrites the URLs of requests before they load, or stops rewriting
    /// them for `None`.
    pub fn set_resource_transform(&self, transform: Option<ResourceTransform>) {
        self.inbox.send(Message::SetResourceTransform(transform));
    }

    /// Sets where the browser's fetch looks for a response that the host
    /// fetched ahead before it fetches a whole resource: `take(url)` returns
    /// a promise of the `Response` fetched for exactly `url`, at most once
    /// for each, or undefined. A promise that rejects is fetched again. Call
    /// it on the thread that runs the source's core.
    #[cfg(target_family = "wasm")]
    pub fn set_fetch_preloads(&self, take: Option<js_sys::Function>) {
        self.http_file_source.set_fetch_preloads(take);
    }

    /// Sets the headers to add to each transfer.
    pub fn set_http_header_transform(&self, transform: HttpHeaderTransform) {
        self.inbox.send(Message::SetHttpHeaderTransform(transform));
    }

    /// Holds the core until the guard drops (`crate::tests::pause`).
    #[cfg(test)]
    pub(crate) fn pause(&self) -> crate::tests::Paused {
        crate::tests::pause(|hold| self.inbox.send(Message::Run(Box::new(move |_| hold()))))
    }

    /// Waits `backoff` to retry a request that failed with a connection
    /// error, in place of the exponential backoff.
    #[cfg(test)]
    pub(crate) fn set_connection_backoff(&self, backoff: Duration) {
        self.inbox.send(Message::Run(Box::new(move |core| core.connection_backoff = Some(backoff))));
    }
}

impl Drop for OnlineFileSource {
    fn drop(&mut self) {
        self.inbox.send(Message::Stop);
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
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let inbox = self.inbox.clone();
        responder.on_cancel(move || inbox.send(Message::Cancel(id)));
        let request = Message::Request { id, resource: resource.clone(), responder };
        if let Err(request) = self.inbox.try_send(request)
            && let Message::Request { responder, .. } = *request
        {
            let mut response = Response::new();
            response.error =
                Some(Box::new(ResourceError::new(Reason::Other, "The online file source's thread isn't running.")));
            responder.send(response);
        }
    }

    fn can_request(&self, resource: &Resource) -> bool {
        resource.has_loading_method(crate::resource::LoadingMethod::Network)
            && !resource.url.starts_with(ASSET_PROTOCOL)
            && !resource.url.starts_with(FILE_PROTOCOL)
    }
}

#[cfg(test)]
mod tests {
    use mln_test_server::closed_port_url;
    use parking_lot::Mutex;

    use super::*;
    use crate::resource::Kind;
    use crate::resource_transform::FinishedCallback;

    /// A core that no thread runs, which the test hands its messages, and
    /// the source whose drop ends it.
    fn core(maximum: u32) -> (OnlineFileSource, Core) {
        let (source, mut core) = OnlineFileSource::with_core(Arc::new(NetworkStatus::new()), None);
        core.maximum_concurrent_requests = maximum;
        (source, core)
    }

    fn request(core: &mut Core, id: u64, url: &str) -> Request {
        let (request, responder) = Request::new(|_| {});
        core.handle(Message::Request { id, resource: Resource::new(Kind::Unknown, url), responder });
        request
    }

    /// The generation of request `id`'s transfer in flight.
    fn transfer(core: &Core, id: u64) -> Option<u64> {
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
        core.fire_due();
        let first = transfer(&core, 1).expect("the first request's transfer");
        assert!(core.pending_requests.contains(2));

        let done = |generation| Message::TransferDone { id: 1, generation, response: Response::new() };
        core.handle(done(first + 1000));
        assert_eq!(Some(first), transfer(&core, 1));
        assert!(core.pending_requests.contains(2));

        core.handle(done(first));
        assert_eq!(None, transfer(&core, 1));
        let second = transfer(&core, 2).expect("the second request's transfer");
        assert_eq!(1, core.active_requests.len());

        core.handle(done(first));
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

        core.handle(Message::Transformed { id: 1, generation: generation - 1, result: Ok("stale".to_owned()) });
        assert_eq!(Some(generation), core.requests[&1].transforming);
        assert_eq!(original, core.requests[&1].resource.url);

        let url = closed_port_url("/transformed");
        let finished = answers.lock().pop().expect("the transform's callback");
        finished(Ok(url.clone()));
        let answer = core.receiver.try_recv().expect("the transform's answer");
        core.handle(answer);
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
        core.handle(Message::Request { id: 1, resource, responder });
        core.fire_due();
        let generation = transfer(&core, 1).expect("the request's transfer");

        let mut response = Response::new();
        response.data = Some(Arc::new(b"tile".to_vec()));
        response.expires = Some(Timestamp::from_secs(now().secs_since_epoch() + 60));
        core.handle(Message::TransferDone { id: 1, generation, response });
        assert_eq!(None, core.requests[&1].timer);
        assert!(core.deadlines.is_empty());
    }

    /// The last owner of a source may drop it on the source's own thread.
    #[test]
    fn a_source_dropped_on_its_own_thread_ends() {
        let source = OnlineFileSource::new(Arc::new(NetworkStatus::new()));
        let inbox = source.inbox.clone();
        let (dropped, done) = mpsc::channel();
        inbox.send(Message::Run(Box::new(move |_| {
            drop(source);
            dropped.send(()).unwrap();
        })));
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
