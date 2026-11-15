//! A network over the benchmarks' local files: while [`set_tile_latency`]
//! sets a delay, every scene's file source answers tile requests that late,
//! fresh for an hour as a tile server's are, and doesn't ask again for a
//! tile it answered until then, as the online file source doesn't. It
//! counts the tile requests that reach the files. Without a delay it passes
//! everything on at once.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use mln_actor::timer::Timer;
use mln_actor::{ActorRef, RunLoop, Thread};
use mln_storage::{FileSource, Kind, Request, Resource, Responder, Response};
use mln_util::chrono::Timestamp;
use mln_util::hash::HashMap;
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
    let timers = Thread::new("TileLatency", |self_ref, run_loop| Timers {
        self_ref,
        run_loop: run_loop.clone(),
        waiting: RefCell::default(),
    });
    Arc::new(Delayed { inner, timers })
}

/// Names a tile's wait.
static NEXT_WAIT: AtomicU64 = AtomicU64::new(0);

struct Delayed {
    inner: Arc<dyn FileSource>,
    /// The thread whose timers the tiles wait out their latency on.
    timers: Thread<Timers>,
}

/// The timers of the tiles that wait, on their thread.
struct Timers {
    self_ref: ActorRef<Timers>,
    run_loop: RunLoop,
    waiting: RefCell<HashMap<u64, Timer>>,
}

impl Timers {
    /// Runs `then` after `delay`, unless `stop(id)` comes first.
    fn start(&self, id: u64, delay: Duration, then: impl FnOnce() + 'static) {
        let timer = Timer::new(&self.run_loop);
        let (mut then, actor) = (Some(then), self.self_ref.clone());
        timer.start(delay, Duration::ZERO, move || {
            if let Some(then) = then.take() {
                then();
            }
            actor.invoke(move |timers: &Timers| timers.stop(id));
        });
        self.waiting.borrow_mut().insert(id, timer);
    }

    fn stop(&self, id: u64) {
        let timer = self.waiting.borrow_mut().remove(&id);
        drop(timer);
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
        // The tile waits out its latency on the timers' thread, then the
        // inner request; cancelling stops either.
        let request: Arc<Mutex<Option<Request>>> = Arc::default();
        let (inner, resource, slot, outer) =
            (Arc::clone(&self.inner), resource.clone(), Arc::clone(&request), responder.clone());
        let fire = move || {
            if outer.is_cancelled() {
                return;
            }
            let answer = outer.clone();
            let (fresh, inner_responder) = Request::direct(move |mut response: Response| {
                let expires = mln_util::chrono::now().secs_since_epoch() + FRESH_SECONDS;
                response.expires = Some(Timestamp::from_secs(expires));
                answer.send(response);
            });
            *slot.lock() = Some(fresh);
            inner.load(&resource, inner_responder);
        };
        let (id, delay, timers) =
            (NEXT_WAIT.fetch_add(1, Ordering::Relaxed), Duration::from_millis(latency), self.timers.actor());
        timers.invoke(move |timers: &Timers| timers.start(id, delay, fire));
        responder.on_cancel(move || {
            timers.invoke(move |timers: &Timers| timers.stop(id));
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
