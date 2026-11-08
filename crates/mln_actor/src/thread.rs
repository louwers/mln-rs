// Ports include/mln/util/thread.hpp: `util::Thread<Object>` runs an actor
// on its own thread with its own RunLoop.
//
// The C++ constructor forwards the object's constructor arguments to the
// thread; here `make` is a `Send` closure that builds the object on the new
// thread (it gets the actor's own `ActorRef`, like `O(ActorRef<O>, args...)`).
// Waiting for the thread to be running (`running.wait()`) happens in
// `pause` and `drop`, as in C++ (gtests-core: it used to happen at the end of
// the constructor, which deadlocks when the object's constructor waits for
// the caller, test/util/thread.test.cpp InvokeBeforeChildStarts).
//
// TODO(port): the thread priority setters (`makeThreadPrioritySetter`,
// platform settings) and `platform::attachThread`/`detachThread` (no-ops on
// Linux).

use std::sync::{Mutex, OnceLock, mpsc};
use std::thread::JoinHandle;

use mln_util::sync::MutexExt;

use crate::actor_ref::ActorRef;
use crate::established_actor::{AspiringActor, EstablishedActor};
use crate::run_loop::{Priority, RunLoop, RunLoopHandle};
use crate::scheduler;

/// `util::Thread<Object>`.
pub struct Thread<O: 'static> {
    object: AspiringActor<O>,
    thread: Option<JoinHandle<()>>,
    /// `running`: the child's loop, once its object is established.
    running: Mutex<Option<mpsc::Receiver<RunLoopHandle>>>,
    run_loop: OnceLock<RunLoopHandle>,
    paused: Mutex<Option<mpsc::Sender<()>>>,
}

impl<O: 'static> Thread<O> {
    /// `Thread(name, args...)`.
    pub fn new(name: &str, make: impl FnOnce(ActorRef<O>) -> O + Send + 'static) -> Self {
        let object = AspiringActor::<O>::new();
        let mailbox = object.mailbox.clone();
        let (running_tx, running_rx) = mpsc::channel::<RunLoopHandle>();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let run_loop = RunLoop::new();
                let parent = AspiringActor::<O>::with_mailbox(mailbox);
                let established_actor = EstablishedActor::new(&scheduler::get_current(), &parent, make);
                let _ = running_tx.send(run_loop.handle());
                run_loop.run();
                drop(established_actor);
            })
            .expect("failed to spawn thread");
        Thread {
            object,
            thread: Some(thread),
            running: Mutex::new(Some(running_rx)),
            run_loop: OnceLock::new(),
            paused: Mutex::new(None),
        }
    }

    /// `running.wait()`, then the child's `loop`.
    fn run_loop(&self) -> &RunLoopHandle {
        self.run_loop.get_or_init(|| {
            let running = self.running.lock_unpoisoned().take().expect("running");
            running.recv().expect("thread failed to start")
        })
    }

    /// `actor()`: a reference to the object on the thread. Messages sent
    /// before the object exists are queued.
    pub fn actor(&self) -> ActorRef<O> {
        self.object.self_ref()
    }

    /// `pause()`: blocks the thread's loop (high priority task) until
    /// `resume()`.
    pub fn pause(&self) {
        let mut paused = self.paused.lock_unpoisoned();
        assert!(paused.is_none());
        let (paused_tx, paused_rx) = mpsc::channel::<()>();
        let (resumed_tx, resumed_rx) = mpsc::channel::<()>();
        self.run_loop().invoke_with_priority(Priority::High, move || {
            let _ = paused_tx.send(());
            let _ = resumed_rx.recv();
        });
        let _ = paused_rx.recv();
        *paused = Some(resumed_tx);
    }

    /// `resume()`.
    pub fn resume(&self) {
        let resumed = self.paused.lock_unpoisoned().take();
        assert!(resumed.is_some());
        if let Some(resumed) = resumed {
            let _ = resumed.send(());
        }
    }
}

impl<O: 'static> Drop for Thread<O> {
    /// `~Thread()`: resume if paused, wait for the queue up to here, stop
    /// the loop and join.
    fn drop(&mut self) {
        if self.paused.lock_unpoisoned().is_some() {
            self.resume();
        }
        let (stoppable_tx, stoppable_rx) = mpsc::channel::<()>();
        let run_loop = self.run_loop().clone();
        run_loop.invoke(move || {
            let _ = stoppable_tx.send(());
        });
        let _ = stoppable_rx.recv();
        run_loop.stop();
        if let Some(thread) = self.thread.take() {
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

    #[test]
    fn pause_resume() {
        let total = Arc::new(AtomicUsize::new(0));
        let t = total.clone();
        let thread = Thread::new("Test", move |_| Counter { count: Cell::new(0), total: t });
        thread.pause();
        let seen = Arc::new(AtomicUsize::new(0));
        let s = seen.clone();
        thread.actor().invoke(move |_: &Counter| {
            s.store(1, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(seen.load(Ordering::SeqCst), 0);
        thread.resume();
        drop(thread);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }
}
