//! The result of a command that runs on a runtime's thread.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};
use std::thread::ThreadId;

use mln_util::error::Error;
use mln_util::sync::{CondvarExt, MutexExt};

type Callback<T> = Box<dyn FnOnce(Result<T, Error>) + Send>;

enum State<T> {
    Pending {
        waker: Option<Waker>,
        callback: Option<Callback<T>>,
    },
    Ready(Result<T, Error>),
    /// The result went to its receiver.
    Taken,
}

/// The thread that resolves a completion, where waiting for it would never
/// end.
pub(crate) enum Home {
    Anywhere,
    Thread(ThreadId),
    /// The thread that the cell names once it is set, and a thread whose
    /// work the result waits for on the way.
    Later(Arc<OnceLock<ThreadId>>, ThreadId),
}

impl Home {
    fn is_current(&self) -> bool {
        let current = std::thread::current().id();
        match self {
            Home::Anywhere => false,
            Home::Thread(thread) => *thread == current,
            Home::Later(thread, via) => thread.get() == Some(&current) || *via == current,
        }
    }
}

struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
    home: Home,
}

/// The result of a command, which arrives once the command has run.
///
/// Receive it in one of three ways: await it (it is a [`Future`], with no
/// executor of its own), block on it with [`wait`](Self::wait), or pass a
/// callback to [`on_complete`](Self::on_complete). A command whose runtime
/// or map goes away before it runs completes with
/// [`ErrorKind::Cancelled`](mln_util::ErrorKind::Cancelled).
#[must_use = "a completion does nothing unless it is received"]
pub struct Completion<T> {
    shared: Arc<Shared<T>>,
}

/// The sending side of a [`Completion`]. Dropping it unresolved cancels the
/// completion.
pub(crate) struct Resolver<T> {
    shared: Option<Arc<Shared<T>>>,
}

impl<T: Send + 'static> Completion<T> {
    /// A completion that a resolver on the thread `home` resolves.
    pub(crate) fn new(home: Option<ThreadId>) -> (Completion<T>, Resolver<T>) {
        Completion::at(home.map_or(Home::Anywhere, Home::Thread))
    }

    /// A completion that a resolver on the thread `home` resolves.
    pub(crate) fn at(home: Home) -> (Completion<T>, Resolver<T>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::Pending { waker: None, callback: None }),
            ready: Condvar::new(),
            home,
        });
        (Completion { shared: shared.clone() }, Resolver { shared: Some(shared) })
    }

    /// A completion that already has its result.
    pub fn ready(result: Result<T, Error>) -> Completion<T> {
        let (completion, resolver) = Completion::new(None);
        resolver.resolve(result);
        completion
    }

    /// Whether the result has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !matches!(*self.shared.state.lock_unpoisoned(), State::Pending { .. })
    }

    /// Blocks until the result arrives, and returns it.
    ///
    /// A wait on the thread that resolves the completion would never end,
    /// such as the runtime's own thread, or the thread that runs a render
    /// session's [`DriverWork`](crate::DriverWork). A render session's
    /// completions wait for its map's runtime too, as a resize or a detach
    /// does, so their wait on the runtime's thread would never end either.
    /// Such a wait fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind::InvalidState) instead,
    /// unless the result is already there.
    pub fn wait(self) -> Result<T, Error> {
        let mut state = self.shared.state.lock_unpoisoned();
        loop {
            match std::mem::replace(&mut *state, State::Taken) {
                State::Ready(result) => return result,
                pending @ State::Pending { .. } => {
                    *state = pending;
                    if self.shared.home.is_current() {
                        return Err(Error::invalid_state(
                            "a completion can't be waited for on the thread that resolves it",
                        ));
                    }
                    state = self.shared.ready.wait_unpoisoned(state);
                }
                State::Taken => return Err(Error::invalid_state("the completion's result was already taken")),
            }
        }
    }

    /// Calls `callback` with the result once it arrives: on the thread that
    /// resolves the completion, or at once on this thread when the result is
    /// already there.
    pub fn on_complete(self, callback: impl FnOnce(Result<T, Error>) + Send + 'static) {
        let result = {
            let mut state = self.shared.state.lock_unpoisoned();
            match std::mem::replace(&mut *state, State::Taken) {
                State::Ready(result) => result,
                State::Pending { waker, .. } => {
                    *state = State::Pending { waker, callback: Some(Box::new(callback)) };
                    return;
                }
                State::Taken => Err(Error::invalid_state("the completion's result was already taken")),
            }
        };
        callback(result);
    }
}

