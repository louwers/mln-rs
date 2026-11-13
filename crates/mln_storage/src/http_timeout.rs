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

/// How long to wait before retrying a failed request; `None` when the
/// failure doesn't call for a retry.
#[must_use]
pub(crate) fn error_retry_timeout(
    failed_request_reason: Reason,
    failed_requests: u32,
    retry_after: Option<Timestamp>,
) -> Option<Duration> {
    match failed_request_reason {
        Reason::Server => {
            // Retry after one second three times, then start exponential backoff.
            Some(Duration::from_secs(if failed_requests <= 3 { 1 } else { 1 << (failed_requests - 3).min(31) }))
        }
        Reason::Connection => {
            // Immediate exponential backoff.
            debug_assert!(failed_requests > 0);
            Some(Duration::from_secs(1 << failed_requests.wrapping_sub(1).min(31)))
        }
        Reason::RateLimit => Some(match retry_after {
            Some(retry_after) => until(retry_after),
            // Default
            None => DEFAULT_RATE_LIMIT_TIMEOUT,
        }),
        // No error, or not an error that triggers retries.
        _ => None,
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
        assert_eq!(error_retry_timeout(Reason::Connection, 50, None), Some(Duration::from_secs(1 << 31)));
        assert_eq!(error_retry_timeout(Reason::RateLimit, 1, None), Some(Duration::from_secs(5)));
        let in_one_minute = Timestamp::from_secs(now().secs_since_epoch() + 60);
        let rate_limited = error_retry_timeout(Reason::RateLimit, 1, Some(in_one_minute));
        assert!(rate_limited >= Some(Duration::from_secs(59)) && rate_limited <= Some(Duration::from_secs(60)));
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
