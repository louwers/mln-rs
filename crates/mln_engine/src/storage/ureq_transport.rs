//! curl through ureq: blocking transfers, each on one of the transport's
//! worker threads.
//!
//! ureq connects to a host's addresses one after the other and waits for
//! each, so a host whose first (IPv6) address drops packets stalls every new
//! connection until the OS gives up (75 s on macOS). curl races them (Happy
//! Eyeballs), so the agent's TCP step does too: see `HappyEyeballsConnector`.
//!
//! A request for a byte range asks for the identity encoding, since a range
//! of a compressed body isn't a range of the resource.
//!
//! TLS checks certificates against the system's trust store (rustls with
//! rustls-platform-verifier; `SChannel` through native-tls on Windows), as curl
//! does with the system's CA bundle. A proxy comes from the environment
//! (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, ...), as for curl.
//!
//! ureq follows redirects with every header it was given, so a request with
//! the host's headers follows them here instead, and drops those headers
//! once a redirect leaves their origin.
//!
//! A cancelled request's reads and writes stop at once: they block, so
//! the cancel shuts down the socket the transfer uses (`Interrupt`), which
//! costs that connection. A transfer left to finish would hold its worker,
//! and spend bandwidth, on a resource nobody wants. A name lookup or a TCP
//! connect under way still runs to its end.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::resolver::{DefaultResolver, Resolver};
#[cfg(target_os = "windows")]
use ureq::unversioned::transport::NativeTlsConnector;
#[cfg(not(target_os = "windows"))]
use ureq::unversioned::transport::RustlsConnector;
use ureq::unversioned::transport::{
    Buffers, ConnectProxyConnector, ConnectionDetails, Connector, Either, NextTimeout, ShutdownHandle, TcpConnector,
    Transport as UreqTransport,
};

use crate::storage::chrono::rfc1123;
use crate::storage::http_file_source::{
    HttpTransport, Transfer, TransferRequest, USER_AGENT, accept_header, range_header, same_origin,
};
use crate::storage::redirect::redirect_url;
use crate::storage::resource::Resource;
use crate::storage::response::{Reason, ResourceError};

/// curl's default `CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS`.
const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(200);

/// The transport's worker threads: as many as the online file source's
/// default maximum of concurrent requests.
const WORKERS: usize = crate::storage::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS as usize;

/// The TCP step of ureq's default connector chain with Happy Eyeballs
/// (RFC 8305): the addresses alternate between the families, starting
/// with the resolver's first, and each attempt gets a head start of
/// `HAPPY_EYEBALLS_DELAY` (or until it fails) before the next one starts
/// alongside it. The first connection wins; the others are closed when
/// they finish. Each attempt is `per_address` (ureq's own `TcpConnector`,
/// or a test's) on one address, on its own thread.
#[derive(Debug)]
struct HappyEyeballsConnector<A> {
    per_address: Arc<A>,
}

impl<In: UreqTransport, A: Connector> Connector<In> for HappyEyeballsConnector<A> {
    type Out = Either<In, Box<dyn UreqTransport>>;

    fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
        if let Some(chained) = chained {
            // A CONNECT proxy's tunnel.
            return Ok(Some(Either::A(chained)));
        }
        if details.addrs.len() < 2 {
            // Nothing to race.
            return Ok(connect_interruptible(&*self.per_address, details)?.map(Either::B));
        }

        let (first, second): (Vec<SocketAddr>, Vec<SocketAddr>) =
            details.addrs.iter().partition(|addr| addr.is_ipv6() == details.addrs[0].is_ipv6());
        let addrs: Vec<SocketAddr> = (0..first.len().max(second.len()))
            .flat_map(|i| first.get(i).into_iter().chain(second.get(i)).copied())
            .collect();

