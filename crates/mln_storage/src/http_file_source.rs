// Ports src/mln/storage/http_file_source.hpp and
// platform/default/src/mln/storage/http_file_source.cpp (curl -> ureq).
//
// C++ drives a curl multi handle from the requesting thread's RunLoop (socket
// watches and a timer). ureq is blocking, so each request runs on one of the
// file source's worker threads, and the result goes back to the requesting
// thread through a mailbox, where `handleResult` runs the callback as in
// C++. Dropping the request cancels it: the mailbox closes, so the callback
// never runs (an in-flight transfer finishes on its worker and is dropped).
//
// Request headers as in C++: `Range` for data ranges, `If-None-Match` from
// the prior etag, else `If-Modified-Since` from the prior modification date,
// user agent "MapLibreNative/1.0", redirects followed. Response headers are
// read in order with curl's header callback rules (case-insensitive names;
// `Last-Modified`/`Expires` through vendor parsedate, `Cache-Control`,
// `ETag`, `Retry-After`, `X-Rate-Limit-Reset`). Status codes map to the same
// Response fields and errors.
//
// The transfer itself is an `HttpTransport`: ureq by default, or another HTTP
// stack a host sets with `set_transport` (mln-rs-web sets the browser's
// fetch). Where ureq doesn't build (Emscripten), there is no default.
//
// Differences: ureq sends `Accept-Encoding: gzip` and
// decodes gzip (curl: "gzip, deflate"); a connection failure's message is
// ureq's text instead of curl's `curl_easy_strerror: errorbuffer`; dates use
// the vendored parsedate instead of libcurl's `curl_getdate` (they differ
// only after 2037).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_util::chrono::{Timestamp, parse_timestamp_str};
use mln_util::client_options::ClientOptions;
use mln_util::http_header::{CacheControl, parse_retry_headers};
use mln_util::string::to_string_i64;
use mln_util::sync::MutexExt;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::resource::{Kind, Resource};
use crate::resource_options::ResourceOptions;
use crate::response::{Error, Reason, Response};

/// The part of `HTTPRequest` that lives on the requesting thread.
pub struct HTTPRequestState {
    callback: RefCell<Callback>,
}

impl HTTPRequestState {
    /// The callback half of `handleResult(CURLcode)`.
    fn handle_result(&self, response: Response) {
        // Calling `callback` may result in deleting `this`; the mailbox keeps
        // the state alive while this message runs.
        (self.callback.borrow_mut())(response);
    }
}

/// `HTTPRequest`
struct HTTPRequest {
    mailbox: Arc<Mailbox>,
    cancelled: Arc<AtomicBool>,
}

impl AsyncRequest for HTTPRequest {}

impl Drop for HTTPRequest {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.mailbox.close();
    }
}

struct Job {
    resource: Resource,
    reply: ActorRef<HTTPRequestState>,
    cancelled: Arc<AtomicBool>,
}

/// `HTTPFileSource::Impl`
struct Impl {
    jobs: Mutex<mpsc::Sender<Job>>,
    resource_options: Mutex<ResourceOptions>,
    client_options: Mutex<ClientOptions>,
}

/// `headerMatches(header, buffer, length)`: ASCII-case-insensitive prefix.
fn header_value<'a>(name: &str, header: &str, line: &'a str) -> Option<&'a str> {
    (name.eq_ignore_ascii_case(header)).then_some(line)
}

/// `curl_getdate` (vendor parsedate, -1 when unparsable).
fn getdate(value: &str) -> Timestamp {
    parse_timestamp_str(value)
}

/// What a transport hands back: the status code, the response headers in
/// order, and the body, which only a 200 or 206 reads.
pub struct Transfer {
    pub status: i64,
    pub headers: Vec<(String, String)>,
    pub body: Result<Vec<u8>, Box<Error>>,
}

/// How an `HTTPFileSource` performs a request: one blocking transfer, on one
/// of the file source's worker threads. A transfer that fails before a
/// response (a connection error) is an `Err`.
pub trait HttpTransport: Send + Sync {
    fn transfer(&self, resource: &Resource) -> Result<Transfer, Box<Error>>;

    /// How many transfers run at a time: one worker thread each. By default
    /// as many as the online file source's default maximum of concurrent
    /// requests.
    fn workers(&self) -> u32 {
        mln_util::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS
    }
}

static TRANSPORT: Mutex<Option<Arc<dyn HttpTransport>>> = Mutex::new(None);

