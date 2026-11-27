//! Work that waits for the GPU to finish what the renderer gave it.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use mln_gfx::gpu::Gpu;

use crate::log_target;

/// Where a layer queues work that waits for the GPU: what runs once the GPU
/// finished the work that the renderer gave it so far, or once the
/// renderer's graphics objects are lost, such as the freeing of what a
/// layer made with the commands a frame lent it ([`Lent`](super::Lent)).
///
/// A layer keeps a clone and pushes to it, typically as it drops, which is
/// when it leaves the renderer. The renderer runs what was pushed at the end
/// of its next update, after waiting for the GPU, or as it clears its data,
/// is torn down or loses its graphics objects.
#[derive(Clone)]
pub struct AfterGpu(Weak<Cell<Vec<Work>>>);

/// How the GPU's work that a piece of work after it waited for ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuEnd {
    /// The GPU finished it: what it used can go.
    Finished,
    /// The renderer's graphics objects are lost, or may be: what the work
    /// used is to be forgotten, not destroyed.
    Lost,
}

/// A piece of work after the GPU.
type Work = Box<dyn FnOnce(GpuEnd)>;

impl AfterGpu {
    /// Runs `work` once the GPU finished the renderer's work so far, or its
    /// graphics objects are lost, as its argument says; at once, as lost,
    /// once the renderer went.
    pub fn push(&self, work: impl FnOnce(GpuEnd) + 'static) {
        match self.0.upgrade() {
            Some(queue) => {
                let mut waiting = queue.take();
                waiting.push(Box::new(work));
                queue.set(waiting);
            }
            None => work(GpuEnd::Lost),
        }
    }
}

/// The work pushed to the [`AfterGpu`]s of a renderer and not run yet,
/// which runs as [`GpuEnd::Lost`] if the queue drops first.
pub(crate) struct AfterGpuQueue {
    work: Rc<Cell<Vec<Work>>>,
    handle: AfterGpu,
}

impl AfterGpuQueue {
    pub(crate) fn new() -> Self {
        let work = Rc::default();
        let handle = AfterGpu(Rc::downgrade(&work));
        AfterGpuQueue { work, handle }
    }

    /// A handle that pushes to the queue.
    pub(crate) fn handle(&self) -> &AfterGpu {
        &self.handle
    }

    /// Takes the work that waits, in the order it was pushed. Work that it
    /// pushes as it runs waits for the next run.
    fn take(&self) -> Vec<Work> {
        self.work.take()
    }

    /// Runs the work that waits, in the order it was pushed, once `gpu`
    /// finished what the renderer gave it, if any work waits.
    pub(crate) fn finish(&self, gpu: &mut dyn Gpu) {
        let work = self.take();
        if work.is_empty() {
            return;
        }
        if let Err(error) = gpu.flush(true) {
            log::error!(target: log_target::RENDER, "Waiting for the GPU before the work after it failed: {error}");
        }
        for work in work {
            work(GpuEnd::Finished);
        }
    }

    /// Runs the work that waits, in the order it was pushed, as lost, and
    /// the work that it pushes.
    pub(crate) fn lose(&self) {
        loop {
            let work = self.take();
            if work.is_empty() {
                return;
            }
            for work in work {
                work(GpuEnd::Lost);
            }
        }
    }
}

impl Drop for AfterGpuQueue {
    fn drop(&mut self) {
        self.lose();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{AfterGpu, AfterGpuQueue, GpuEnd};

    type Ends = Rc<RefCell<Vec<(&'static str, GpuEnd)>>>;

    /// Pushes to `after` work that records its end as `name`'s in `ends`.
    fn record(after: &AfterGpu, ends: &Ends, name: &'static str) {
        let ends = Rc::clone(ends);
        after.push(move |end| ends.borrow_mut().push((name, end)));
    }

    #[test]
    fn work_runs_once_in_order_and_as_lost_once_the_queue_goes() {
        let ends = Ends::default();
        let queue = AfterGpuQueue::new();
        let after = queue.handle().clone();
        record(&after, &ends, "first");
        record(&after, &ends, "second");
        queue.lose();
        queue.lose();
        assert_eq!(ends.take(), [("first", GpuEnd::Lost), ("second", GpuEnd::Lost)]);

        record(&after, &ends, "waiting");
        drop(queue);
        record(&after, &ends, "late");
        assert_eq!(ends.take(), [("waiting", GpuEnd::Lost), ("late", GpuEnd::Lost)]);
    }
}
