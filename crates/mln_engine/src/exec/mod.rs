//! Where the engine's work runs: the thread pool and its task groups
//! ([`pool`]), the run loop's queue ([`queue`]), the queue that wakes a
//! host's loop ([`event_queue`]), named threads ([`thread`]) and what a
//! panic on one of them reports ([`mod@panic`]).

pub mod event_queue;
pub mod panic;
pub mod pool;
pub mod queue;
pub mod thread;
