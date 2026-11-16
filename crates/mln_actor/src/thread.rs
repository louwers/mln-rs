// The C++ constructor forwards the object's constructor arguments to the
// thread; here `make` is a `Send` closure that builds the object on the new
// thread (it gets the actor's own `ActorRef`, like `O(ActorRef<O>, args...)`,
// and the thread's loop, which C++ finds as the thread's current one).
// Waiting for the thread to be running (`running.wait()`) happens in `drop`,
// as in C++, not at the end of the constructor, which would deadlock when the
// object's constructor waits for the caller (test/util/thread.test.cpp
// InvokeBeforeChildStarts).
//
// Port-only: with a cooperative scheduler installed (a host without threads,
// see cooperative_scheduler.rs) no thread starts: the object is established
// on a RunLoop of the creating thread, which the cooperative scheduler runs
// when the host runs its tasks, and its messages run there.

use std::sync::mpsc;
use std::thread::JoinHandle;

use parking_lot::Mutex;

use crate::actor_ref::ActorRef;
use crate::cooperative_scheduler::CooperativeScheduler;
use crate::established_actor::{AspiringActor, EstablishedActor};
use crate::local_object::LocalObject;
use crate::run_loop::{RunLoop, RunLoopHandle};
use crate::scheduler;

pub struct Thread<O: 'static> {
    object: AspiringActor<O>,
    backing: Backing<O>,
}

/// Where the object lives.
enum Backing<O: 'static> {
    /// On a thread of its own (C++).
    Spawned {
        thread: Option<JoinHandle<()>>,
        /// `running`: the child's loop, once its object is established (in a
        /// `Mutex` so that a `Thread` is `Sync`).
        running: Mutex<mpsc::Receiver<RunLoopHandle>>,
    },
    /// Port-only: on the creating thread, on a loop that the cooperative
    /// scheduler runs. Dropping it drops the object.
    Local {
        established: Option<LocalObject<EstablishedActor<O>>>,
        scheduler: &'static std::sync::Arc<CooperativeScheduler>,
        key: u64,
    },
}

impl<O: 'static> Thread<O> {
    /// `Thread(name, args...)`: `make` builds the object on the thread,
    /// from the actor's own reference and the thread's loop.
    pub fn new(name: &str, make: impl FnOnce(ActorRef<O>, &RunLoop) -> O + Send + 'static) -> Self {
        let object = AspiringActor::<O>::new();
        if let Some(scheduler) = scheduler::cooperative() {
            let run_loop = RunLoop::new();
            let established = EstablishedActor::new(&run_loop.handle(), &object, |self_ref| make(self_ref, &run_loop));
            let key = scheduler.host(&run_loop);
            let established = Some(LocalObject::new(established));
            return Thread { object, backing: Backing::Local { established, scheduler, key } };
        }
        let mailbox = std::sync::Arc::clone(&object.mailbox);
        let (running_tx, running_rx) = mpsc::channel::<RunLoopHandle>();
        let thread = mln_util::thread::spawn(name, move || {
            let run_loop = RunLoop::new();
            let parent = AspiringActor::<O>::with_mailbox(mailbox);
            let established_actor =
                EstablishedActor::new(&run_loop.handle(), &parent, |self_ref| make(self_ref, &run_loop));
            let _ = running_tx.send(run_loop.handle());
            run_loop.run();
            drop(established_actor);
        })
        .expect("failed to spawn thread");
        Thread { object, backing: Backing::Spawned { thread: Some(thread), running: Mutex::new(running_rx) } }
    }

    /// Returns a reference to the object on the thread. Messages sent before
    /// the object exists are queued.
    pub fn actor(&self) -> ActorRef<O> {
        self.object.self_ref()
    }
}

impl<O: 'static> Drop for Thread<O> {
    /// Waits for the queue up to here, stops the loop and joins the thread.
    /// Dropped on its own thread, by the last of its owners there, it only
    /// stops the loop, which ends once the task that drops it returns.
    fn drop(&mut self) {
        let (thread, running) = match &mut self.backing {
            Backing::Spawned { thread, running } => (thread, running),
            Backing::Local { established, scheduler, key } => {
                scheduler.unhost(*key);
                drop(established.take());
                return;
            }
        };
        let (stoppable_tx, stoppable_rx) = mpsc::channel::<()>();
        // `running.wait()`.
        let run_loop = running.lock().recv().expect("thread failed to start");
        if thread.as_ref().is_some_and(|thread| thread.thread().id() == std::thread::current().id()) {
            run_loop.stop();
            return;
        }
        run_loop.invoke(move || {
            let _ = stoppable_tx.send(());
        });
        let _ = stoppable_rx.recv();
        run_loop.stop();
        if let Some(thread) = thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Counter {
        count: Cell<usize>,
        total: Arc<AtomicUsize>,
    }

    impl Drop for Counter {
        fn drop(&mut self) {
            self.total.store(self.count.get(), Ordering::SeqCst);
        }
    }

    #[test]
    fn messages_run_on_the_thread_in_order() {
        let total = Arc::new(AtomicUsize::new(0));
        let t = Arc::clone(&total);
        let thread = Thread::new("Test", move |_, _| Counter { count: Cell::new(0), total: t });
        let main = std::thread::current().id();
        for i in 0..100 {
            thread.actor().invoke(move |counter: &Counter| {
                assert_ne!(std::thread::current().id(), main);
                assert_eq!(counter.count.get(), i);
                counter.count.set(i + 1);
            });
        }
        drop(thread);
        assert_eq!(total.load(Ordering::SeqCst), 100);
    }
    /// The last owner of a thread may be a message on that thread.
    #[test]
    fn a_thread_dropped_on_itself_ends_without_waiting_for_itself() {
        let total = Arc::new(AtomicUsize::new(0));
        let thread = Arc::new(Mutex::new(Some(Thread::new("Test", {
            let total = Arc::clone(&total);
            move |_, _| Counter { count: Cell::new(7), total }
        }))));
        let (done, finished) = mpsc::channel();
        let owner = Arc::clone(&thread);
        let actor = thread.lock().as_ref().unwrap().actor();
        actor.invoke(move |_: &Counter| {
            drop(owner.lock().take());
            done.send(()).unwrap();
        });
        finished.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        while total.load(Ordering::SeqCst) != 7 {
            std::thread::yield_now();
        }
    }
}
