// HTTP retry and expiration timeouts. They live here rather than in
// `mln_util` because they take a response error's `Reason`.

use std::time::Duration;

use mln_util::chrono::{Timestamp, now};
use mln_util::constants::DEFAULT_RATE_LIMIT_TIMEOUT;

use crate::response::Reason;

/// How long from now until `timestamp`; zero once it has passed.
fn until(timestamp: Timestamp) -> Duration {
    Duration::from_secs(u64::try_from(timestamp.secs_since_epoch() - now().secs_since_epoch()).unwrap_or(0))
}

/// The longest wait between retries after a run of connection or server
/// errors, so that a resource comes back soon after a long outage ends.
const MAXIMUM_ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// The shortest wait a server's `Retry-After` gets, so that a retry time
/// already past, as a skewed clock makes it, doesn't retry in a loop.
const MINIMUM_RETRY_AFTER: Duration = Duration::from_secs(1);

/// `2^exponent` seconds, at most `MAXIMUM_ERROR_BACKOFF`.
fn backoff(exponent: u32) -> Duration {
    Duration::from_secs(1 << exponent.min(31)).min(MAXIMUM_ERROR_BACKOFF)
}

/// How long to wait before retrying a failed request; `None` when the
/// failure doesn't call for a retry. A server error or rate limit waits as
/// long as the server's `Retry-After` asks.
#[must_use]
pub(crate) fn error_retry_timeout(
    failed_request_reason: Reason,
    failed_requests: u32,
    retry_after: Option<Timestamp>,
) -> Option<Duration> {
    match (failed_request_reason, retry_after) {
        (Reason::Server | Reason::RateLimit, Some(retry_after)) => Some(until(retry_after).max(MINIMUM_RETRY_AFTER)),
        // Retry after one second three times, then start exponential backoff.
        (Reason::Server, None) => Some(backoff(failed_requests.saturating_sub(3))),
        // Immediate exponential backoff.
        (Reason::Connection, _) => Some(backoff(failed_requests.saturating_sub(1))),
        (Reason::RateLimit, None) => Some(DEFAULT_RATE_LIMIT_TIMEOUT),
        // No error, or not an error that triggers retries.
        (Reason::Success | Reason::NotFound | Reason::Other, _) => None,
    }
}

/// How long until a response expires, or, after it came back expired, the
/// backoff before asking again; `None` when it never expires.
#[must_use]
pub(crate) fn expiration_timeout(expires: Option<Timestamp>, expired_requests: u32) -> Option<Duration> {
    if expired_requests > 0 {
        return Some(Duration::from_secs(1 << (expired_requests - 1).min(31)));
    }
    expires.map(until)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_retry_timeouts() {
        assert_eq!(error_retry_timeout(Reason::Success, 1, None), None);
        assert_eq!(error_retry_timeout(Reason::NotFound, 1, None), None);
        assert_eq!(error_retry_timeout(Reason::Server, 1, None), Some(Duration::from_secs(1)));
        assert_eq!(error_retry_timeout(Reason::Server, 2, None), Some(Duration::from_secs(1)));
        assert_eq!(error_retry_timeout(Reason::Server, 3, None), Some(Duration::from_secs(1)));
        assert_eq!(error_retry_timeout(Reason::Server, 4, None), Some(Duration::from_secs(2)));
        assert_eq!(error_retry_timeout(Reason::Server, 5, None), Some(Duration::from_secs(4)));
        assert_eq!(error_retry_timeout(Reason::Connection, 1, None), Some(Duration::from_secs(1)));
        assert_eq!(error_retry_timeout(Reason::Connection, 2, None), Some(Duration::from_secs(2)));
        assert_eq!(error_retry_timeout(Reason::Connection, 3, None), Some(Duration::from_secs(4)));
        assert_eq!(error_retry_timeout(Reason::Connection, 4, None), Some(Duration::from_secs(8)));
        assert_eq!(error_retry_timeout(Reason::Connection, 7, None), Some(Duration::from_secs(60)));
        assert_eq!(error_retry_timeout(Reason::Connection, 50, None), Some(Duration::from_secs(60)));
        assert_eq!(error_retry_timeout(Reason::RateLimit, 1, None), Some(Duration::from_secs(5)));
        let in_one_minute = Timestamp::from_secs(now().secs_since_epoch() + 60);
        let rate_limited = error_retry_timeout(Reason::RateLimit, 1, Some(in_one_minute));
        assert!(rate_limited >= Some(Duration::from_secs(59)) && rate_limited <= Some(Duration::from_secs(60)));
        let unavailable = error_retry_timeout(Reason::Server, 1, Some(in_one_minute));
        assert!(unavailable >= Some(Duration::from_secs(59)) && unavailable <= Some(Duration::from_secs(60)));
    }

    #[test]
    fn expiration_timeouts() {
        assert_eq!(expiration_timeout(None, 0), None);
        assert_eq!(expiration_timeout(None, 1), Some(Duration::from_secs(1)));
        assert_eq!(expiration_timeout(None, 2), Some(Duration::from_secs(2)));
        assert_eq!(expiration_timeout(None, 3), Some(Duration::from_secs(4)));
        let past = Timestamp::from_secs(now().secs_since_epoch() - 10);
        assert_eq!(expiration_timeout(Some(past), 0), Some(Duration::ZERO));
    }
}
