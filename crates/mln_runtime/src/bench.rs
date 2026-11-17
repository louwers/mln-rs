//! Entry points to the runtime's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.

use std::sync::Arc;

use mln_actor::RunLoop;

use crate::queue::Queue;

/// The queue that carries commands to a runtime's thread, and the loop of
/// that thread, which a posted command wakes. Both run on the thread that
/// makes them, so a command's whole trip runs there.
pub struct CommandQueue<S> {
    queue: Queue<S>,
    run_loop: RunLoop,
}

impl<S: 'static> CommandQueue<S> {
    /// A queue whose commands wake its loop, as a runtime's thread has them
    /// do while it runs the queue.
    #[must_use]
    pub fn new() -> Self {
        let queue = Queue::default();
        let run_loop = RunLoop::new();
        let handle = run_loop.handle();
        queue.set_waker(Some(Arc::new(move || handle.wake())));
        CommandQueue { queue, run_loop }
    }

    /// Posts `command`, which runs after those posted before it.
    pub fn post(&self, command: impl FnOnce(&mut S) + Send + 'static) {
        self.queue.post(command);
    }

    /// One turn of the thread: its loop's work, then the commands waiting
    /// and those they post, with `state`. Returns whether a command ran.
    pub fn run(&self, state: &mut S) -> bool {
        self.run_loop.run_once();
        self.queue.run_once(state)
    }
}

impl<S: 'static> Default for CommandQueue<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_run_in_order_on_the_next_turn() {
        let queue = CommandQueue::new();
        for value in 1..=3 {
            queue.post(move |state: &mut Vec<u32>| state.push(value));
        }
        let mut state = Vec::new();
        assert!(queue.run(&mut state));
        assert_eq!(state, [1, 2, 3]);
        assert!(!queue.run(&mut state));
    }
}