        let (sender, receiver) = mpsc::channel();
        let mut started = 0;
        let mut running = 0;
        let mut last_error = None;
        loop {
            if started < addrs.len() {
                start_attempt(&self.per_address, details, addrs[started], sender.clone())?;
                started += 1;
                running += 1;
            }
            let result = if started < addrs.len() {
                match receiver.recv_timeout(HAPPY_EYEBALLS_DELAY) {
                    Ok(result) => result,
                    Err(_) => continue, // the next attempt's turn
                }
            } else if running > 0 {
                // `sender` is still here, so this can't disconnect.
                receiver.recv().expect("happy eyeballs channel")
            } else {
                return Err(last_error.unwrap_or(ureq::Error::ConnectionFailed));
            };
            running -= 1;
            match result {
                Ok(transport) => return Ok(Some(Either::B(transport))),
                Err(error) => last_error = Some(error),
            }
        }
    }
}

/// One connection attempt of `HappyEyeballsConnector`: `per_address` on
/// `addr` with the rest of `details`, on a thread that sends the result
/// to `sender`.
fn start_attempt<A: Connector>(
    per_address: &Arc<A>,
    details: &ConnectionDetails,
    addr: SocketAddr,
    sender: mpsc::Sender<Result<Box<dyn UreqTransport>, ureq::Error>>,
) -> Result<(), ureq::Error> {
    let uri = details.uri.clone();
    let config = details.config.clone();
    let request_level = details.request_level;
    let now = details.now;
    let timeout = details.timeout;
    let current_time = Arc::clone(&details.current_time);
    let run_connector = Arc::clone(&details.run_connector);
    let register_shutdown = Arc::clone(&details.register_shutdown);
    let per_address = Arc::clone(per_address);
    crate::exec::thread::spawn("HTTPFileSource connect", move || {
        // TcpConnector doesn't resolve; a resolver is only required.
        let resolver = DefaultResolver::default();
        let mut addrs = resolver.empty();
        addrs.push(addr);
        let details = ConnectionDetails {
            uri: &uri,
            addrs,
            config: &config,
            request_level,
            resolver: &resolver,
            now,
            timeout,
            current_time,
            run_connector,
            register_shutdown,
        };
        let result = connect_interruptible(&*per_address, &details)
            .and_then(|transport| transport.ok_or(ureq::Error::ConnectionFailed));
        // After another attempt won, nobody listens: the
        // connection closes here.
        let _ = sender.send(result);
    })?;
    Ok(())
}

/// `per_address`'s connection on `details`, made [`Interruptible`] by the
/// socket it registers for the agent's shutdown.
fn connect_interruptible<A: Connector>(
    per_address: &A,
    details: &ConnectionDetails,
) -> Result<Option<Box<dyn UreqTransport>>, ureq::Error> {
    let registered: Arc<Mutex<Option<Weak<dyn ShutdownHandle>>>> = Arc::default();
    let register_shutdown = {
        let (registered, register) = (Arc::clone(&registered), Arc::clone(&details.register_shutdown));
        Arc::new(move |socket: Weak<dyn ShutdownHandle>| {
            *registered.lock() = Some(Weak::clone(&socket));
            register(socket)
        })
    };
    let mut addrs = details.resolver.empty();
    for addr in &details.addrs {
        addrs.push(*addr);
    }
    let details = ConnectionDetails {
        uri: details.uri,
        addrs,
        config: details.config,
        request_level: details.request_level,
        resolver: details.resolver,
        now: details.now,
        timeout: details.timeout,
        current_time: Arc::clone(&details.current_time),
        run_connector: Arc::clone(&details.run_connector),
        register_shutdown,
    };
    let transport = per_address.connect(&details, None)?;
    Ok(transport.map(|transport| -> Box<dyn UreqTransport> {
        match registered.lock().take() {
            Some(socket) => Box::new(Interruptible { inner: Box::new(transport), socket }),
            None => Box::new(transport),
        }
    }))
}

thread_local! {
    /// The interrupt of the transfer this worker thread runs.
    static TRANSFER: RefCell<Option<Arc<Interrupt>>> = const { RefCell::new(None) };
}