/// Sets the transport of the `HTTPFileSource`s created from now on, instead of
/// the default (ureq): for a host with its own HTTP stack, such as a
/// browser's fetch.
pub fn set_transport(transport: Arc<dyn HttpTransport>) {
    *TRANSPORT.lock_unpoisoned() = Some(transport);
}

/// The transport `set_transport` set, else ureq.
fn current_transport() -> Arc<dyn HttpTransport> {
    if let Some(transport) = TRANSPORT.lock_unpoisoned().clone() {
        return transport;
    }
    default_transport()
}

#[cfg(not(target_os = "emscripten"))]
fn default_transport() -> Arc<dyn HttpTransport> {
    Arc::new(transport::Transport::new())
}

/// ureq doesn't build here, so requests fail until a host sets a transport.
#[cfg(target_os = "emscripten")]
fn default_transport() -> Arc<dyn HttpTransport> {
    struct NoTransport;
    impl HttpTransport for NoTransport {
        fn transfer(&self, resource: &Resource) -> Result<Transfer, Box<Error>> {
            Err(Box::new(Error::new(
                Reason::Connection,
                format!("no HTTP transport for {} (see http_file_source::set_transport)", resource.url),
            )))
        }
    }
    Arc::new(NoTransport)
}

/// The worker half of `HTTPRequest`: performs the transfer and builds the
/// response as the header callback and `handleResult(CURLcode)` do.
fn perform(transport: &dyn HttpTransport, resource: &Resource) -> Response {
    // Make sure a response object exists in case we haven't got any headers or content.
    let mut response = Response::new();
    let transfer = match transport.transfer(resource) {
        Ok(transfer) => transfer,
        Err(error) => {
            response.error = Some(error);
            return response;
        }
    };

    let mut retry_after: Option<String> = None;
    let mut x_rate_limit_reset: Option<String> = None;
    for (name, value) in &transfer.headers {
        let (name, value) = (name.as_str(), value.as_str());
        if let Some(value) = header_value(name, "last-modified", value) {
            // Always overwrite the modification date; We might already have
            // a value here from the Date header, but this one is more accurate.
            response.modified = Some(getdate(value));
        } else if let Some(value) = header_value(name, "etag", value) {
            response.etag = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "cache-control", value) {
            let cc = CacheControl::parse(value);
            response.expires = cc.to_time_point();
            response.must_revalidate = cc.must_revalidate;
        } else if let Some(value) = header_value(name, "expires", value) {
            response.expires = Some(getdate(value));
        } else if let Some(value) = header_value(name, "retry-after", value) {
            retry_after = Some(value.to_owned());
        } else if let Some(value) = header_value(name, "x-rate-limit-reset", value) {
            x_rate_limit_reset = Some(value.to_owned());
        }
    }

    let response_code = transfer.status;
    if response_code == 200 || response_code == 206 {
        match transfer.body {
            Ok(data) => response.data = Some(Arc::new(data)),
            // curl reports a failed transfer as an error code, not a status.
            Err(error) => response.error = Some(error),
        }
    } else if response_code == 204 || (response_code == 404 && resource.kind == Kind::Tile) {
        response.no_content = true;
    } else if response_code == 304 {
        response.not_modified = true;
    } else if response_code == 404 {
        response.error = Some(Box::new(Error::new(Reason::NotFound, "HTTP status code 404")));
    } else if response_code == 429 {
        response.error = Some(Box::new(Error::with_retry_after(
            Reason::RateLimit,
            "HTTP status code 429",
            parse_retry_headers(&retry_after, &x_rate_limit_reset),
        )));
    } else if (500..600).contains(&response_code) {
        response.error =
            Some(Box::new(Error::new(Reason::Server, format!("HTTP status code {}", to_string_i64(response_code)))));
    } else {
        response.error =
            Some(Box::new(Error::new(Reason::Other, format!("HTTP status code {}", to_string_i64(response_code)))));
    }
    response
}

/// The `Range` request header for a resource's data range, for transports.
pub fn range_header(resource: &Resource) -> Option<String> {
    resource.data_range.map(|(first, second)| format!("bytes={first}-{second}"))
}

/// curl through ureq: blocking transfers on the worker threads.
///
/// ureq connects to a host's addresses one after the other and waits for
/// each, so a host whose first (IPv6) address drops packets stalls every new
/// connection until the OS gives up (75 s on macOS). curl races them (Happy
/// Eyeballs), so the agent's TCP step does too: see `HappyEyeballsConnector`.
#[cfg(not(target_os = "emscripten"))]
mod transport {
    use std::net::SocketAddr;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use mln_util::chrono::rfc1123;
    use ureq::unversioned::resolver::{DefaultResolver, Resolver};
    use ureq::unversioned::transport::{
        ConnectProxyConnector, ConnectionDetails, Connector, Either, RustlsConnector, TcpConnector,
        Transport as UreqTransport,
    };

