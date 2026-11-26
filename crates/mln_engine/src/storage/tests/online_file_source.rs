// The tests run the online file source over the simulated network
// (`super::network`): its server answers on the test's thread, and its
// clock moves only when a test moves it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::network::{Answers, Network, Reply};
use crate::storage::chrono::{Timestamp, now, rfc1123};
use crate::storage::file_source::FileSource;
use crate::storage::http_header_transform::HttpHeaderTransform;
use crate::storage::network_status::Status;
use crate::storage::online_file_source::OnlineFileSource;
use crate::storage::request::Request;
use crate::storage::resource::{Kind, Priority, Resource};
use crate::storage::resource_transform::FinishedCallback;
use crate::storage::response::{Reason, ResourceError, Response};

/// The URL of `path` on the simulated server.
fn url(path: &str) -> String {
    format!("http://example.invalid{path}")
}

fn resource(path: &str) -> Resource {
    Resource::new(Kind::Unknown, url(path))
}

fn data(res: &Response) -> &[u8] {
    res.data.as_deref().map(Vec::as_slice).expect("data")
}

/// A server that answers every transfer with "Hello World!" and no
/// headers.
fn hello() -> Network {
    Network::serving(|_| Reply::ok("Hello World!"))
}

/// A server whose first answer is a 500, and every later one "Hello
/// World!".
fn temporary_error() -> Network {
    let answered = AtomicU32::new(0);
    Network::serving(move |_| {
        if answered.fetch_add(1, Ordering::SeqCst) == 0 { Reply::status(500) } else { Reply::ok("Hello World!") }
    })
}

/// Checks a response that came without caching headers: stale at once.
fn assert_uncached(res: &Response) {
    assert!(res.caching.expires.is_some_and(|expires| expires <= now()));
    assert!(!res.caching.must_revalidate);
    assert!(res.modified.is_none());
    assert!(res.etag.is_none());
}

/// Checks an error response, which has no data and no caching rules.
fn assert_error(res: &Response, reason: Reason) {
    assert_eq!(reason, res.error.as_ref().expect("error").reason);
    assert!(res.data.is_none());
    assert!(res.caching.expires.is_none());
    assert!(!res.caching.must_revalidate);
    assert!(res.modified.is_none());
    assert!(res.etag.is_none());
}

/// Answers the held transfers one at a time, the next once the source has
/// run with the last one's answer, until none is left.
fn answer_in_turn(network: &mut Network) {
    network.run();
    loop {
        let held = network.server.held();
        if held.is_empty() {
            return;
        }
        for transfer in held {
            transfer.respond(200, Vec::new(), b"Response".to_vec());
        }
        network.run();
    }
}

// gtest: OnlineFileSource.Cancel
#[test]
fn cancel() {
    let mut network = hello();
    let (request, answers) = network.request(&resource("/test"));
    drop(request);
    network.run();
    assert_eq!(Vec::<String>::new(), network.server.requested());
    assert!(answers.lock().is_empty());

    // Nor does one dropped before the source took it.
    let (request, responder) = Request::new(|_| panic!("Callback should not be called"));
    drop(request);
    network.source.load(&resource("/test"), responder);
    network.run();
    assert_eq!(Vec::<String>::new(), network.server.requested());
}

// gtest: OnlineFileSource.CancelMultiple
#[test]
fn cancel_multiple() {
    let mut network = hello();
    let (cancelled, not_answered) = network.request(&resource("/test"));
    let (_request, answers) = network.request(&resource("/test"));
    drop(cancelled);
    network.run();

    assert!(not_answered.lock().is_empty());
    let answers = answers.lock();
    assert_eq!(1, answers.len());
    assert_eq!(None, answers[0].error);
    assert_eq!(b"Hello World!", data(&answers[0]));
    assert_uncached(&answers[0]);
}

