// Port-only: a host without threads installs a `CooperativeScheduler` at
// startup (`install_cooperative`), which runs the objects of `Thread`s on the
// host's thread (cooperative_scheduler.rs).

use std::sync::{Arc, OnceLock};

use crate::cooperative_scheduler::CooperativeScheduler;

static COOPERATIVE: OnceLock<Arc<CooperativeScheduler>> = OnceLock::new();

/// Port-only: makes `scheduler` run the objects of `util::Thread`s, which
/// stay on the creating thread's `RunLoop` (see `cooperative_scheduler.rs`).
/// For a host with one thread (the browser), which runs them with
/// `CooperativeScheduler::run_loops`. It must come before anything starts a
/// `Thread`, and only once.
pub fn install_cooperative(scheduler: Arc<CooperativeScheduler>) {
    if COOPERATIVE.set(scheduler).is_err() {
        panic!("a cooperative scheduler is already installed");
    }
}

/// Port-only: the installed cooperative scheduler, after installing a new
/// one if none is: `install_cooperative` for hosts that may each try to
/// install it, such as libraries that share a wasm instance. The first
/// install must still come before anything starts a `Thread`.
pub fn install_cooperative_once() -> &'static Arc<CooperativeScheduler> {
    COOPERATIVE.get_or_init(|| Arc::new(CooperativeScheduler::new()))
}

/// Port-only: the scheduler `install_cooperative` installed, if any.
pub fn cooperative() -> Option<&'static Arc<CooperativeScheduler>> {
    COOPERATIVE.get()
}
