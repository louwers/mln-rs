//! Work that waits for the GPU to finish what the renderer gave it.

use std::cell::RefCell;
use std::rc::Rc;

/// The renderer's work after the GPU: what runs once the GPU finished the
/// work that the renderer gave it, or once the renderer's graphics objects
/// are lost, such as a host's freeing of what it drew with outside the
/// renderer. A layer keeps a clone ([`NewCx::after_gpu`](crate::layer::NewCx))
/// and pushes to it, typically when it drops. The renderer runs what was
/// pushed at its next update, after it waited for the GPU, or when it is
/// torn down, clears its data or loses its graphics objects.
#[derive(Clone, Default)]
pub struct AfterGpu(Rc<Queue>);

/// How the GPU's work that a piece of [`AfterGpu`] waited for ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuEnd {
    /// The GPU finished it: what it used can go.
    Finished,
    /// The renderer's graphics objects are lost, or may be: what it used
    /// is to be forgotten, not destroyed.
    Lost,
}

/// The work pushed and not run yet. Work still here when the last
/// [`AfterGpu`] goes runs as [`GpuEnd::Lost`].
#[derive(Default)]
struct Queue(RefCell<Vec<Work>>);

/// A piece of work after the GPU.
type Work = Box<dyn FnOnce(GpuEnd)>;

impl AfterGpu {
    /// Runs `work` once the GPU finished the renderer's work so far, or its
    /// graphics objects are lost, as its argument says.
    pub fn push(&self, work: impl FnOnce(GpuEnd) + 'static) {
        self.0.0.borrow_mut().push(Box::new(work));
    }

    /// Whether no work waits.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.0.borrow().is_empty()
    }

    /// Runs the work that waits, in the order it was pushed, as `end` says.
    pub(crate) fn run(&self, end: GpuEnd) {
        let work = std::mem::take(&mut *self.0.0.borrow_mut());
        for work in work {
            work(end);
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        for work in self.0.get_mut().drain(..) {
            work(GpuEnd::Lost);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn work_runs_once_in_order_and_as_lost_when_the_queue_goes() {
        let ends = Rc::new(RefCell::new(Vec::new()));
        let after = AfterGpu::default();
        for name in ["first", "second"] {
            let ends = Rc::clone(&ends);
            after.push(move |end| ends.borrow_mut().push((name, end)));
        }
        after.run(GpuEnd::Finished);
        after.run(GpuEnd::Finished);
        assert_eq!(*ends.borrow(), [("first", GpuEnd::Finished), ("second", GpuEnd::Finished)]);

        let lost = Rc::new(Cell::new(false));
        let clone = after.clone();
        {
            let lost = Rc::clone(&lost);
            clone.push(move |end| lost.set(end == GpuEnd::Lost));
        }
        drop(after);
        assert!(!lost.get(), "a clone still holds the work");
        drop(clone);
        assert!(lost.get());
    }
}