// gtest: OnlineFileSource.TemporaryError
#[test]
fn temporary_error_is_retried_after_a_second() {
    let mut network = temporary_error();
    let (_request, answers) = network.request(&resource("/temporary-error"));
    network.run();
    {
        let answers = answers.lock();
        assert_eq!(1, answers.len());
        assert_error(&answers[0], Reason::Server);
        assert_eq!("HTTP status code 500", answers[0].error.as_ref().unwrap().message);
    }

    network.advance(Duration::from_millis(999));
    assert_eq!(1, network.server.requested().len(), "no retry before a second");
    network.advance(Duration::from_millis(1));
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    assert_eq!(None, answers[1].error);
    assert_eq!(b"Hello World!", data(&answers[1]));
    assert_uncached(&answers[1]);
}

// gtest: OnlineFileSource.ConnectionError
#[test]
fn connection_error() {
    let mut network = Network::serving(|_| Reply::Refused);
    let (_request, answers) = network.request(&resource("/"));
    network.run();
    assert_eq!(1, answers.lock().len());

    // The backoff doubles: a second, then two.
    for backoff in [1000, 2000] {
        let answered = answers.lock().len();
        network.advance(Duration::from_millis(backoff - 1));
        assert_eq!(answered, answers.lock().len(), "no retry before {backoff} ms");
        network.advance(Duration::from_millis(1));
        assert_eq!(answered + 1, answers.lock().len(), "a retry after {backoff} ms");
    }
    for answer in answers.lock().iter() {
        assert_error(answer, Reason::Connection);
    }
}

// gtest: OnlineFileSource.Timeout
// A response revalidates when it expires, each time. Its lifetime is a
// minute, which the source counts in whole seconds: 59 or 60.
#[test]
fn timeout() {
    let mut network = Network::serving(|_| Reply::ok("Hello World!").header("Cache-Control", "max-age=60"));
    let (_request, answers) = network.request(&resource("/test"));
    network.run();
    for answered in 1..4 {
        network.advance(Duration::from_secs(58));
        assert_eq!(answered, answers.lock().len(), "no revalidation before the response expires");
        network.advance(Duration::from_secs(2));
        assert_eq!(answered + 1, answers.lock().len(), "a revalidation once it expires");
    }
    for res in answers.lock().iter() {
        assert_eq!(None, res.error);
        assert_eq!(b"Hello World!", data(res));
        assert!(res.caching.expires.is_some());
        assert!(!res.caching.must_revalidate);
        assert!(res.modified.is_none());
        assert!(res.etag.is_none());
    }
}

/// A response that is fresh isn't asked for again while it is, whatever the
/// order of its caching headers: here tile.openstreetmap.org's, whose
/// `Expires`, after `max-age`, is in the past.
#[test]
fn a_fresh_response_is_not_requested_again() {
    let mut network = Network::serving(|_| {
        Reply::ok("tile")
            .header("ETag", "\"a\"")
            .header("Cache-Control", "max-age=88823, stale-while-revalidate=604800, stale-if-error=604800")
            .header("Expires", "Wed, 18 Nov 2026 02:37:39 GMT")
            .header("Age", "67981")
    });
    let (_request, answers) = network.request(&resource("/0/0/0.png"));
    network.run();
    // Less than the 88823 - 67981 seconds it stays fresh.
    network.advance(Duration::from_secs(3600));

    let answers = answers.lock();
    assert_eq!(1, answers.len());
    assert_eq!(None, answers[0].error);
    assert!(answers[0].caching.expires.is_some_and(|expires| expires > now()), "fresh");
    assert_eq!(1, network.server.requested().len());
}

/// A 404 isn't requested again, even with caching headers: the file source
/// of the C++ Map.StyleNotFound.
#[test]
fn a_not_found_is_not_requested_again() {
    for reply in [|| Reply::status(404), || Reply::status(404).header("Cache-Control", "max-age=30")] {
        let mut network = Network::serving(move |_| reply());
        let (_request, answers) = network.request(&resource("/style.json"));
        network.run();
        network.advance(Duration::from_secs(3600));

        let answers = answers.lock();
        assert_eq!(1, answers.len());
        assert_error(&answers[0], Reason::NotFound);
        assert_eq!(1, network.server.requested().len());
    }
}

