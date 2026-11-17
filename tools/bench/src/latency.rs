//! A network over the benchmarks' local files: while [`set_tile_latency`]
//! sets a delay, every scene's file source answers tile requests that late,
//! fresh for an hour as a tile server's are, and doesn't ask again for a
//! tile it answered until then, as the online file source doesn't. It
//! counts the tile requests that reach the files. Without a delay it passes
//! everything on at once.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use mln_storage::{FileSource, Kind, Request, Resource, Responder, Response};
use mln_util::chrono::Timestamp;
use mln_util::hash::HashMap;
use mln_util::thread::Joined;
use parking_lot::Mutex;

/// How long a tile stays fresh: a tile server's `Cache-Control: max-age`.
const FRESH_SECONDS: i64 = 3600;

static LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static TILE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

/// Delays every tile request by `latency` from now on, as a network would
/// answer it; zero passes them on at once.
pub(crate) fn set_tile_latency(latency: Duration) {
    LATENCY_MS.store(latency.as_millis() as u64, Ordering::SeqCst);
}

/// The tile requests that reached the files since the last call, and those
/// cancelled while they waited out the delay.
pub(crate) fn take_tile_requests() -> usize {
    TILE_REQUESTS.swap(0, Ordering::SeqCst)
}

/// `inner`, with the tile latency.
pub(crate) fn delayed(inner: Arc<dyn FileSource>) -> Arc<dyn FileSource> {
    let (waits, received) = mpsc::channel();
    let thread =
        mln_util::thread::spawn("TileLatency", move || wait_out(&received)).expect("the tile latency's thread");
    Arc::new(Delayed { inner, waits, next_wait: AtomicU64::new(0), _thread: Joined::new(thread) })
}

struct Delayed {
    inner: Arc<dyn FileSource>,
    /// The thread where the tiles wait out their latency.
    waits: mpsc::Sender<Wait>,
    /// Names the next tile's wait.
    next_wait: AtomicU64,
    _thread: Joined,
}

impl Drop for Delayed {
    fn drop(&mut self) {
        let _ = self.waits.send(Wait::End);
    }
}

/// What the latency's thread takes, in the order sent.
enum Wait {
    /// Runs `then` at `deadline`, unless `Stop(id)` comes first.
    Start {
        id: u64,
        deadline: Instant,
        then: Box<dyn FnOnce() + Send>,
    },
    Stop(u64),
    /// Ends the thread, dropping the waits that haven't ended.
    End,
}

/// The latency's thread: runs each wait's `then` when its deadline passes.
fn wait_out(received: &mpsc::Receiver<Wait>) {
    let mut deadlines: BinaryHeap<Reverse<(Instant, u64)>> = BinaryHeap::new();
    let mut waiting: HashMap<u64, Box<dyn FnOnce() + Send>> = HashMap::default();
    loop {
        let now = Instant::now();
        while let Some(&Reverse((deadline, id))) = deadlines.peek()
            && deadline <= now
        {
            deadlines.pop();
            // A stopped wait leaves its deadline, which ends nothing.
            if let Some(then) = waiting.remove(&id) {
                then();
            }
        }
        let wait = match deadlines.peek() {
            Some(&Reverse((deadline, _))) => {
                match received.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(wait) => wait,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            None => match received.recv() {
                Ok(wait) => wait,
                Err(_) => return,
            },
        };
        match wait {
            Wait::Start { id, deadline, then } => {
                waiting.insert(id, then);
                deadlines.push(Reverse((deadline, id)));
            }
            Wait::Stop(id) => drop(waiting.remove(&id)),
            Wait::End => return,
        }
    }
}

impl FileSource for Delayed {
    fn load(&self, resource: &Resource, responder: Responder) {
        if resource.kind != Kind::Tile {
            return self.inner.load(resource, responder);
        }
        let latency = LATENCY_MS.load(Ordering::SeqCst);
        if latency == 0 {
            TILE_REQUESTS.fetch_add(1, Ordering::SeqCst);
            return self.inner.load(resource, responder);
        }
        // A tile that is still fresh, which the online file source holds
        // until it expires: within a case, never answered.
        let now = mln_util::chrono::now();
        if resource.prior_expires.is_some_and(|expires| expires > now) {
            return;
        }
        TILE_REQUESTS.fetch_add(1, Ordering::SeqCst);
        // The tile waits out its latency on the latency's thread, then the
        // inner request; cancelling stops either.
        let request: Arc<Mutex<Option<Request>>> = Arc::default();
        let (inner, resource, slot, outer) =
            (Arc::clone(&self.inner), resource.clone(), Arc::clone(&request), responder.clone());
        let fire = move || {
            if outer.is_cancelled() {
                return;
            }
            let answer = outer.clone();
            let (fresh, inner_responder) = Request::new(move |mut response: Response| {
                let expires = mln_util::chrono::now().secs_since_epoch() + FRESH_SECONDS;
                response.expires = Some(Timestamp::from_secs(expires));
                answer.send(response);
            });
            *slot.lock() = Some(fresh);
            inner.load(&resource, inner_responder);
        };
        let id = self.next_wait.fetch_add(1, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_millis(latency);
        let _ = self.waits.send(Wait::Start { id, deadline, then: Box::new(fire) });
        let waits = self.waits.clone();
        responder.on_cancel(move || {
            let _ = waits.send(Wait::Stop(id));
            drop(request.lock().take());
        });
    }

    fn supports_cache_only(&self) -> bool {
        self.inner.supports_cache_only()
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.inner.can_request(resource)
    }
}
