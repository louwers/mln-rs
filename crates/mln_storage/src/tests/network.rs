// The network of the online file source's and the loader's tests: an online
// file source over the simulated network (`crate::simulated_network`), whose
// transfers a `Server` answers on the test's thread with real status codes
// and headers, and whose clock moves only when the test moves it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mln_style::time::Instant;
use parking_lot::Mutex;

use super::TestLoop;
use crate::file_source::FileSource;
use crate::network_status::NetworkStatus;
use crate::online_file_source::OnlineFileSource;
use crate::request::Request;
use crate::resource::Resource;
use crate::response::Response;
use crate::simulated_network::{Clock, OnlineDriver, Transfer, Transport, online_file_source};

/// How a [`Server`] answers a transfer.
pub(crate) enum Reply {
    /// An HTTP response: its status, its headers in order and its body.
    Http(u16, Vec<(String, String)>, Vec<u8>),
    /// No response: the connection failed.
    Refused,
    /// Not yet: the transfer waits for the test in [`Server::held`].
    Hold,
}

impl Reply {
    /// A 200 with `body` and no headers.
    pub(crate) fn ok(body: &str) -> Reply {
        Reply::Http(200, Vec::new(), body.as_bytes().to_vec())
    }

    /// A response of `status` with no headers and no body.
    pub(crate) fn status(status: u16) -> Reply {
        Reply::Http(status, Vec::new(), Vec::new())
    }

    /// The response with the header `name: value` after its others.
    pub(crate) fn header(mut self, name: &str, value: impl Into<String>) -> Reply {
        if let Reply::Http(_, headers, _) = &mut self {
            headers.push((name.to_owned(), value.into()));
        }
        self
    }

    fn answer(self, transfer: Transfer) -> Option<Transfer> {
        match self {
            Reply::Http(status, headers, body) => transfer.respond(status, headers, body),
            Reply::Refused => transfer.fail("Connection refused"),
            Reply::Hold => return Some(transfer),
        }
        None
    }
}

type Route = Box<dyn Fn(&Resource) -> Reply + Send + Sync>;

/// Answers each transfer as its route says, at once, and keeps the
/// resource of each it was asked for.
pub(crate) struct Server {
    route: Route,
    held: Mutex<Vec<Transfer>>,
    requested: Mutex<Vec<Resource>>,
}

impl Server {
    /// Takes the transfers that wait for an answer, the first started first.
    pub(crate) fn held(&self) -> Vec<Transfer> {
        std::mem::take(&mut self.held.lock())
    }

    /// Takes the URLs of the transfers started since the last call, in the
    /// order they started.
    pub(crate) fn requested(&self) -> Vec<String> {
        self.requested.lock().drain(..).map(|resource| resource.url).collect()
    }

    /// Takes the resources of the transfers started since the last call.
    pub(crate) fn requested_resources(&self) -> Vec<Resource> {
        std::mem::take(&mut self.requested.lock())
    }
}

impl Transport for Server {
    fn start(&self, transfer: Transfer) {
        self.requested.lock().push(transfer.resource().clone());
        if let Some(held) = (self.route)(transfer.resource()).answer(transfer) {
            self.held.lock().push(held);
        }
    }
}

/// The time on a [`Network`]'s clock, which starts at the system's time
/// when the network is made and moves only with [`advance`](Self::advance).
#[derive(Clone)]
pub(crate) struct TestClock {
    base: Instant,
    elapsed: Arc<AtomicU64>,
}

impl TestClock {
    /// Moves the clock on by `by`. The network runs the deadlines that
    /// passed in its next [`run`](Network::run).
    pub(crate) fn advance(&self, by: Duration) {
        self.elapsed.fetch_add(u64::try_from(by.as_nanos()).unwrap(), Ordering::SeqCst);
    }

    fn now(&self) -> Instant {
        self.base + Duration::from_nanos(self.elapsed.load(Ordering::SeqCst))
    }
}

/// The answers to a request, in the order they came.
pub(crate) type Answers = Arc<Mutex<Vec<Response>>>;

/// An online file source over a [`Server`], following `status`, whose core
/// runs only in [`run`](Self::run) and whose deadlines go by `clock`.
pub(crate) struct Network {
    pub(crate) source: Arc<OnlineFileSource>,
    pub(crate) status: Arc<NetworkStatus>,
    pub(crate) server: Arc<Server>,
    pub(crate) clock: TestClock,
    driver: OnlineDriver,
}

impl Network {
    /// A network whose server answers by `route`.
    pub(crate) fn serving(route: impl Fn(&Resource) -> Reply + Send + Sync + 'static) -> Network {
        let server = Arc::new(Server { route: Box::new(route), held: Mutex::default(), requested: Mutex::default() });
        let clock = TestClock { base: Instant::now(), elapsed: Arc::default() };
        let time = clock.clone();
        let time: Clock = Arc::new(move || time.now());
        let status = Arc::new(NetworkStatus::new());
        let (source, driver) = online_file_source(&status, Arc::<Server>::clone(&server), time);
        Network { source: Arc::new(source), status, server, clock, driver }
    }

    /// A network whose server holds every transfer for the test to answer.
    pub(crate) fn holding() -> Network {
        Network::serving(|_| Reply::Hold)
    }

    /// Runs what the source has to do now, until nothing is left: its
    /// requests, the server's answers, and the deadlines that passed.
    pub(crate) fn run(&mut self) {
        self.driver.run();
    }

    /// Moves the clock on by `by`, then [`run`](Self::run)s.
    pub(crate) fn advance(&mut self, by: Duration) {
        self.clock.advance(by);
        self.run();
    }

    /// Requests `resource`, whose answers the returned list collects until
    /// the returned request drops.
    pub(crate) fn request(&self, resource: &Resource) -> (Request, Answers) {
        let answers = Answers::default();
        let collected = Arc::clone(&answers);
        let (request, responder) = Request::new(move |response| collected.lock().push(response));
        self.source.load(resource, responder);
        (request, answers)
    }
}

impl TestLoop {
    /// [`run`](Self::run)s the loop, and `network` after each of its turns
    /// and whenever the network's source has something to do. A run that
    /// is still going after a minute fails the test instead of hanging.
    pub(crate) fn run_with_network(&self, network: &mut Network) {
        let queue = self.handle();
        let hung = queue.post_after(Duration::from_secs(60), |_| panic!("the test loop ran for a minute"));
        let waker = queue.clone();
        network.driver.set_waker(Some(Arc::new(move || waker.wake())));
        queue.run_with(&mut self.dispatcher(), |_| network.run());
        network.driver.set_waker(None);
        queue.cancel(hung);
    }
}