/// A response that came expired has no freshness lifetime to revalidate it
/// at: the source doesn't ask for it again until it is requested anew.
#[test]
fn an_expired_response_is_not_requested_again() {
    let mut network =
        Network::serving(|_| Reply::ok("Hello World!").header("Expires", rfc1123(Timestamp::from_secs(10000))));
    let (_request, answers) = network.request(&resource("/test"));
    network.run();
    network.advance(Duration::from_secs(3600));

    let answers = answers.lock();
    assert_eq!(1, answers.len());
    assert_eq!(None, answers[0].error);
    assert!(now() >= answers[0].caching.expires.expect("expires"));
    assert!(!answers[0].caching.must_revalidate);
    assert_eq!(1, network.server.requested().len());
}

/// The freshness lifetime counts from the server's `Date`, whatever its
/// clock says.
#[test]
fn clock_skew() {
    let mut network = Network::serving(|_| {
        Reply::ok("Response")
            .header("Date", "Fri, 01 Jan 2010 10:00:00 GMT")
            .header("Expires", "Fri, 01 Jan 2010 10:01:00 GMT")
    });
    let (_request, answers) = network.request(&resource("/clockskew"));
    network.run();

    let answers = answers.lock();
    assert_eq!(None, answers[0].error);
    let lifetime = answers[0].caching.expires.expect("expires").secs_since_epoch() - now().secs_since_epoch();
    assert!((58..=60).contains(&lifetime), "a minute's lifetime, not {lifetime}s");
}

// gtest: OnlineFileSource.RespectPriorExpires
#[test]
fn respect_prior_expires() {
    let mut network = hello();
    let far_ahead = Timestamp::from_secs(now().secs_since_epoch() + 100_000);

    // A copy that expires far ahead isn't requested before it does.
    let mut fresh = resource("/test");
    fresh.prior_caching.expires = Some(far_ahead);
    let (_first, first) = network.request(&fresh);
    // Without an expiry, at once.
    let (_second, second) = network.request(&resource("/test"));
    let (_third, third) = network.request(&fresh);
    network.run();
    network.advance(Duration::from_secs(3600));

    assert_eq!(1, second.lock().len());
    assert!(first.lock().is_empty());
    assert!(third.lock().is_empty());
    assert_eq!(1, network.server.requested().len());
}

// gtest: OnlineFileSource.RespectMinimumUpdateInterval
#[test]
fn respect_minimum_update_interval() {
    let mut network = hello();
    let mut resource = resource("/test");
    resource.prior_caching.expires = Some(now());
    resource.minimum_update_interval = Duration::from_secs(1);

    let (_request, answers) = network.request(&resource);
    network.run();
    network.advance(Duration::from_millis(999));
    assert!(answers.lock().is_empty(), "no request before the minimum update interval");
    network.advance(Duration::from_millis(1));
    assert_eq!(1, answers.lock().len());
}

/// The `Load` test's requests: each answer to one starts the next, until
/// `MAX` have been made.
struct Load {
    source: Arc<OnlineFileSource>,
    requests: Mutex<Vec<Option<Request>>>,
    next: AtomicI32,
}

impl Load {
    const MAX: i32 = 10000;

    fn request(self: &Arc<Self>, slot: usize) {
        let number = self.next.fetch_add(1, Ordering::SeqCst);
        let load = Arc::clone(self);
        let (request, responder) = Request::new(move |res: Response| {
            assert_eq!(None, res.error);
            assert_eq!(format!("Request {number}").as_bytes(), data(&res));
            assert_uncached(&res);
            if load.next.load(Ordering::SeqCst) <= Load::MAX {
                load.request(slot);
            }
        });
        self.source.load(&resource(&format!("/load/{number}")), responder);
        // Drops the request that this one follows.
        self.requests.lock()[slot] = Some(request);
    }
}

