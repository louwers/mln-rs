//! A network over the benchmarks' local files: while [`set_tile_latency`]
//! sets a delay, every scene's file source answers tile requests that late,
//! fresh for an hour as a tile server's are, and doesn't ask again for a
//! tile it answered until then, as the online file source doesn't. It
//! counts the tile requests that reach the files. Without a delay it passes
//! everything on at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use mln_actor::LocalObject;
use mln_actor::timer::Timer;
use mln_storage::{FileSource, Kind, Request, Resource, Responder, Response};
use mln_util::chrono::Timestamp;
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
    Arc::new(Delayed { inner })
}

struct Delayed {
    inner: Arc<dyn FileSource>,
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
        // The tile waits out its latency on the requesting thread's timer,
        // then the inner request; cancelling stops either.
        let request: Arc<Mutex<Option<Request>>> = Arc::default();
        let timer = Timer::new();
        let (inner, resource, slot, outer) =
            (Arc::clone(&self.inner), resource.clone(), Arc::clone(&request), responder.clone());
        timer.start(Duration::from_millis(latency), Duration::ZERO, move || {
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
        });
        let timer = LocalObject::new(timer);
        responder.on_cancel(move || {
            drop(timer.take());
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