/// Stops a worker's transfer when its request is cancelled, by shutting
/// down the socket the transfer uses: reads and writes blocked on it end
/// at once, and the pool drops the connection.
#[derive(Default)]
struct Interrupt(Mutex<InterruptState>);

#[derive(Default)]
struct InterruptState {
    cancelled: bool,
    /// The socket the transfer reads or writes now. Only while it does:
    /// between them, a connection may be back in the pool, for another
    /// transfer.
    socket: Option<Weak<dyn ShutdownHandle>>,
}

fn shut_down(socket: &Weak<dyn ShutdownHandle>) {
    if let Some(socket) = socket.upgrade() {
        socket.shutdown();
    }
}

impl Interrupt {
    /// The request was cancelled: shuts down the socket the transfer reads
    /// or writes, now or when it next does.
    fn cancel(&self) {
        let mut state = self.0.lock();
        state.cancelled = true;
        if let Some(socket) = state.socket.take() {
            shut_down(&socket);
        }
    }

    /// The transfer is about to read or write `socket`.
    fn enter(&self, socket: &Weak<dyn ShutdownHandle>) {
        let mut state = self.0.lock();
        if state.cancelled {
            shut_down(socket);
        } else {
            state.socket = Some(Weak::clone(socket));
        }
    }

    /// The transfer's read or write returned.
    fn leave(&self) {
        self.0.lock().socket = None;
    }
}

/// A connection whose reads and writes tell the [`Interrupt`] of the
/// worker's transfer which socket to shut down.
#[derive(Debug)]
struct Interruptible {
    inner: Box<dyn UreqTransport>,
    socket: Weak<dyn ShutdownHandle>,
}

impl Interruptible {
    /// Runs `io` on the connection, which the worker's transfer's
    /// interrupt shuts down meanwhile.
    fn io<T>(&mut self, io: impl FnOnce(&mut dyn UreqTransport) -> T) -> T {
        let interrupt = TRANSFER.with_borrow(Clone::clone);
        if let Some(interrupt) = &interrupt {
            interrupt.enter(&self.socket);
        }
        let result = io(&mut *self.inner);
        if let Some(interrupt) = &interrupt {
            interrupt.leave();
        }
        result
    }
}

impl UreqTransport for Interruptible {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.io(|inner| inner.transmit_output(amount, timeout))
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.io(|inner| inner.await_input(timeout))
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

/// The ureq transport: requests wait in a queue for the next free one
/// of its worker threads, which runs the transfer.
///
/// Dropping it ends the workers: their transfers fail at once, except a
/// name lookup or a TCP connect, which nothing interrupts. It waits up to
/// `STOP_TIMEOUT` for them, and a worker still in one after that ends alone
/// once it returns, without finishing its request.
pub(crate) struct Transport {
    requests: Option<mpsc::Sender<TransferRequest>>,
    client: Arc<OnceLock<Client>>,
    stopping: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    /// Disconnects once every worker has returned.
    ended: Mutex<mpsc::Receiver<()>>,
}

/// How long dropping a [`Transport`] waits for its workers. Shutting the
/// agent down ends their reads, writes and TLS handshakes at once, so only
/// a name lookup or a TCP connect lasts longer, which the operating system
/// gives up on after tens of seconds or minutes: a bound that long would
/// hold up a host that drops its runtime, and a second covers a lookup or
/// connect that is merely slow.
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

impl Transport {
    /// Starts as many workers as the online file source's default
    /// maximum of concurrent requests. Their client is made with the first
    /// transfer.
    pub(crate) fn new() -> Self {
        Self::with_client(Client::new)
    }