impl<T> Future for Completion<T> {
    type Output = Result<T, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.shared.state.lock_unpoisoned();
        match std::mem::replace(&mut *state, State::Taken) {
            State::Ready(result) => Poll::Ready(result),
            State::Pending { callback, .. } => {
                *state = State::Pending { waker: Some(cx.waker().clone()), callback };
                Poll::Pending
            }
            State::Taken => Poll::Ready(Err(Error::invalid_state("the completion's result was already taken"))),
        }
    }
}

impl<T> std::fmt::Debug for Completion<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let complete = !matches!(*self.shared.state.lock_unpoisoned(), State::Pending { .. });
        f.debug_struct("Completion").field("complete", &complete).finish()
    }
}

impl<T> Resolver<T> {
    /// Delivers `result`: to the callback, if one is waiting, or to the
    /// receiver. Nothing is locked while the callback or waker runs.
    pub(crate) fn resolve(mut self, result: Result<T, Error>) {
        if let Some(shared) = self.shared.take() {
            Self::deliver(&shared, result);
        }
    }

    fn deliver(shared: &Shared<T>, result: Result<T, Error>) {
        let (callback, waker) = {
            let mut state = shared.state.lock_unpoisoned();
            match std::mem::replace(&mut *state, State::Taken) {
                State::Pending { callback: Some(callback), .. } => (Some((callback, result)), None),
                State::Pending { waker, callback: None } => {
                    *state = State::Ready(result);
                    shared.ready.notify_all();
                    (None, waker)
                }
                done => {
                    *state = done;
                    (None, None)
                }
            }
        };
        if let Some((callback, result)) = callback {
            callback(result);
        }
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for Resolver<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            Self::deliver(&shared, Err(Error::cancelled("the command was dropped before it ran")));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Wake;

    use mln_util::ErrorKind;

    use super::*;

    #[test]
    fn wait_receives_a_result_from_another_thread() {
        let (completion, resolver) = Completion::new(None);
        let thread = std::thread::spawn(move || resolver.resolve(Ok(7)));
        assert_eq!(Ok(7), completion.wait());
        thread.join().unwrap();
    }

    #[test]
    fn a_callback_runs_once_with_the_result() {
        let (completion, resolver) = Completion::<u32>::new(None);
        let (tx, rx) = std::sync::mpsc::channel();
        completion.on_complete(move |result| tx.send(result).unwrap());
        assert!(rx.try_recv().is_err());
        resolver.resolve(Ok(3));
        assert_eq!(Ok(3), rx.recv().unwrap());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_callback_on_a_ready_completion_runs_at_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        Completion::ready(Ok("done")).on_complete(move |result| tx.send(result).unwrap());
        assert_eq!(Ok("done"), rx.try_recv().unwrap());
    }

    #[test]
    fn a_dropped_resolver_cancels() {
        let (completion, resolver) = Completion::<()>::new(None);
        drop(resolver);
        assert_eq!(ErrorKind::Cancelled, completion.wait().unwrap_err().kind);
    }

    #[test]
    fn waiting_on_the_resolving_thread_fails_instead_of_hanging() {
        let (completion, resolver) = Completion::<()>::new(Some(std::thread::current().id()));
        assert_eq!(ErrorKind::InvalidState, completion.wait().unwrap_err().kind);
        drop(resolver);
    }

    struct Flag(AtomicBool);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn polling_wakes_when_the_result_arrives() {
        let (mut completion, resolver) = Completion::new(None);
        let flag = Arc::new(Flag(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut completion).poll(&mut cx).is_pending());
        resolver.resolve(Ok(1));
        assert!(flag.0.load(Ordering::SeqCst));
        assert_eq!(Poll::Ready(Ok(1)), Pin::new(&mut completion).poll(&mut cx));
    }
}
