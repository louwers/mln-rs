//! The threads mln-rs starts, and the waits for them to end.
//!
//! A runtime or a render session owns library threads, which [`spawn`]
//! starts, and a [`Joined`] waits for one to end when it drops. On a host's
//! thread it waits at once. On a library thread it waits once that thread's
//! work returns, since in the middle of its work the thread could be what
//! the other one waits for. Two library threads that would each wait at
//! their ends for the other don't: the second to start waiting leaves the
//! first to end alone.

use std::cell::{Cell, RefCell};
use std::thread::{JoinHandle, ThreadId};

use parking_lot::Mutex;

/// The stack size of every thread mln-rs starts. Parsing and evaluating
/// style JSON, filters and expressions recurses once per nesting level, so
/// a thread gets this much whatever `RUST_MIN_STACK` or the platform's
/// default would give it.
pub(crate) const STACK_SIZE: usize = 2 * 1024 * 1024;

/// A builder for a thread named `name` with a `STACK_SIZE` stack. It isn't a
/// library thread: a [`Joined`] that drops on it waits at once, so a thread
/// that a runtime's or a session's work may wait for starts with [`spawn`].
pub fn builder(name: impl Into<String>) -> std::thread::Builder {
    std::thread::Builder::new().name(name.into()).stack_size(STACK_SIZE)
}

/// Starts a library thread named `name`, with a `STACK_SIZE` stack, that
/// runs `work`, then waits for the threads that [`Joined`]s dropped on it
/// left to it.
///
/// # Errors
///
/// Fails when the operating system can't start the thread.
pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    builder(name).spawn(move || {
        let _end = LibraryThread::enter();
        work()
    })
}

thread_local! {
    /// Whether [`spawn`] started this thread.
    static LIBRARY: Cell<bool> = const { Cell::new(false) };
    /// The threads that this library thread waits for at its end.
    static AT_END: RefCell<Vec<JoinHandle<()>>> = const { RefCell::new(Vec::new()) };
}

/// The library threads that wait at their ends, and the thread each waits
/// for.
static WAITING: Mutex<Vec<(ThreadId, ThreadId)>> = Mutex::new(Vec::new());

/// Marks this thread as a library thread, and at its drop waits for the
/// threads that it was left.
struct LibraryThread;

impl LibraryThread {
    fn enter() -> LibraryThread {
        LIBRARY.with(|library| library.set(true));
        LibraryThread
    }
}

impl Drop for LibraryThread {
    fn drop(&mut self) {
        // Waiting leaves this thread nothing more, since it runs no work.
        let left = AT_END.with(|left| std::mem::take(&mut *left.borrow_mut()));
        for thread in left {
            wait_at_end(thread);
        }
    }
}

/// Waits at this library thread's end for `thread`, unless `thread` waits for
/// this one, through the threads it waits for.
fn wait_at_end(thread: JoinHandle<()>) {
    let this = std::thread::current().id();
    {
        let mut waiting = WAITING.lock();
        let mut next = Some(thread.thread().id());
        while let Some(id) = next {
            if id == this {
                return;
            }
            next = waiting.iter().find(|(waiter, _)| *waiter == id).map(|(_, awaited)| *awaited);
        }
        waiting.push((this, thread.thread().id()));
    }
    let _ = thread.join();
    WAITING.lock().retain(|(waiter, _)| *waiter != this);
}

/// A library thread that ends by itself once its owner lets go of it, which
/// this waits for when it drops, as the module describes. A drop on the
/// thread itself leaves it to end alone.
#[derive(Debug)]
pub struct Joined(Option<JoinHandle<()>>);

impl Joined {
    /// Waits for `thread` when this drops.
    #[must_use]
    pub fn new(thread: JoinHandle<()>) -> Joined {
        Joined(Some(thread))
    }