// gtest: OnlineFileSource.Load
#[test]
fn load() {
    const CONCURRENCY: usize = 50;

    let mut network = Network::serving(|resource| {
        let number = resource.url.rsplit('/').next().unwrap();
        Reply::ok(&format!("Request {number}"))
    });
    let load = Arc::new(Load {
        source: Arc::clone(&network.source),
        requests: Mutex::new((0..CONCURRENCY).map(|_| None).collect()),
        next: AtomicI32::new(1),
    });
    for slot in 0..CONCURRENCY {
        load.request(slot);
    }
    network.run();
    assert_eq!(Load::MAX as usize, network.server.requested().len());
}

// Test for https://github.com/mapbox/mapbox-gl-native/issues/2123
//
// A request is made. While the request is in progress, the network status
// changes. This should trigger an immediate retry of all requests that are
// not in progress. This test makes sure that we don't accidentally
// double-trigger the request: its retry after a connection error is in
// flight when the network becomes reachable.

// gtest: OnlineFileSource.NetworkStatusChange
#[test]
fn network_status_change() {
    let refused = AtomicBool::new(false);
    let mut network =
        Network::serving(move |_| if refused.swap(true, Ordering::SeqCst) { Reply::Hold } else { Reply::Refused });
    let (_request, answers) = network.request(&resource("/delayed"));
    network.run();
    network.advance(Duration::from_secs(1));
    assert_eq!(2, network.server.requested().len(), "the retry is in flight");
    network.status.reachable();
    network.run();
    assert_eq!(Vec::<String>::new(), network.server.requested(), "no second transfer");

    for transfer in network.server.held() {
        transfer.respond(200, Vec::new(), b"Response".to_vec());
    }
    network.run();
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    assert_eq!(None, answers[1].error);
    assert_eq!(b"Response", data(&answers[1]));
    assert_uncached(&answers[1]);
    assert_eq!(Vec::<String>::new(), network.server.requested());
}

// Tests that a change in network status preempts the backoff of a request
// that failed due to connection or reachability issues: the request retries
// at once.

// gtest: OnlineFileSource.NetworkStatusChangePreempt
#[test]
fn network_status_change_preempt() {
    let mut network = Network::serving(|_| Reply::Refused);
    let (_request, answers) = network.request(&resource("/test"));
    network.run();
    assert_eq!(1, answers.lock().len());

    // The clock stays where it is, a second before the backoff ends.
    network.status.reachable();
    network.run();
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    for answer in answers.iter() {
        assert_error(answer, Reason::Connection);
    }
}

// gtest: OnlineFileSource.NetworkStatusOnlineOffline
#[test]
fn network_status_online_offline() {
    let mut network = hello();
    assert_eq!(network.status.get(), Status::Online, "Default status should be Online");
    network.status.set(Status::Offline);

    // Offline, the request fails at once, without a transfer, then loads
    // once back online.
    let (_request, answers) = network.request(&resource("/test"));
    network.run();
    assert_eq!(1, answers.lock().len());
    assert_eq!(Reason::Connection, answers.lock()[0].error.as_ref().expect("an error").reason);
    assert_eq!(Vec::<String>::new(), network.server.requested());

    network.status.set(Status::Online);
    network.run();
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    assert_eq!(None, answers[1].error);
    assert!(answers[1].data.is_some());
}

/// Checks the error of a 429 that `reply` answers.
fn rate_limit_test(reply: impl Fn() -> Reply + Send + Sync + 'static, expect_retry_after: bool) {
    let mut network = Network::serving(move |_| reply());
    // The reset time has whole seconds, so it can pass before the response
    // arrives; it's only certain to be after the request was made.
    let requested_at = now();
    let (_request, answers) = network.request(&resource("/rate-limit"));
    network.run();

    let answers = answers.lock();
    let error = answers[0].error.as_ref().expect("error");
    assert_eq!(Reason::RateLimit, error.reason);
    if expect_retry_after {
        let retry_after = error.retry_after.expect("retry after");
        assert!(requested_at < retry_after);
    } else {
        assert!(error.retry_after.is_none());
    }
}