    use super::{Transfer, range_header};
    use crate::resource::Resource;
    use crate::response::{Error, Reason};

    /// curl's default `CURLOPT_HAPPY_EYEBALLS_TIMEOUT_MS`.
    const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(200);

    /// The TCP step of ureq's default connector chain with Happy Eyeballs
    /// (RFC 8305): the addresses alternate between the families, starting
    /// with the resolver's first, and each attempt gets a head start of
    /// `HAPPY_EYEBALLS_DELAY` (or until it fails) before the next one starts
    /// alongside it. The first connection wins; the others are closed when
    /// they finish. Each attempt is `per_address` (ureq's own TcpConnector,
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
                return Ok(transport.map(|transport| Either::B(Box::new(transport) as Box<dyn UreqTransport>)));
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
        let current_time = details.current_time.clone();
        let run_connector = details.run_connector.clone();
        let register_shutdown = details.register_shutdown.clone();
        let per_address = per_address.clone();
        std::thread::Builder::new().name("HTTPFileSource connect".to_owned()).spawn(move || {
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
                .map(|transport| Box::new(transport) as Box<dyn UreqTransport>);
            // After another attempt won, nobody listens: the
            // connection closes here.
            let _ = sender.send(result);
        })?;
        Ok(())
    }

    #[derive(Clone)]
    pub struct Transport {
        agent: ureq::Agent,
    }

    /// The curl error codes `handleResult` maps to `Reason::Connection`.
    fn is_connection_error(error: &ureq::Error) -> bool {
        matches!(
            error,
            ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) | ureq::Error::Io(_)
        )
    }

    fn error(error: ureq::Error) -> Box<Error> {
        // Add human-readable error code
        let reason = if is_connection_error(&error) { Reason::Connection } else { Reason::Other };
        Box::new(Error::new(reason, error.to_string()))
    }

    impl super::HttpTransport for Transport {
        fn transfer(&self, resource: &Resource) -> Result<Transfer, Box<Error>> {
            Transport::transfer(self, resource)
        }
    }

    impl Transport {
        pub fn new() -> Self {
            Self::with_parts(TcpConnector::default(), DefaultResolver::default())
        }

        /// `new` with another per-address connector and resolver (tests).
        fn with_parts(per_address: impl Connector, resolver: impl Resolver) -> Self {
            let config =
                ureq::Agent::config_builder().http_status_as_error(false).user_agent("MapLibreNative/1.0").build();
            // ureq's DefaultConnector with HappyEyeballsConnector for its
            // TcpConnector (and without the SOCKS and native-tls steps,
            // whose features are off).
            let connector =
                ().chain(ConnectProxyConnector::default())
                    .chain(HappyEyeballsConnector { per_address: Arc::new(per_address) })
                    .chain(RustlsConnector::default());
            let agent = ureq::Agent::with_parts(config, connector, resolver);
            Transport { agent }
        }

        pub fn transfer(&self, resource: &Resource) -> Result<Transfer, Box<Error>> {
            let mut request = self.agent.get(&resource.url);
            if let Some(range) = range_header(resource) {
                request = request.header("Range", range);
            }

            // If there's already a response, set the correct etags/modified headers to
            // make sure we are getting a 304 response if possible. This avoids
            // redownloading unchanged data.
            if let Some(prior_etag) = &resource.prior_etag {
                request = request.header("If-None-Match", prior_etag.as_str());
            } else if let Some(prior_modified) = resource.prior_modified {
                request = request.header("If-Modified-Since", rfc1123(prior_modified));
            }

            let mut http_response = request.call().map_err(error)?;
            let headers = http_response
                .headers()
                .iter()
                .filter_map(|(name, value)| Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned())))
                .collect();
            let status = i64::from(http_response.status().as_u16());
            let body = if status == 200 || status == 206 {
                http_response.body_mut().with_config().limit(u64::MAX).read_to_vec().map_err(error)
            } else {
                Ok(Vec::new())
            };
            Ok(Transfer { status, headers, body })
        }
    }

    #[cfg(test)]
    mod tests {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Mutex;
        use std::time::Instant;

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

        /// TcpConnector, except that a connect to one of `dropped` hangs as
        /// for a host that drops the SYN. Records the addresses tried.
        #[derive(Debug)]
        struct DroppingConnector {
            dropped: Vec<SocketAddr>,
            attempts: Arc<Mutex<Vec<SocketAddr>>>,
        }

        impl Connector for DroppingConnector {
            type Out = Box<dyn UreqTransport>;

            fn connect(
                &self,
                details: &ConnectionDetails,
                _chained: Option<()>,
            ) -> Result<Option<Self::Out>, ureq::Error> {
                let addr = details.addrs[0];
                self.attempts.lock().unwrap().push(addr);
                if self.dropped.contains(&addr) {
                    std::thread::sleep(Duration::from_secs(60));
                    return Err(ureq::Error::ConnectionFailed);
                }
                let transport = TcpConnector::default().connect(details, None::<()>)?;
                Ok(transport.map(|transport| Box::new(transport) as Box<dyn UreqTransport>))
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

            let dropped: Vec<SocketAddr> =
                vec!["[2001:db8::1]:80".parse().unwrap(), "[2001:db8::2]:80".parse().unwrap()];
            let attempts = Arc::new(Mutex::new(Vec::new()));
            let transport = Transport::with_parts(
                DroppingConnector { dropped: dropped.clone(), attempts: attempts.clone() },
                FixedResolver(vec![dropped[0], dropped[1], server]),
            );

            let started = Instant::now();
            let transfer = transport.transfer(&Resource::style("http://mapeak.test/style.json")).unwrap();
            assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
            assert_eq!(200, transfer.status);
            assert_eq!(b"ok".to_vec(), transfer.body.unwrap());
            // The families alternate: the IPv4 address goes second, after
            // HAPPY_EYEBALLS_DELAY.
            assert_eq!([dropped[0], server], attempts.lock().unwrap()[..2]);
        }
    }
}

