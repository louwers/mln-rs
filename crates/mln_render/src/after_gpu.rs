//! Work that waits for the GPU to finish what the renderer gave it.

use std::sync::mpsc::{Receiver, SendError, Sender, channel};

/// The renderer's work after the GPU: what runs once the GPU finished the
/// work that the renderer gave it, or once the renderer's graphics objects
/// are lost, such as a host's freeing of what it drew with outside the
/// renderer. A layer keeps a clone ([`NewCx::after_gpu`](crate::layer::NewCx))
/// and pushes to it, typically when it drops. The renderer runs what was
/// pushed at its next update, after it waited for the GPU, or when it is
/// torn down, clears its data or loses its graphics objects.
#[derive(Clone)]
pub struct AfterGpu(Sender<Work>);

/// How the GPU's work that a piece of [`AfterGpu`] waited for ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuEnd {
    /// The GPU finished it: what it used can go.
    Finished,
    /// The renderer's graphics objects are lost, or may be: what it used
    /// is to be forgotten, not destroyed.
    Lost,
}

/// A piece of work after the GPU.
type Work = Box<dyn FnOnce(GpuEnd)>;

impl AfterGpu {
    /// Runs `work` once the GPU finished the renderer's work so far, or its
    /// graphics objects are lost, as its argument says: at once, as lost,
    /// once the renderer went.
    pub fn push(&self, work: impl FnOnce(GpuEnd) + 'static) {
        if let Err(SendError(work)) = self.0.send(Box::new(work)) {
            work(GpuEnd::Lost);
        }
    }
}

/// The work pushed to the [`AfterGpu`]s of a renderer and not run yet,
/// which runs as [`GpuEnd::Lost`] if the queue drops first.
pub(crate) struct AfterGpuQueue {
    sender: AfterGpu,
    receiver: Receiver<Work>,
}

impl AfterGpuQueue {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = channel();
        AfterGpuQueue { sender: AfterGpu(sender), receiver }
    }

    /// A handle that pushes to the queue.
    pub(crate) fn handle(&self) -> &AfterGpu {
        &self.sender
    }

    /// Takes the work that waits, in the order it was pushed.
    pub(crate) fn take(&self) -> Vec<Work> {
        self.receiver.try_iter().collect()
    }

    /// Runs the work that waits, in the order it was pushed, as `end` says.
    pub(crate) fn run(&self, end: GpuEnd) {
        for work in self.take() {
            work(end);
        }
    }
}

impl Drop for AfterGpuQueue {
    fn drop(&mut self) {
        self.run(GpuEnd::Lost);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    /// Pushes to `after` work that records its end as `name`'s in `ends`.
    fn record(after: &AfterGpu, ends: &Rc<Cell<Vec<(&'static str, GpuEnd)>>>, name: &'static str) {
        let ends = Rc::clone(ends);
        after.push(move |end| {
            let mut seen = ends.take();
            seen.push((name, end));
            ends.set(seen);
        });
    }

    #[test]
    fn work_runs_once_in_order_and_as_lost_once_the_queue_goes() {
        let ends = Rc::new(Cell::new(Vec::new()));
        let queue = AfterGpuQueue::new();
        let after = queue.handle().clone();
        record(&after, &ends, "first");
        record(&after, &ends, "second");
        queue.run(GpuEnd::Finished);
        queue.run(GpuEnd::Finished);
        assert_eq!(ends.take(), [("first", GpuEnd::Finished), ("second", GpuEnd::Finished)]);

        record(&after, &ends, "waiting");
        drop(queue);
        record(&after, &ends, "late");
        assert_eq!(ends.take(), [("waiting", GpuEnd::Lost), ("late", GpuEnd::Lost)]);
    }
}