    /// `new`, with the workers' client from `make` (tests).
    fn with_client(make: fn() -> Client) -> Self {
        let client = Arc::new(OnceLock::new());
        let stopping = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel::<TransferRequest>();
        let (running, ended) = mpsc::channel::<()>();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..WORKERS)
            .filter_map(|i| {
                let worker = Worker {
                    client: Arc::clone(&client),
                    make,
                    requests: Arc::clone(&receiver),
                    stopping: Arc::clone(&stopping),
                    _running: running.clone(),
                };
                crate::exec::thread::spawn(format!("HTTPFileSource {i}"), move || worker.run()).ok()
            })
            .collect();
        Transport { requests: Some(requests), client, stopping, workers, ended: Mutex::new(ended) }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        // Set first: a worker that checks it unset has its client made
        // already, which the shutdown below reaches.
        self.stopping.store(true, Ordering::SeqCst);
        self.requests = None;
        if let Some(client) = self.client.get() {
            client.agent.clone().shutdown();
        }
        if matches!(self.ended.get_mut().recv_timeout(STOP_TIMEOUT), Err(mpsc::RecvTimeoutError::Disconnected)) {
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

impl HttpTransport for Transport {
    fn start(&self, request: TransferRequest) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }
}

/// A worker thread of a [`Transport`].
struct Worker {
    client: Arc<OnceLock<Client>>,
    make: fn() -> Client,
    requests: Arc<Mutex<mpsc::Receiver<TransferRequest>>>,
    stopping: Arc<AtomicBool>,
    /// Tells the transport that the worker returned, when it drops.
    _running: mpsc::Sender<()>,
}

impl Worker {
    fn run(self) {
        loop {
            let request = self.requests.lock().recv();
            let Ok(request) = request else {
                return; // the transport is gone
            };
            if request.is_cancelled() {
                continue;
            }
            let client = self.client.get_or_init(self.make);
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let interrupt = Arc::new(Interrupt::default());
            let cancel = Arc::clone(&interrupt);
            request.set_cancel_handler(move || cancel.cancel());
            TRANSFER.set(Some(Arc::clone(&interrupt)));
            let transfer = client.transfer(request.resource(), request.headers());
            TRANSFER.set(None);
            request.finish(transfer);
        }
    }
}

/// One blocking transfer at a time, through ureq.
struct Client {
    agent: ureq::Agent,
}

/// The curl error codes `handleResult` maps to `Reason::Connection`.
fn is_connection_error(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) | ureq::Error::Io(_)
    )
}

fn error(error: &ureq::Error) -> ResourceError {
    // Add human-readable error code
    let reason = if is_connection_error(error) { Reason::Connection } else { Reason::Other };
    ResourceError::new(reason, error.to_string())
}

/// The system's trust store, through native-tls (`SChannel`).
#[cfg(target_os = "windows")]
fn tls_config() -> TlsConfig {
    TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build()
}

/// The system's verifier, through rustls-platform-verifier, with ring's
/// cryptography (ureq's rustls without its baked-in Mozilla roots needs the
/// provider spelled out).
#[cfg(not(target_os = "windows"))]
fn tls_config() -> TlsConfig {
    TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build()
}

impl Client {
    fn new() -> Self {
        Self::with_parts(TcpConnector::default(), DefaultResolver::default())
    }

    /// `new` with another per-address connector and resolver (tests).
    fn with_parts(per_address: impl Connector, resolver: impl Resolver) -> Self {
        // As many idle connections as workers, so that a burst of requests
        // finds the connections the last one opened.
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_idle_connections(WORKERS)
            .max_idle_connections_per_host(WORKERS)
            .user_agent(USER_AGENT)
            .tls_config(tls_config())
            .build();
        // ureq's DefaultConnector with HappyEyeballsConnector for its
        // TcpConnector, and only the TLS step this platform uses (no SOCKS,
        // whose feature is off).
        let connector =
            ().chain(ConnectProxyConnector::default())
                .chain(HappyEyeballsConnector { per_address: Arc::new(per_address) });
        #[cfg(target_os = "windows")]
        let connector = connector.chain(NativeTlsConnector::default());
        #[cfg(not(target_os = "windows"))]
        let connector = connector.chain(RustlsConnector::default());
        let agent = ureq::Agent::with_parts(config, connector, resolver);
        Client { agent }
    }

