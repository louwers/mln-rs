// `AspiringActor<O>` is the mailbox that exists before the object (messages
// queue in it); `EstablishedActor<O>` constructs the object on the thread
// that will own it, opens the mailbox there, and on drop closes the mailbox
// before the object goes away.

use std::rc::Rc;
use std::sync::Arc;

use crate::actor_ref::ActorRef;
use crate::mailbox::Mailbox;
use crate::scheduler::SchedulerPtr;

/// The mailbox and the `ActorRef` handed out before the object exists.
pub(crate) struct AspiringActor<O: 'static> {
    pub(crate) mailbox: Arc<Mailbox>,
    _object: std::marker::PhantomData<fn(&O)>,
}

impl<O: 'static> AspiringActor<O> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_mailbox(Mailbox::new())
    }

    pub(crate) fn with_mailbox(mailbox: Arc<Mailbox>) -> Self {
        AspiringActor { mailbox, _object: std::marker::PhantomData }
    }

    #[must_use]
    pub(crate) fn self_ref(&self) -> ActorRef<O> {
        ActorRef::new(&self.mailbox)
    }
}

/// Owns the object on its thread.
pub(crate) struct EstablishedActor<O: 'static> {
    mailbox: Arc<Mailbox>,
    object: Option<Rc<O>>,
}

impl<O: 'static> EstablishedActor<O> {
    /// Creates the object for `parent`'s mailbox. `make` receives the
    /// actor's own reference and builds the object.
    pub(crate) fn new(
        scheduler: &SchedulerPtr,
        parent: &AspiringActor<O>,
        make: impl FnOnce(ActorRef<O>) -> O,
    ) -> Self {
        let object = Rc::new(make(parent.self_ref()));
        parent.mailbox.open(scheduler, Rc::<O>::clone(&object));
        EstablishedActor { mailbox: Arc::clone(&parent.mailbox), object: Some(object) }
    }
}

impl<O: 'static> Drop for EstablishedActor<O> {
    fn drop(&mut self) {
        self.mailbox.close();
        drop(self.object.take());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use super::*;
    use crate::run_loop::RunLoop;
    use crate::scheduler;

    // gtest: Actor.TwoPhaseConstruction
    #[test]
    fn two_phase_construction() {
        // This test mimics, in simplified form, the approach used by the
        // Thread<Object> to construct its actor in two parts so that the
        // Thread<Object> instance can be created without waiting for the target
        // thread to be up and running.
        struct TestActor {
            destroyed: Arc<AtomicBool>,
        }
        impl Drop for TestActor {
            fn drop(&mut self) {
                self.destroyed.store(true, Ordering::SeqCst);
            }
        }
        impl TestActor {
            fn call_me(p: &mpsc::Sender<()>) {
                p.send(()).unwrap();
            }
            fn stop() {
                RunLoop::get().stop();
            }
        }

        let parent = AspiringActor::<TestActor>::new();

        let destroyed = Arc::new(AtomicBool::new(false));

        let (queue_executed, queue_executed_future) = mpsc::channel();

        parent.self_ref().invoke(move |_: &TestActor| TestActor::call_me(&queue_executed));
        parent.self_ref().invoke(|_: &TestActor| TestActor::stop());

        std::thread::scope(|s| {
            let captured = Arc::clone(&destroyed);
            let parent = &parent;
            let thread = s.spawn(move || {
                let run_loop = RunLoop::new();
                let _test = EstablishedActor::new(&scheduler::get_current(), parent, move |_| TestActor {
                    destroyed: captured,
                });
                run_loop.run();
            });

            // should not hang
            queue_executed_future.recv_timeout(std::time::Duration::from_secs(60)).expect("should not hang");
            thread.join().unwrap();
        });

        assert!(destroyed.load(Ordering::SeqCst));
    }
}
