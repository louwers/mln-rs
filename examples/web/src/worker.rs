//! The tile workers of one Web Worker: its own wasm instance, with its own
//! memory, running a `TileWorkerHost` (vector and GeoJSON tiles' layouts,
//! raster tiles' images and raster-dem tiles' DEMs). web/worker.ts passes it
//! the map's requests and posts its replies back.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use mln_actor::TaggedScheduler;
use mln_actor::cooperative_scheduler::CooperativeScheduler;
use mln_renderer::tile::worker_codec::{Encoded, TileWorkerHost};
use mln_util::identity::SimpleIdentity;
use wasm_bindgen::prelude::*;

use crate::{cooperative_scheduler, deadline};

thread_local! {
    /// The Web Worker's `wake` (see `TileWorker::new`).
    static WAKE: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
    /// Whether `TileWorker::run` is running, which takes the replies it
    /// makes itself.
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// Tells the Web Worker to `run`, for a reply made outside it: a raster or
/// raster-dem tile's, once the browser has decoded its image.
fn wake_host() {
    if !RUNNING.get() {
        WAKE.with(|wake| {
            if let Some(wake) = wake.borrow().as_ref() {
                let _ = wake.call0(&JsValue::NULL);
            }
        });
    }
}

/// The worker side: the tile workers of one Web Worker's wasm instance.
#[wasm_bindgen]
pub struct TileWorker {
    /// Where the workers run, when `run` runs them.
    scheduler: Arc<CooperativeScheduler>,
    host: TileWorkerHost,
    /// The replies since the last `run`.
    replies: Arc<Mutex<Vec<Encoded>>>,
}

#[wasm_bindgen]
impl TileWorker {
    /// The instance's tile workers, on its cooperative scheduler. `wake()`
    /// asks the Web Worker to `run` soon: when a reply comes outside `run`.
    #[wasm_bindgen(constructor)]
    pub fn new(wake: js_sys::Function) -> TileWorker {
        WAKE.with(|w| *w.borrow_mut() = Some(wake));
        let scheduler = cooperative_scheduler();
        let replies = Arc::new(Mutex::new(Vec::new()));
        let workers = TaggedScheduler::new(scheduler.clone(), SimpleIdentity::new());
        let sink = replies.clone();
        let host = TileWorkerHost::new(workers, move |reply| {
            sink.lock().unwrap().push(reply);
            wake_host();
        });
        TileWorker { scheduler, host, replies }
    }

    /// Takes a request from the map. Its work runs in `run`.
    pub fn handle(&self, request: &[u8]) {
        self.host.handle(request);
    }

    /// Runs the workers' waiting tasks until none are left or `budget_ms`
    /// has passed (checked after each task), and returns the replies for
    /// `WebMap::receive`: each an array of `Uint8Array`s, its bytes and then
    /// its payloads (pixels, vertices, indices), which worker.ts transfers.
    pub fn run(&self, budget_ms: f64) -> js_sys::Array {
        RUNNING.set(true);
        self.scheduler.run_until(Some(deadline(budget_ms)));
        RUNNING.set(false);
        let replies = std::mem::take(&mut *self.replies.lock().unwrap());
        replies
            .into_iter()
            .map(|reply| {
                std::iter::once(reply.bytes)
                    .chain(reply.payloads.into_iter().map(|payload| payload.into_vec()))
                    .map(|part| js_sys::Uint8Array::from(&part[..]))
                    .collect::<js_sys::Array>()
            })
            .collect()
    }

    /// Whether tasks are waiting for `run`.
    pub fn has_tasks(&self) -> bool {
        self.scheduler.has_tasks()
    }
}