// gtest: OnlineFileSource.RateLimitStandard
#[test]
fn rate_limit_standard() {
    rate_limit_test(|| Reply::status(429).header("Retry-After", "1"), true);
}

// gtest: OnlineFileSource.RateLimitMBX
#[test]
fn rate_limit_mbx() {
    rate_limit_test(
        || Reply::status(429).header("x-rate-limit-reset", (now().secs_since_epoch() + 1).to_string()),
        true,
    );
}

// gtest: OnlineFileSource.RateLimitDefault
#[test]
fn rate_limit_default() {
    rate_limit_test(|| Reply::status(429), false);
}

// gtest: OnlineFileSource.LowHighPriorityRequests
// A low priority request waits behind a regular one made after it.
#[test]
fn low_high_priority_requests() {
    let mut network = Network::holding();
    network.source.set_maximum_concurrent_requests(1);
    let mut low = resource("/load/2");
    low.set_priority(Priority::Low);
    let _requests =
        [network.request(&resource("/load/1")), network.request(&low), network.request(&resource("/load/3"))];

    answer_in_turn(&mut network);
    assert_eq!([url("/load/1"), url("/load/3"), url("/load/2")].to_vec(), network.server.requested());
}

// gtest: OnlineFileSource.LowHighPriorityRequestsMany
#[test]
fn low_high_priority_requests_many() {
    let mut network = Network::holding();
    network.source.set_maximum_concurrent_requests(1);
    let _requests: Vec<(Request, Answers)> = (0..20)
        .map(|number| {
            let mut resource = resource(&format!("/load/{number}"));
            if number % 2 == 1 {
                resource.set_priority(Priority::Low);
            }
            network.request(&resource)
        })
        .collect();

    answer_in_turn(&mut network);
    // The first starts at once; the regular ones go before the low ones.
    let expected: Vec<String> = [0]
        .into_iter()
        .chain((2..20).step_by(2))
        .chain((1..20).step_by(2))
        .map(|n| url(&format!("/load/{n}")))
        .collect();
    assert_eq!(expected, network.server.requested());
}

/// Waiting requests start by their resources' order as it is when a
/// connection frees up, which the requester may have changed since it made
/// the request, and one cancelled meanwhile doesn't start.
#[test]
fn pending_requests_start_by_order() {
    let mut network = Network::holding();
    network.source.set_maximum_concurrent_requests(1);
    let resources: Vec<Resource> = (0..5)
        .map(|order| {
            let resource = resource(&format!("/load/{order}"));
            resource.order.set(order);
            resource
        })
        .collect();
    let mut requests: Vec<Option<Request>> =
        resources.iter().map(|resource| Some(network.request(resource).0)).collect();
    // The first request starts; the others wait for it.
    network.run();

    resources[4].order.set(1);
    resources[1].order.set(5);
    requests[2] = None;
    answer_in_turn(&mut network);
    let expected: Vec<String> = [0, 4, 3, 1].iter().map(|n| url(&format!("/load/{n}"))).collect();
    assert_eq!(expected, network.server.requested());
}

// gtest: OnlineFileSource.MaximumConcurrentRequests
#[test]
fn maximum_concurrent_requests() {
    let network = Network::holding();
    assert_eq!(network.source.maximum_concurrent_requests(), 20);

    network.source.set_maximum_concurrent_requests(10);
    assert_eq!(network.source.maximum_concurrent_requests(), 10);
}

// gtest: OnlineFileSource.RequestSameUrlMultipleTimes
// The requests share one transfer, and each takes the answer.
#[test]
fn request_same_url_multiple_times() {
    let mut network = Network::serving(|_| Reply::ok("Request"));
    let requests: Vec<(Request, Answers)> = (0..100).map(|_| network.request(&resource("/load"))).collect();
    network.run();
    assert!(requests.iter().all(|(_, answers)| answers.lock().len() == 1));
    assert_eq!(1, network.server.requested().len());
}

