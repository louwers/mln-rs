//! The online file source over a simulated network: its transfers go to a
//! [`Transport`] of the caller's, its deadlines (retries after errors,
//! revalidations) go by a [`Clock`] of the caller's, and its core runs on
//! the caller's thread when it calls [`OnlineDriver::run`], so that its
//! limit on requests in flight, their priorities, cancellation and backoff
//! run as they do in an application, on a schedule that only the caller
//! advances. The benchmarks (`tools/bench`, with the `bench` feature) and
//! this crate's tests run it. Hidden from the documentation: it isn't API.

use std::sync::Arc;

use mln_util::queue::Waker;
use mln_util::time::Instant;

use crate::http_file_source::{HttpTransport, TransferRequest};
use crate::network_status::NetworkStatus;
use crate::online_file_source::{OnlineCore, OnlineFileSource, OnlineQueue};
use crate::resource::Resource;
use crate::response::{Reason, ResourceError};

/// The time, as the simulation keeps it.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// A transfer the online file source started, for a [`Transport`] to
/// answer. Dropping it unanswered answers nothing, as if it were cancelled.
pub struct Transfer(TransferRequest);

impl Transfer {
    /// What to transfer: its URL and kind, and the validators of the data
    /// the requester has (`prior_etag`, `prior_modified`).
    #[must_use]
    pub fn resource(&self) -> &Resource {
        self.0.resource()
    }

    /// Whether the online file source no longer waits for the transfer.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Answers with an HTTP response: `status`, the response's `headers` in
    /// order, and its `body`, decoded, which only a 200 or 206 reads.
    pub fn respond(self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
        let transfer =
            crate::http_file_source::Transfer { status: i64::from(status), headers, body: Ok(body), stale: false };
        self.0.finish(Ok(transfer));
    }

    /// Fails with a connection error: no response came.
    pub fn fail(self, message: &str) {
        self.0.finish(Err(Box::new(ResourceError::new(Reason::Connection, message))));
    }
}

/// Answers the transfers of an online file source.
pub trait Transport: Send + Sync {
    /// Takes a transfer the source started, without blocking, to answer
    /// later from any thread.
    fn start(&self, transfer: Transfer);
}

struct Adapter(Arc<dyn Transport>);

impl HttpTransport for Adapter {
    fn start(&self, request: TransferRequest) {
        self.0.start(Transfer(request));
    }
}

/// Runs the core of an [`online_file_source`] on the caller's thread.
pub struct OnlineDriver {
    queue: OnlineQueue,
    core: OnlineCore,
    clock: Clock,
}

impl OnlineDriver {
    /// Runs what the source has to do now: its requests and cancellations,
    /// the transfers' answers, and the deadlines that have passed by the
    /// source's clock, until none is left. Returns whether there was any.
    pub fn run(&mut self) -> bool {
        let now = (self.clock)();
        let mut ran = false;
        // A turn's tasks can set deadlines that have passed already, such
        // as a new request's, which the next turn runs.
        while self.queue.run_once_at(&mut self.core, now) {
            ran = true;
        }
        ran
    }

    /// Calls `waker`, from the posting thread, whenever the source has a
    /// task for [`run`](Self::run): a request, a cancellation or a
    /// transfer's answer. The clock's moves don't call it.
    pub fn set_waker(&self, waker: Option<Waker>) {
        self.queue.set_waker(waker);
    }
}

impl Drop for OnlineDriver {
    fn drop(&mut self) {
        self.queue.close();
    }
}

/// An online file source whose requests follow `network`, whose transfers
/// go to `transport`, whose deadlines go by `clock`, and whose core runs
/// only in the returned driver's [`run`](OnlineDriver::run).
#[must_use]
pub fn online_file_source(
    network: &Arc<NetworkStatus>,
    transport: Arc<dyn Transport>,
    clock: Clock,
) -> (OnlineFileSource, OnlineDriver) {
    let (source, queue, core) =
        OnlineFileSource::driven(Arc::clone(network), Arc::new(Adapter(transport)), Arc::clone(&clock));
    (source, OnlineDriver { queue, core, clock })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use parking_lot::Mutex;

    use super::*;
    use crate::file_source::FileSource;
    use crate::request::Request;
    use crate::resource::Kind;
    use crate::response::Response;

    /// Holds each transfer until the test answers it.
    #[derive(Default)]
    struct Held(Mutex<Vec<Transfer>>);

    impl Transport for Held {
        fn start(&self, transfer: Transfer) {
            self.0.lock().push(transfer);
        }
    }

    /// A transfer that fails is retried once a second has passed on the
    /// source's clock, whatever the system's says.
    #[test]
    fn retries_go_by_the_sources_clock() {
        let held = Arc::new(Held::default());
        let base = Instant::now();
        let elapsed = Arc::new(AtomicU64::new(0));
        let clock: Clock = {
            let elapsed = Arc::clone(&elapsed);
            Arc::new(move || base + Duration::from_millis(elapsed.load(Ordering::SeqCst)))
        };
        let (source, mut driver) = online_file_source(&Arc::default(), Arc::<Held>::clone(&held), clock);
        let answers: Arc<Mutex<Vec<Response>>> = Arc::default();
        let saved = Arc::clone(&answers);
        let (_request, responder) = Request::new(move |response| saved.lock().push(response));
        source.load(&Resource::new(Kind::Tile, "https://example.invalid/0/0/0"), responder);

        assert!(driver.run());
        let first = held.0.lock().pop().expect("the first transfer");
        first.fail("refused");
        driver.run();
        assert_eq!(answers.lock().len(), 1, "the error reaches the requester");
        assert!(held.0.lock().is_empty());

        elapsed.store(999, Ordering::SeqCst);
        driver.run();
        assert!(held.0.lock().is_empty(), "no retry before a second");
        elapsed.store(1000, Ordering::SeqCst);
        driver.run();
        let retry = held.0.lock().pop().expect("the retry");
        retry.respond(200, Vec::new(), b"tile".to_vec());
        driver.run();
        let answers = answers.lock();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[1].data.as_deref().map(Vec::as_slice), Some(&b"tile"[..]));
    }
}
