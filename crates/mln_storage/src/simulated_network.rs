//! The online file source over a simulated network: its transfers go to a
//! [`Transport`] of the caller's, its deadlines (retries after errors,
//! revalidations) go by a [`Clock`] of the caller's, and its core runs on
//! the caller's thread when it calls [`OnlineDriver::run`], so that its
//! limit on requests in flight, their priorities, cancellation and backoff
//! run as they do in an application, on a schedule that only the caller
//! advances. The benchmarks (`tools/bench`, with the `bench` feature) and
//! this crate's tests run it. Hidden from the documentation: it isn't API.

use std::sync::Arc;

use mln_style::time::Instant;

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
    #[cfg(test)]
    pub(crate) fn set_waker(&self, waker: Option<mln_util::queue::Waker>) {
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
