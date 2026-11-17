//! A file source whose answers take a fixed number of frames, as a
//! network's would, on a schedule that only frames advance.

use std::collections::VecDeque;
use std::sync::Arc;

use mln_storage::{FileSource, Kind, Resource, Responder};
use parking_lot::Mutex;

/// Another file source, behind a latency counted in frames.
///
/// While its delay is zero, each request goes straight on. Otherwise a
/// request made before frame `n`'s [`tick`](Self::tick) goes on at the
/// tick of frame `n + delay`: a scenario that ticks before each frame and
/// then waits for the workers sees each resource arrive exactly `delay`
/// frames after the map asked for it, however fast the machine reads it.
/// A request the map cancels in the meantime never goes on.
pub(crate) struct Latency {
    inner: Arc<dyn FileSource>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    delay: u64,
    /// The frames ticked so far.
    frame: u64,
    /// The requests held back, with the tick that sends each on, in the
    /// order they came.
    held: VecDeque<(u64, Resource, Responder)>,
    tile_requests: u64,
}

impl Latency {
    /// `inner`, with no delay until [`set_delay`](Self::set_delay).
    pub(crate) fn new(inner: Arc<dyn FileSource>) -> Arc<Latency> {
        Arc::new(Latency { inner, state: Mutex::default() })
    }

    /// Holds each request from now on for `frames` ticks. Zero sends the
    /// held requests on, and the ones after them straight on.
    pub(crate) fn set_delay(&self, frames: u64) {
        let mut state = self.state.lock();
        state.delay = frames;
        if frames == 0 {
            let held = std::mem::take(&mut state.held);
            drop(state);
            for (_, resource, responder) in held {
                self.send_on(&resource, responder);
            }
        }
    }

    /// Counts a frame, and sends on the requests whose delay ends at it.
    pub(crate) fn tick(&self) {
        let mut state = self.state.lock();
        state.frame += 1;
        let frame = state.frame;
        let due = state.held.iter().take_while(|(at, ..)| *at <= frame).count();
        let due: Vec<_> = state.held.drain(..due).collect();
        drop(state);
        for (_, resource, responder) in due {
            self.send_on(&resource, responder);
        }
    }

    /// The tile requests made so far, held or not.
    pub(crate) fn tile_requests(&self) -> u64 {
        self.state.lock().tile_requests
    }

    fn send_on(&self, resource: &Resource, responder: Responder) {
        if !responder.is_cancelled() {
            self.inner.load(resource, responder);
        }
    }
}

impl FileSource for Latency {
    fn load(&self, resource: &Resource, responder: Responder) {
        let mut state = self.state.lock();
        if resource.kind == Kind::Tile {
            state.tile_requests += 1;
        }
        if state.delay == 0 {
            drop(state);
            self.inner.load(resource, responder);
            return;
        }
        let at = state.frame + state.delay;
        state.held.push_back((at, resource.clone(), responder));
    }

    fn can_request(&self, resource: &Resource) -> bool {
        self.inner.can_request(resource)
    }

    fn supports_cache_only(&self) -> bool {
        self.inner.supports_cache_only()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mln_storage::{Request, Response};

    use super::*;

    /// Answers every request at once, on the calling thread.
    struct Answers;

    impl FileSource for Answers {
        fn load(&self, _resource: &Resource, responder: Responder) {
            responder.send(Response::new());
        }

        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }

    fn request(latency: &Latency, answered: &Arc<AtomicUsize>) -> Request {
        let answered = Arc::clone(answered);
        let (request, responder) = Request::new(move |_| {
            answered.fetch_add(1, Ordering::SeqCst);
        });
        latency.load(&Resource::style("asset://style.json"), responder);
        request
    }

    #[test]
    fn answers_a_fixed_number_of_ticks_after_the_request() {
        let latency = Latency::new(Arc::new(Answers));
        let answered = Arc::new(AtomicUsize::new(0));
        let _now = request(&latency, &answered);
        assert_eq!(answered.load(Ordering::SeqCst), 1, "no delay: straight on");

        latency.set_delay(2);
        let _first = request(&latency, &answered);
        latency.tick();
        let _second = request(&latency, &answered);
        let cancelled = request(&latency, &answered);
        drop(cancelled);
        assert_eq!(answered.load(Ordering::SeqCst), 1);
        latency.tick();
        assert_eq!(answered.load(Ordering::SeqCst), 2, "the first, two ticks after it");
        latency.tick();
        assert_eq!(answered.load(Ordering::SeqCst), 3, "the second; the cancelled one never");

        let _held = request(&latency, &answered);
        latency.set_delay(0);
        assert_eq!(answered.load(Ordering::SeqCst), 4, "no delay sends the held requests on");
    }
}