// gtest: OnlineFileSource.FailedResourceTransform
#[test]
fn failed_resource_transform() {
    let mut network = hello();
    network.source.set_resource_transform(Some(Arc::new(|_kind: Kind, _url: &str, finished: FinishedCallback| {
        finished(Err(ResourceError::new(Reason::Other, "No credential")));
    })));

    let (_request, answers) = network.request(&resource("/test"));
    network.run();
    let answers = answers.lock();
    let error = answers[0].error.as_ref().expect("error");
    assert_eq!(Reason::Other, error.reason);
    assert_eq!("No credential", error.message);
    assert!(answers[0].data.is_none());
    assert_eq!(Vec::<String>::new(), network.server.requested());
}

// A retry of a request that the resource transform failed asks the transform
// again instead of loading the original URL.
// gtest: OnlineFileSource.RetryFailedResourceTransform
#[test]
fn retry_failed_resource_transform() {
    let mut network = hello();
    let transforms = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&transforms);
    network.source.set_resource_transform(Some(Arc::new(
        move |_kind: Kind, original: &str, finished: FinishedCallback| {
            assert_eq!(url("/delayed"), original);
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                finished(Err(ResourceError::new(Reason::Connection, "Offline")));
            } else {
                finished(Ok(url("/test")));
            }
        },
    )));

    let (_request, answers) = network.request(&resource("/delayed"));
    network.run();
    {
        let answers = answers.lock();
        let error = answers[0].error.as_ref().expect("error");
        assert_eq!(Reason::Connection, error.reason);
        assert_eq!("Offline", error.message);
    }
    // Retries now instead of after the backoff.
    network.status.reachable();
    network.run();
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    assert_eq!(None, answers[1].error);
    assert_eq!(b"Hello World!", data(&answers[1]));
    assert_eq!(2, transforms.load(Ordering::SeqCst));
    assert_eq!(vec![url("/test")], network.server.requested());
}

// The header transform sees the URL that the resource transform wrote, and
// runs again for a retry.
#[test]
fn a_header_transform_runs_for_every_attempt() {
    let mut network = temporary_error();
    let target = url("/temporary-error");
    let rewritten = target.clone();
    network.source.set_resource_transform(Some(Arc::new(
        move |_kind: Kind, _url: &str, finished: FinishedCallback| {
            finished(Ok(rewritten.clone()));
        },
    )));
    let urls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&urls);
    network.source.set_http_header_transform(HttpHeaderTransform::new(Some(Arc::new(move |kind: Kind, url: &str| {
        seen.lock().push((kind, url.to_owned()));
        vec![("X-Test-Token".to_owned(), "secret-token".to_owned())]
    }))));

    let (_request, answers) = network.request(&Resource::new(Kind::Source, "localhost://temporary-error"));
    network.run();
    network.advance(Duration::from_secs(1));
    let answers = answers.lock();
    assert_eq!(2, answers.len());
    assert_eq!(None, answers[1].error);
    assert_eq!(b"Hello World!", data(&answers[1]));
    assert_eq!(vec![(Kind::Source, target.clone()), (Kind::Source, target)], *urls.lock());
}

/// Offline, a request for a resource the requester has a copy of waits for
/// the network quietly, and revalidates the copy once it is back.
#[test]
fn offline_a_requester_with_a_copy_keeps_it() {
    let mut network = hello();
    network.status.set(Status::Offline);
    let mut resource = resource("/test");
    resource.prior_etag = Some("a".to_owned());

    let (_request, answers) = network.request(&resource);
    network.run();
    assert!(answers.lock().is_empty(), "no error while offline");
    assert_eq!(Vec::<String>::new(), network.server.requested());

    network.status.set(Status::Online);
    network.run();
    let answers = answers.lock();
    assert_eq!(1, answers.len());
    assert_eq!(None, answers[0].error);
    let revalidated = network.server.requested_resources();
    assert_eq!(1, revalidated.len());
    assert_eq!(Some("a"), revalidated[0].prior_etag.as_deref());
}
