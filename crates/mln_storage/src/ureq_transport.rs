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

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use mln_util::chrono::rfc1123;
use parking_lot::Mutex;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::resolver::{DefaultResolver, Resolver};
#[cfg(target_os = "windows")]
use ureq::unversioned::transport::NativeTlsConnector;
#[cfg(not(target_os = "windows"))]
use ureq::unversioned::transport::RustlsConnector;
use ureq::unversioned::transport::{
    ConnectProxyConnector, ConnectionDetails, Connector, Either, TcpConnector, Transport as UreqTransport,
};

use crate::http_file_source::{HttpTransport, Transfer, TransferRequest, range_header, same_origin};
use crate::redirect::redirect_url;
use crate::resource::Resource;
use crate::response::{Reason, ResourceError};

/// curl's default `CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS`.
const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(200);

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
            let transport = self.per_address.connect(details, None)?;
            return Ok(transport.map(|transport| -> Box<dyn UreqTransport> { Box::new(transport) }).map(Either::B));
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
    mln_util::thread::builder("HTTPFileSource connect").spawn(move || {
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
        let result = per_address
            .connect(&details, None)
            .and_then(|transport| transport.ok_or(ureq::Error::ConnectionFailed))
            .map(|transport| -> Box<dyn UreqTransport> { Box::new(transport) });
        // After another attempt won, nobody listens: the
        // connection closes here.
        let _ = sender.send(result);
    })?;
    Ok(())
}

/// The ureq transport: requests wait in a queue for the next free one
/// of its worker threads, which runs the transfer. The workers exit when
/// the transport is dropped, each after its current transfer.
pub(crate) struct Transport {
    requests: mpsc::Sender<TransferRequest>,
}

impl Transport {
    /// Starts as many workers as the online file source's default
    /// maximum of concurrent requests. Their client is made with the first
    /// transfer.
    pub(crate) fn new() -> Self {
        let client = Arc::new(OnceLock::new());
        let (requests, receiver) = mpsc::channel::<TransferRequest>();
        let receiver = Arc::new(Mutex::new(receiver));
        for i in 0..mln_util::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS {
            let client = Arc::clone(&client);
            let receiver = Arc::clone(&receiver);
            // Detached: a worker exits when the channel closes.
            let _ = mln_util::thread::builder(format!("HTTPFileSource {i}")).spawn(move || worker(&client, &receiver));
        }
        Transport { requests }
    }
}

impl HttpTransport for Transport {
    fn start(&self, request: TransferRequest) {
        let _ = self.requests.send(request);
    }
}

fn worker(client: &OnceLock<Client>, requests: &Mutex<mpsc::Receiver<TransferRequest>>) {
    loop {
        let request = requests.lock().recv();
        let Ok(request) = request else {
            return; // the transport is gone
        };
        if request.is_cancelled() {
            continue;
        }
        let transfer = client.get_or_init(Client::new).transfer(request.resource(), request.headers());
        request.finish(transfer);
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

/// The system's trust store, through native-tls (SChannel).
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
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent("MapLibreNative/1.0")
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
            return Ok(Transfer { status, headers: response_headers, body });
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
    use std::net::TcpListener;
    use std::time::Instant;

    use parking_lot::Mutex;
    use ureq::config::Config;
    use ureq::http::Uri;
    use ureq::unversioned::resolver::ResolvedSocketAddrs;
    use ureq::unversioned::transport::NextTimeout;

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

    /// Answers one request on a loopback port with 200 and returns the
    /// port and the request's head.
    fn serve_one() -> (SocketAddr, std::thread::JoinHandle<String>) {
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
