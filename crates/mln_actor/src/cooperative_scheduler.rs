// Port-only: the loops of `util::Thread`s for a host that has one thread
// (the browser: a wasm32-unknown-unknown instance per page or Web Worker,
// each with its own memory). C++ has no counterpart; it always has threads.
//
// A host installs one at startup with `scheduler::install_cooperative`,
// before any file source exists. From then on a `util::Thread` keeps its
// object on a RunLoop that the scheduler runs on the host's thread
// (thread.rs), when the host calls `run_loops`. Nothing changes for a host
// that doesn't install one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

use crate::local_object::LocalObject;
use crate::run_loop::RunLoop;

/// Runs the loops of `Thread`s on the host's own thread.
pub struct CooperativeScheduler {
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// The loops of the `Thread`s whose objects live on the host's thread,
    /// which `run_loops` runs, by key.
    loops: Mutex<Vec<(u64, LocalObject<RunLoop>)>>,
    next_loop: AtomicU64,
}

impl Default for CooperativeScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl CooperativeScheduler {
    #[must_use]
    pub fn new() -> Self {
        CooperativeScheduler { waker: Mutex::new(None), loops: Mutex::new(Vec::new()), next_loop: AtomicU64::new(0) }
    }

    /// Calls `waker`, from the waking thread, whenever a hosted loop gets
    /// work, so that the host knows to run it (as `RunLoop::set_waker`).
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.waker.lock() = waker;
    }

    fn wake(&self) {
        let waker = self.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Runs `run_loop`, which belongs to the host's thread, in each
    /// [`run_loops`](Self::run_loops) until [`unhost`](Self::unhost) with the
    /// returned key. Its work wakes the host.
    pub(crate) fn host(self: &Arc<Self>, run_loop: &RunLoop) -> u64 {
        let scheduler = Arc::downgrade(self);
        run_loop.set_waker(Some(Arc::new(move || {
            if let Some(scheduler) = Weak::upgrade(&scheduler) {
                scheduler.wake();
            }
        })));
        let key = self.next_loop.fetch_add(1, Ordering::Relaxed);
        self.loops.lock().push((key, LocalObject::new(run_loop.clone())));
        key
    }

    pub(crate) fn unhost(&self, key: u64) {
        let removed: Vec<_> = {
            let mut loops = self.loops.lock();
            let (removed, kept) = loops.drain(..).partition(|(k, _)| *k == key);
            *loops = kept;
            removed
        };
        drop(removed);
    }

    /// Runs each hosted loop once, and returns whether any has work left.
    pub fn run_loops(&self) -> bool {
        let loops: Vec<_> = self.loops.lock().iter().filter_map(|(_, run_loop)| run_loop.get()).collect();
        for run_loop in &loops {
            run_loop.run_once();
        }
        loops.iter().any(|run_loop| run_loop.has_pending())
    }
}
