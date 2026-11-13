use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::actor::{Actor, PoolActorRef};
use crate::scheduler;
use crate::tests::gtest::{get, wait};

// gtest: ActorRef.CanOutliveActor
#[test]
fn can_outlive_actor() {
    // An ActorRef can outlive its actor. Doing does not extend the actor's lifetime.
    // Sending a message to an ActorRef whose actor has died is a no-op.
    struct TestActorRef {
        died: Arc<AtomicBool>,
    }
    impl Drop for TestActorRef {
        fn drop(&mut self) {
            self.died.store(true, Ordering::SeqCst);
        }
    }

    let died = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicBool::new(false));

    let test: PoolActorRef<TestActorRef> = {
        let d = Arc::clone(&died);
        Actor::new(scheduler::get_background(), move |_| TestActorRef { died: d }).self_ref()
    };

    assert!(died.load(Ordering::SeqCst));
    let r = Arc::clone(&received);
    // `receive() { FAIL(); }`
    test.invoke(move |_: &mut TestActorRef| r.store(true, Ordering::SeqCst));
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(!received.load(Ordering::SeqCst));
}

// gtest: ActorRef.Ask
#[test]
fn ask() {
    // Ask returns a Future eventually returning the result
    struct TestActorRef;
    impl TestActorRef {
        fn gimme() -> i32 {
            20
        }
        fn echo(i: i32) -> i32 {
            i
        }
    }

    let actor = Actor::new(scheduler::get_background(), |_| TestActorRef);
    let r: PoolActorRef<TestActorRef> = actor.self_ref();

    assert_eq!(20, get(&r.ask(|_: &mut TestActorRef| TestActorRef::gimme())));
    assert_eq!(30, get(&r.ask(|_: &mut TestActorRef| TestActorRef::echo(30))));
}

// gtest: ActorRef.AskVoid
#[test]
fn ask_void() {
    // Ask waits for void methods
    struct TestActorRef {
        executed: Arc<AtomicBool>,
    }
    impl TestActorRef {
        fn do_it(&mut self) {
            self.executed.store(true, Ordering::SeqCst);
        }
    }

    let executed = Arc::new(AtomicBool::new(false));
    let e = Arc::clone(&executed);
    let actor = Actor::new(scheduler::get_background(), move |_| TestActorRef { executed: e });
    let r: PoolActorRef<TestActorRef> = actor.self_ref();

    wait(&r.ask(TestActorRef::do_it));
    assert!(executed.load(Ordering::SeqCst));
}

// gtest: ActorRef.AskOnDestroyedActor
#[test]
fn ask_on_destroyed_actor() {
    // Tests behavior when calling ask() after the
    // Actor has gone away. Should set a exception_ptr.
    struct TestActorRef {
        died: Arc<AtomicBool>,
    }
    impl Drop for TestActorRef {
        fn drop(&mut self) {
            self.died.store(true, Ordering::SeqCst);
        }
    }
    impl TestActorRef {
        fn receive() -> i32 {
            1
        }
    }

    let died = Arc::new(AtomicBool::new(false));
    let d = Arc::clone(&died);
    let actor = Actor::new(scheduler::get_background(), move |_| TestActorRef { died: d });
    let r: PoolActorRef<TestActorRef> = actor.self_ref();

    drop(actor);
    assert!(died.load(Ordering::SeqCst));

    // C++ leaves the rest commented out (a use-after-free with newer gcc,
    // maplibre-native#876): `auto result = ref.ask(&TestActorRef::receive);
    // EXPECT_ANY_THROW(result.get());`. The port has no dangling pointer, so
    // it checks it: the future is broken (C++ std::future_error).
    let result = r.ask(|_: &mut TestActorRef| TestActorRef::receive());
    assert!(result.recv().is_err());
}