fn worker(transport: Arc<dyn HttpTransport>, jobs: Arc<Mutex<mpsc::Receiver<Job>>>) {
    loop {
        let job = match jobs.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else {
            return; // the file source is gone
        };
        if job.cancelled.load(Ordering::SeqCst) {
            continue;
        }
        let response = perform(transport.as_ref(), &job.resource);
        if !job.cancelled.load(Ordering::SeqCst) {
            job.reply.invoke(move |request: &HTTPRequestState| request.handle_result(response));
        }
    }
}

/// `HTTPFileSource`: plain HTTP(S) requests, used by the OnlineFileSource.
pub struct HTTPFileSource {
    impl_: Impl,
}

impl HTTPFileSource {
    /// `HTTPFileSource(const ResourceOptions&, const ClientOptions&)`: starts
    /// the transfer threads (`HttpTransport::workers`).
    pub fn new(resource_options: &ResourceOptions, client_options: &ClientOptions) -> Self {
        let transport = current_transport();
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for i in 0..transport.workers() {
            let transport = transport.clone();
            let receiver = receiver.clone();
            // Detached: a worker exits when the channel closes, after its
            // current transfer.
            let _ = std::thread::Builder::new()
                .name(format!("HTTPFileSource {i}"))
                .spawn(move || worker(transport, receiver));
        }
        HTTPFileSource {
            impl_: Impl {
                jobs: Mutex::new(sender),
                resource_options: Mutex::new(resource_options.clone()),
                client_options: Mutex::new(client_options.clone()),
            },
        }
    }

    /// `request(const Resource&, Callback)`. The callback runs on the
    /// calling thread's RunLoop.
    pub fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let state = Rc::new(HTTPRequestState { callback: RefCell::new(callback) });
        let mailbox = Mailbox::with_target(&scheduler::get_current(), state as Rc<dyn Any>);
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job { resource: resource.clone(), reply: ActorRef::new(&mailbox), cancelled: cancelled.clone() };
        let _ = self.impl_.jobs.lock_unpoisoned().send(job);
        Box::new(HTTPRequest { mailbox, cancelled })
    }
}

impl FileSource for HTTPFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        HTTPFileSource::request(self, resource, callback)
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.impl_.resource_options.lock_unpoisoned() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.impl_.resource_options.lock_unpoisoned().clone()
    }

    fn set_client_options(&self, options: ClientOptions) {
        *self.impl_.client_options.lock_unpoisoned() = options;
    }

    fn get_client_options(&self) -> ClientOptions {
        self.impl_.client_options.lock_unpoisoned().clone()
    }
}
