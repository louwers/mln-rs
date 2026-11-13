//! The threads mln-rs starts, and the waits for them to end.
//!
//! A runtime or a render session owns library threads, which [`spawn`]
//! starts, and a [`Joined`] waits for one to end when it drops on a host's
//! thread. On a library thread it doesn't wait, since the thread it would
//! wait for could be waiting for this one: it leaves it to end alone.

use std::cell::Cell;
use std::thread::JoinHandle;

use parking_lot::Mutex;

/// The stack size of every thread mln-rs starts. Parsing and evaluating
/// style JSON, filters and expressions recurses once per nesting level, so
/// a thread gets this much whatever `RUST_MIN_STACK` or the platform's
/// default would give it.
pub(crate) const STACK_SIZE: usize = 2 * 1024 * 1024;

/// A builder for a thread named `name` with a `STACK_SIZE` stack. It isn't a
/// library thread: a [`Joined`] that drops on it waits, so a thread that a
/// runtime's or a session's work may wait for starts with [`spawn`].
pub fn builder(name: impl Into<String>) -> std::thread::Builder {
    std::thread::Builder::new().name(name.into()).stack_size(STACK_SIZE)
}

/// Starts a library thread named `name`, with a `STACK_SIZE` stack, that
/// runs `work`.
///
/// # Errors
///
/// Fails when the operating system can't start the thread.
pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    builder(name).spawn(move || {
        LIBRARY.with(|library| library.set(true));
        work()
    })
}

thread_local! {
    /// Whether [`spawn`] started this thread.
    static LIBRARY: Cell<bool> = const { Cell::new(false) };
}

/// A library thread that ends by itself once its owner lets go of it, which
/// this waits for when it drops on a host's thread, unless it was
/// [`detach`](Self::detach)ed. On a library thread, whose locals are still
/// there, the drop leaves the thread to end alone.
#[derive(Debug)]
pub struct Joined(Mutex<Option<JoinHandle<()>>>);

impl Joined {
    /// Waits for `thread` when this drops on a host's thread.
    #[must_use]
    pub fn new(thread: JoinHandle<()>) -> Joined {
        Joined(Mutex::new(Some(thread)))
    }

    /// Leaves the thread to end alone: the drop no longer waits for it.
    pub fn detach(&self) {
        self.0.lock().take();
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        let Some(thread) = self.0.get_mut().take() else { return };
        if !LIBRARY.try_with(Cell::get).unwrap_or(false) {
            let _ = thread.join();
        }
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
    fn a_drop_on_a_library_thread_leaves_the_thread() {
        let (joined, open, ended) = gated();
        let owner = spawn("owner", move || drop(joined)).unwrap();
        owner.join().unwrap();
        assert!(!ended.load(Ordering::SeqCst));
        drop(open);
    }

    #[test]
    fn a_drop_on_the_thread_itself_leaves_it() {
        let (joined, joined_here) = mpsc::channel::<Joined>();
        let thread = spawn("itself", move || drop(joined_here.recv().unwrap())).unwrap();
        joined.send(Joined::new(thread)).unwrap();
        // The thread ends, and nothing waits for it.
        drop(joined);
    }

    #[test]
    fn a_detached_thread_is_left_to_end_alone() {
        let (joined, open, ended) = gated();
        joined.detach();
        drop(joined);
        assert!(!ended.load(Ordering::SeqCst));
        drop(open);
    }
}
