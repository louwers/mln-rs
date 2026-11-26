// The network the app loads over: as it is, offline, or slowed to a mobile
// link like browsers' developer tools' throttling presets. Each is a
// resource provider of the runtime's, and choosing one replaces the last.

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use mln_engine::storage::network_status::NetworkStatus;
use mln_engine::storage::response::Reason;
use mln_engine::storage::{FileSource, OnlineFileSource, Request, ResourceError, Response};
use mln_runtime::{Completion, ResourceRequest, Runtime};
use mln_style::error::Error;
use parking_lot::Mutex;

/// The bytes of a response's status line and headers.
const HEADER_BYTES: u64 = 300;

/// A network to load over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Network {
    #[default]
    Online,
    Fast4g,
    Slow4g,
    ThreeG,
    /// Every request fails; the cache still answers those it can.
    Offline,
}

impl Network {
    pub(crate) const ALL: [Network; 5] =
        [Network::Online, Network::Fast4g, Network::Slow4g, Network::ThreeG, Network::Offline];

    /// The network that the command line names: `online`, `fast-4g`,
    /// `slow-4g`, `3g` or `offline`.
    pub(crate) fn parse(name: &str) -> Result<Network, String> {
        match name {
            "online" => Ok(Network::Online),
            "fast-4g" => Ok(Network::Fast4g),
            "slow-4g" => Ok(Network::Slow4g),
            "3g" => Ok(Network::ThreeG),
            "offline" => Ok(Network::Offline),
            _ => Err(format!("Unknown network '{name}' (online, fast-4g, slow-4g, 3g or offline)")),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Network::Online => "No throttling",
            Network::Fast4g => "Fast 4G: 9 Mbps, 170 ms",
            Network::Slow4g => "Slow 4G: 1.6 Mbps, 300 ms",
            Network::ThreeG => "3G: 500 kbps, 400 ms",
            Network::Offline => "Offline",
        }
    }

    /// The link a throttled network loads over.
    const fn link(self) -> Option<Link> {
        match self {
            Network::Fast4g => Some(Link::new(170, 9_000)),
            Network::Slow4g => Some(Link::new(300, 1_600)),
            Network::ThreeG => Some(Link::new(400, 500)),
            Network::Online | Network::Offline => None,
        }
    }

    /// Makes `runtime` load over this network.
    pub(crate) fn apply(self, runtime: &Runtime) -> Result<Completion<()>, Error> {
        if self == Network::Offline {
            return runtime.set_resource_provider(|request: ResourceRequest| {
                let mut response = Response::new();
                response.error = Some(Box::new(ResourceError::new(Reason::Connection, "The application is offline")));
                let _ = request.respond(response);
            });
        }
        match self.link().map(Throttle::new) {
            Some(Ok(throttle)) => runtime.set_resource_provider(move |request| throttle.load(request)),
            Some(Err(e)) => Err(e),
            None => runtime.clear_resource_provider(),
        }
    }
}

/// A link's round trip, from a request to the first byte of its answer, and
/// its bandwidth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Link {
    rtt: Duration,
    bytes_per_second: u64,
}

impl Link {
    const fn new(rtt_ms: u64, kbps: u64) -> Link {
        Link { rtt: Duration::from_millis(rtt_ms), bytes_per_second: kbps * 1_000 / 8 }
    }

    /// When the last byte of an answer of `bytes` arrives, for a request
    /// sent at `sent`, if the link is busy with earlier answers until
    /// `busy_until`.
    fn arrival(&self, sent: Instant, busy_until: Instant, bytes: u64) -> Instant {
        let transfer = Duration::from_secs_f64(bytes as f64 / self.bytes_per_second as f64);
        busy_until.max(sent + self.rtt) + transfer
    }
}

/// Loads requests over the network with an online file source of its own
/// (without the runtime's transforms of URLs and headers, which the app sets
/// none of), and answers each once it would have come over the link.
///
/// The link carries one answer at a time, in the order they come: the
/// answers in flight together take as long as if they shared it. A request
/// cancelled while its answer waits still takes its turn.
struct Throttle(Arc<Inner>);

struct Inner {
    link: Link,
    online: OnlineFileSource,
    busy_until: Mutex<Instant>,
    /// To the thread that answers each request at its time, in order. The
    /// thread ends once the throttle and the requests in flight are gone.
    answers: Sender<(Instant, ResourceRequest, Response)>,
}

impl Throttle {
    fn new(link: Link) -> Result<Throttle, Error> {
        let (answers, queue) = mpsc::channel::<(Instant, ResourceRequest, Response)>();
        mln_engine::exec::thread::spawn("network-throttle", move || {
            for (at, request, response) in queue {
                std::thread::sleep(at.saturating_duration_since(Instant::now()));
                let _ = request.respond(response);
            }
        })
        .map_err(|e| Error::native(format!("Failed to start the network throttle: {e}")))?;
        Ok(Throttle(Arc::new(Inner {
            link,
            online: OnlineFileSource::new(Arc::new(NetworkStatus::new())),
            busy_until: Mutex::new(Instant::now()),
            answers,
        })))
    }

    fn load(&self, request: ResourceRequest) {
        // The transfer stops when the map cancels the request.
        let transfer: Arc<Mutex<Option<Request>>> = Arc::default();
        let cancel = Arc::clone(&transfer);
        if request.on_cancel(move || drop(cancel.lock().take())).is_err() {
            return;
        }
        let resource = request.resource().clone();
        let inner = Arc::clone(&self.0);
        let sent = Instant::now();
        let (upstream, responder) = Request::once(move |response: Response| {
            let body = if response.not_modified { 0 } else { response.data.as_ref().map_or(0, |data| data.len()) };
            let at = {
                let mut busy_until = inner.busy_until.lock();
                *busy_until = inner.link.arrival(sent, *busy_until, HEADER_BYTES + body as u64);
                *busy_until
            };
            let _ = inner.answers.send((at, request, response));
        });
        *transfer.lock() = Some(upstream);
        self.0.online.load(&resource, responder);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Link, Network};

    #[test]
    fn an_answer_takes_a_round_trip_and_its_bytes() {
        let link = Link::new(100, 8); // 1,000 bytes per second
        let sent = Instant::now();
        assert_eq!(link.arrival(sent, sent, 500), sent + Duration::from_millis(600));
    }

    #[test]
    fn answers_in_flight_together_share_the_link() {
        let link = Link::new(100, 8);
        let sent = Instant::now();
        let first = link.arrival(sent, sent, 500);
        assert_eq!(link.arrival(sent, first, 500), sent + Duration::from_millis(1_100));
        // A request sent after the link is free again waits only for its
        // round trip.
        let later = first + Duration::from_secs(1);
        assert_eq!(link.arrival(later, first, 500), later + Duration::from_millis(600));
    }

    #[test]
    fn the_command_line_names_each_network() {
        for (name, network) in ["online", "fast-4g", "slow-4g", "3g", "offline"].into_iter().zip(Network::ALL) {
            assert_eq!(Network::parse(name), Ok(network));
        }
        assert!(Network::parse("5g").is_err());
    }
}