    /// Fetches `resource` with the host's `headers`.
    fn transfer(&self, resource: &Resource, headers: &[(String, String)]) -> Result<Transfer, Box<ResourceError>> {
        let mut url = resource.url.clone();
        let mut headers = headers;
        let mut redirects = 0;
        let follows_redirects = !headers.is_empty();
        loop {
            let mut http_response = self.call(resource, &url, headers, follows_redirects)?;
            let status = http_response.status().as_u16();
            if follows_redirects
                && matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(next) = http_response
                    .headers()
                    .get("location")
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| redirect_url(&url, location))
            {
                if redirects == self.agent.config().max_redirects() {
                    return Err(Box::new(error(&ureq::Error::TooManyRedirects)));
                }
                redirects += 1;
                if !same_origin(&url, &next) {
                    headers = &[];
                }
                url = next;
                continue;
            }
            let response_headers = http_response
                .headers()
                .iter()
                .filter_map(|(name, value)| Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned())))
                .collect();
            let status = i64::from(status);
            let body = if status == 200 || status == 206 {
                http_response.body_mut().with_config().limit(u64::MAX).read_to_vec().map_err(|e| Box::new(error(&e)))
            } else {
                Ok(Vec::new())
            };
            return Ok(Transfer { status, headers: response_headers, body, stale: false });
        }
    }

    /// One GET of `url` for `resource`, with the host's `headers`. ureq
    /// follows its redirects unless `transfer` `follows_redirects` itself.
    fn call(
        &self,
        resource: &Resource,
        url: &str,
        headers: &[(String, String)],
        follows_redirects: bool,
    ) -> Result<ureq::http::Response<ureq::Body>, Box<ResourceError>> {
        let mut request = self.agent.get(url);
        if let Some(range) = range_header(resource) {
            // A range of a compressed body isn't a range of the resource.
            request = request.header("Range", range).header("Accept-Encoding", "identity");
        }

        // If there's already a response, set the correct etags/modified headers to
        // make sure we are getting a 304 response if possible. This avoids
        // redownloading unchanged data.
        if let Some(prior_etag) = &resource.prior_etag {
            request = request.header("If-None-Match", prior_etag.as_str());
        } else if let Some(prior_modified) = resource.prior_modified {
            request = request.header("If-Modified-Since", rfc1123(prior_modified));
        }
        if let Some(accept) = accept_header(resource, headers) {
            request = request.header("Accept", accept);
        }

        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if follows_redirects {
            request.config().max_redirects(0).build().call().map_err(|e| Box::new(error(&e)))
        } else {
            request.call().map_err(|e| Box::new(error(&e)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use parking_lot::Mutex;
    use ureq::config::Config;
    use ureq::http::Uri;
    use ureq::unversioned::resolver::ResolvedSocketAddrs;

    use super::*;

    /// Resolves every host to `addrs`, in order.
    #[derive(Debug)]
    struct FixedResolver(Vec<SocketAddr>);

    impl Resolver for FixedResolver {
        fn resolve(
            &self,
            _uri: &Uri,
            _config: &Config,
            _timeout: NextTimeout,
        ) -> Result<ResolvedSocketAddrs, ureq::Error> {
            let mut addrs = self.empty();
            for addr in &self.0 {
                addrs.push(*addr);
            }
            Ok(addrs)
        }
    }

    /// Connects like ureq's `TcpConnector`, except that a connect to one of
    /// `dropped` hangs as for a host that drops the SYN. Records the
    /// addresses tried.
    #[derive(Debug)]
    struct DroppingConnector {
        dropped: Vec<SocketAddr>,
        attempts: Arc<Mutex<Vec<SocketAddr>>>,
    }

    impl Connector for DroppingConnector {
        type Out = Box<dyn UreqTransport>;

        fn connect(&self, details: &ConnectionDetails, _chained: Option<()>) -> Result<Option<Self::Out>, ureq::Error> {
            let addr = details.addrs[0];
            self.attempts.lock().push(addr);
            if self.dropped.contains(&addr) {
                std::thread::sleep(Duration::from_secs(60));
                return Err(ureq::Error::ConnectionFailed);
            }
            let transport = TcpConnector::default().connect(details, None::<()>)?;
            Ok(transport.map(|transport| -> Box<dyn UreqTransport> { Box::new(transport) }))
        }
    }

    /// mapeak.com resolved to an IPv6 address that drops packets, then
    /// an IPv4 one: each new connection waited for the OS connect
    /// timeout (75 s on macOS) before trying the IPv4 address.
    #[test]
    fn falls_back_from_an_address_that_drops_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
        });

        let dropped: Vec<SocketAddr> = vec!["[2001:db8::1]:80".parse().unwrap(), "[2001:db8::2]:80".parse().unwrap()];
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let client = Client::with_parts(
            DroppingConnector { dropped: dropped.clone(), attempts: Arc::clone(&attempts) },
            FixedResolver(vec![dropped[0], dropped[1], server]),
        );

        let started = Instant::now();
        let transfer = client.transfer(&Resource::style("http://mapeak.test/style.json"), &[]).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert_eq!(200, transfer.status);
        assert_eq!(b"ok".to_vec(), transfer.body.unwrap());
        // The families alternate: the IPv4 address goes second, after
        // HAPPY_EYEBALLS_DELAY.
        assert_eq!([dropped[0], server], attempts.lock()[..2]);
    }

    /// Answers each connection of `listener` with a redirect to `/next` once,
    /// then a 200, and sends the `User-Agent` of every request down `agents`.
    #[test]
    fn sends_the_user_agent_on_every_request_of_a_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (agents, received) = mpsc::channel();
        std::thread::spawn(move || {
            for response in [
                // Each response closes its connection, as the server drops
                // it, so the client doesn't send the next request down it.
                "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let agent = request
                    .lines()
                    .find_map(|line| line.to_ascii_lowercase().strip_prefix("user-agent: ").map(str::to_owned));
                agents.send(agent).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let transfer = Client::new()
            .transfer(&Resource::style(&format!("http://{addr}/style.json")), &[("Accept".into(), "*/*".into())])
            .unwrap();
        assert_eq!(200, transfer.status);
        let expected = Some(USER_AGENT.to_ascii_lowercase());
        assert_eq!(expected, received.recv().unwrap());
        assert_eq!(expected, received.recv().unwrap());
    }

    /// Takes one connection on a loopback port and never answers it.
    /// Returns the port and what tells of the connection.
    fn never_answer() -> (SocketAddr, mpsc::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (accepted, was_accepted) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted.send(()).unwrap();
            let mut buffer = [0; 1024];
            while stream.read(&mut buffer).is_ok_and(|read| read != 0) {}
        });
        (addr, was_accepted)
    }

    #[test]
    fn dropping_the_transport_ends_a_transfer_that_waits_for_its_response() {
        let (addr, accepted) = never_answer();
        let transport = Transport::new();
        let client = Arc::downgrade(&transport.client);
        let (transfer, _request) = TransferRequest::unanswered(Resource::style(&format!("http://{addr}/style.json")));
        transport.start(transfer);
        accepted.recv_timeout(Duration::from_secs(10)).unwrap();
        let started = Instant::now();
        drop(transport);
        assert!(started.elapsed() < STOP_TIMEOUT, "took {:?}", started.elapsed());
        assert!(client.upgrade().is_none(), "a worker still runs");
    }

    #[test]
    fn dropping_the_transport_waits_for_a_connect_until_its_timeout() {
        const HANGING: SocketAddr =
            SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)), 80);
        static ATTEMPTS: std::sync::LazyLock<Arc<Mutex<Vec<SocketAddr>>>> = std::sync::LazyLock::new(Arc::default);
        let transport = Transport::with_client(|| {
            Client::with_parts(
                DroppingConnector { dropped: vec![HANGING], attempts: Arc::clone(&ATTEMPTS) },
                FixedResolver(vec![HANGING]),
            )
        });
        let (transfer, _request) = TransferRequest::unanswered(Resource::style("http://hanging.test/style.json"));
        transport.start(transfer);
        while ATTEMPTS.lock().is_empty() {
            std::thread::yield_now();
        }
        let started = Instant::now();
        drop(transport);
        let waited = started.elapsed();
        assert!(waited >= STOP_TIMEOUT && waited < STOP_TIMEOUT * 5, "took {waited:?}");
    }

    /// Answers one request on a loopback port with 200 and returns the
    /// port and the request's head.
    fn serve_one() -> (SocketAddr, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                assert_ne!(read, 0, "the request ended early");
                request.extend_from_slice(&buffer[..read]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });
        (addr, server)
    }

    /// Reads a request's head off `stream`. False once the client closed it.
    fn read_head(stream: &mut TcpStream) -> bool {
        let mut head = Vec::new();
        let mut byte = [0];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return false,
            }
        }
        true
    }

    #[test]
    fn cancelling_a_request_ends_its_transfer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (sent, was_sent) = mpsc::channel();
        let (closed, was_closed) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(read_head(&mut stream));
            // A body that stalls after its first bytes.
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\npartial").unwrap();
            sent.send(()).unwrap();
            let mut buffer = [0; 1024];
            while stream.read(&mut buffer).is_ok_and(|read| read != 0) {}
            closed.send(()).unwrap();
        });

        let transport = Transport::new();
        let (transfer, request) = TransferRequest::unanswered(Resource::style(&format!("http://{addr}/style.json")));
        transport.start(transfer);
        was_sent.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(request);
        was_closed.recv_timeout(Duration::from_secs(5)).expect("the cancelled transfer goes on");
    }

    #[test]
    fn a_burst_of_requests_reuses_the_connections_of_the_one_before() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        // A burst's requests all arrive before any is answered, so that
        // each burst holds a connection per worker.
        let burst = Arc::new(Barrier::new(WORKERS));
        let counted = Arc::clone(&accepted);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                counted.fetch_add(1, Ordering::SeqCst);
                let burst = Arc::clone(&burst);
                std::thread::spawn(move || {
                    while read_head(&mut stream) {
                        burst.wait();
                        if stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").is_err() {
                            return;
                        }
                    }
                });
            }
        });

        let transport = Transport::new();
        for _ in 0..2 {
            let (answered, answers) = mpsc::channel();
            let requests: Vec<_> = (0..WORKERS)
                .map(|i| {
                    let answered = answered.clone();
                    let resource = Resource::style(&format!("http://{addr}/{i}.json"));
                    let (transfer, request) = TransferRequest::answered(resource, move |_| {
                        let _ = answered.send(());
                    });
                    transport.start(transfer);
                    request
                })
                .collect();
            for _ in 0..WORKERS {
                answers.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            drop(requests);
        }
        assert_eq!(WORKERS, accepted.load(Ordering::SeqCst));
    }

    #[test]
    fn a_range_asks_for_the_identity_encoding() {
        let (addr, server) = serve_one();
        let mut resource = Resource::source(&format!("http://{addr}/archive.pmtiles"));
        resource.data_range = Some((0, 126));
        Client::new().transfer(&resource, &[]).unwrap();
        let request = server.join().unwrap();
        assert!(request.contains("\r\nrange: bytes=0-126\r\n"), "{request}");
        assert!(request.contains("\r\naccept-encoding: identity\r\n"), "{request}");
        assert_eq!(request.matches("accept-encoding").count(), 1, "{request}");

        // A whole resource may come compressed.
        let (addr, server) = serve_one();
        Client::new().transfer(&Resource::style(&format!("http://{addr}/style.json")), &[]).unwrap();
        let request = server.join().unwrap();
        assert!(request.contains("\r\naccept-encoding: gzip\r\n"), "{request}");
    }
}
