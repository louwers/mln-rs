// Ports include/mln/util/thread.hpp: `util::Thread<Object>` runs an actor
// on its own thread with its own RunLoop.
//
// The C++ constructor forwards the object's constructor arguments to the
// thread; here `make` is a `Send` closure that builds the object on the new
// thread (it gets the actor's own `ActorRef`, like `O(ActorRef<O>, args...)`).
// Waiting for the thread to be running (`running.wait()`) happens in `drop`,
// as in C++, not at the end of the constructor, which would deadlock when the
// object's constructor waits for the caller (test/util/thread.test.cpp
// InvokeBeforeChildStarts).
//
// TODO(port): the thread priority setters (`makeThreadPrioritySetter`,
// platform settings) and `platform::attachThread`/`detachThread` (no-ops on
// Linux).
//
// Port-only: with a cooperative scheduler installed (a host without threads,
// see cooperative_scheduler.rs) no thread starts: the object is established
// on the creating thread's RunLoop, which the host drives, and its messages
// run there.

use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;

use mln_util::sync::MutexExt;

use crate::actor_ref::ActorRef;
use crate::established_actor::{AspiringActor, EstablishedActor};
use crate::local_object::LocalObject;
use crate::run_loop::{RunLoop, RunLoopHandle};
use crate::scheduler;

/// `util::Thread<Object>`.
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
    /// Port-only: on the creating thread, with a cooperative scheduler.
    /// Dropping it drops the object.
    Local { _established: LocalObject<EstablishedActor<O>> },
}

impl<O: 'static> Thread<O> {
    /// `Thread(name, args...)`.
    pub fn new(name: &str, make: impl FnOnce(ActorRef<O>) -> O + Send + 'static) -> Self {
        let object = AspiringActor::<O>::new();
        if scheduler::cooperative().is_some() {
            let established = EstablishedActor::new(&scheduler::get_current(), &object, make);
            return Thread { object, backing: Backing::Local { _established: LocalObject::new(established) } };
        }
        let mailbox = object.mailbox.clone();
        let (running_tx, running_rx) = mpsc::channel::<RunLoopHandle>();
        let thread = mln_util::thread::builder(name)
            .spawn(move || {
                let run_loop = RunLoop::new();
                let parent = AspiringActor::<O>::with_mailbox(mailbox);
                let established_actor = EstablishedActor::new(&scheduler::get_current(), &parent, make);
                let _ = running_tx.send(run_loop.handle());
                run_loop.run();
                drop(established_actor);
            })
            .expect("failed to spawn thread");
        Thread { object, backing: Backing::Spawned { thread: Some(thread), running: Mutex::new(running_rx) } }
    }

    /// `actor()`: a reference to the object on the thread. Messages sent
    /// before the object exists are queued.
    pub fn actor(&self) -> ActorRef<O> {
        self.object.self_ref()
    }
}

impl<O: 'static> Drop for Thread<O> {
    /// `~Thread()`: wait for the queue up to here, stop the loop and join.
    fn drop(&mut self) {
        let Backing::Spawned { thread, running } = &mut self.backing else {
            return;
        };
        let (stoppable_tx, stoppable_rx) = mpsc::channel::<()>();
        // `running.wait()`.
        let run_loop = running.lock_unpoisoned().recv().expect("thread failed to start");
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
        let t = total.clone();
        let thread = Thread::new("Test", move |_| Counter { count: Cell::new(0), total: t });
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
}