    /// The thread's ID.
    #[must_use]
    pub fn id(&self) -> Option<ThreadId> {
        self.0.as_ref().map(|thread| thread.thread().id())
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        let Some(thread) = self.0.take() else { return };
        if thread.thread().id() == std::thread::current().id() {
            return;
        }
        // A thread whose locals are gone is past its work: it waits at once.
        if LIBRARY.try_with(Cell::get).unwrap_or(false) {
            let mut thread = Some(thread);
            let left = AT_END.try_with(|left| left.borrow_mut().extend(thread.take()));
            if left.is_ok() {
                return;
            }
            if let Some(thread) = thread {
                let _ = thread.join();
            }
            return;
        }
        let _ = thread.join();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use super::*;

    #[test]
    fn names_the_thread() {
        let name = builder("parse").spawn(|| std::thread::current().name().map(str::to_owned)).unwrap().join();
        assert_eq!(name.unwrap().as_deref(), Some("parse"));
    }

    /// A library thread that ends once `gate` closes, and the flag that it
    /// sets as it ends.
    fn gated() -> (Joined, mpsc::Sender<()>, Arc<AtomicBool>) {
        let (open, gate) = mpsc::channel::<()>();
        let ended = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ended);
        let thread = spawn("gated", move || {
            let _ = gate.recv();
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::SeqCst);
        })
        .unwrap();
        (Joined::new(thread), open, ended)
    }

    #[test]
    fn a_drop_on_a_host_thread_waits_for_the_thread() {
        let (joined, open, ended) = gated();
        drop(open);
        drop(joined);
        assert!(ended.load(Ordering::SeqCst));
    }

    #[test]
    fn a_drop_on_a_library_thread_waits_at_its_end() {
        let (joined, open, ended) = gated();
        let (dropped, was_dropped) = mpsc::channel();
        let ended_then = Arc::clone(&ended);
        let owner = spawn("owner", move || {
            drop(joined);
            let _ = dropped.send(ended_then.load(Ordering::SeqCst));
            // The gated thread ends only once this one's work returned.
            drop(open);
        })
        .unwrap();
        assert!(!was_dropped.recv().unwrap(), "the drop waited in the middle of the owner's work");
        owner.join().unwrap();
        assert!(ended.load(Ordering::SeqCst));
    }

    #[test]
    fn a_drop_on_the_thread_itself_leaves_it() {
        let (joined, joined_here) = mpsc::channel::<Joined>();
        let thread = spawn("itself", move || drop(joined_here.recv().unwrap())).unwrap();
        joined.send(Joined::new(thread)).unwrap();
        // The thread ends, and nothing waits for it.
        drop(joined);
    }

    /// A flag that this thread sets once it has ended.
    fn ended_flag() -> Arc<AtomicBool> {
        struct SetOnDrop(Arc<AtomicBool>);
        impl Drop for SetOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        thread_local! {
            static ENDED: RefCell<Option<SetOnDrop>> = const { RefCell::new(None) };
        }
        let ended = Arc::new(AtomicBool::new(false));
        ENDED.with(|flag| *flag.borrow_mut() = Some(SetOnDrop(Arc::clone(&ended))));
        ended
    }

    #[test]
    fn library_threads_that_wait_for_each_other_at_their_ends_end() {
        let (first_joined, first_here) = mpsc::channel::<Joined>();
        let (second_joined, second_here) = mpsc::channel::<Joined>();
        let (flags, flag) = mpsc::channel();
        let first_flags = flags.clone();
        let first = spawn("first", move || {
            first_flags.send(ended_flag()).unwrap();
            drop(first_here.recv().unwrap());
        })
        .unwrap();
        let second = spawn("second", move || {
            flags.send(ended_flag()).unwrap();
            drop(second_here.recv().unwrap());
        })
        .unwrap();
        let ended = [flag.recv().unwrap(), flag.recv().unwrap()];
        first_joined.send(Joined::new(second)).unwrap();
        second_joined.send(Joined::new(first)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ended.iter().all(|ended| ended.load(Ordering::SeqCst)) {
            assert!(std::time::Instant::now() < deadline, "the threads wait for each other");
            std::thread::yield_now();
        }
    }
}
