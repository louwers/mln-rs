// Ports src/mln/util/http_timeout.hpp and src/mln/util/http_timeout.cpp
// (`http::errorRetryTimeout`, `http::expirationTimeout`). They live here
// rather than in mln_util because they take `Response::Error::Reason`.

use mln_util::chrono::{Duration, Timestamp, now};
use mln_util::constants::DEFAULT_RATE_LIMIT_TIMEOUT;

use crate::response::Reason;

fn seconds(secs: i64) -> Duration {
    Duration::from_secs(secs)
}

/// `Timestamp - Timestamp` as a `Duration` (both are whole seconds).
fn timestamp_diff(a: Timestamp, b: Timestamp) -> Duration {
    seconds(a.secs_since_epoch() - b.secs_since_epoch())
}

/// `errorRetryTimeout(failedRequestReason, failedRequests, retryAfter)`
pub fn error_retry_timeout(
    failed_request_reason: Reason,
    failed_requests: u32,
    retry_after: Option<Timestamp>,
) -> Duration {
    match failed_request_reason {
        Reason::Server => {
            // Retry after one second three times, then start exponential backoff.
            seconds(if failed_requests <= 3 { 1 } else { 1i64 << (failed_requests - 3).min(31) })
        }
        Reason::Connection => {
            // Immediate exponential backoff.
            debug_assert!(failed_requests > 0);
            seconds(1i64 << failed_requests.wrapping_sub(1).min(31))
        }
        Reason::RateLimit => match retry_after {
            Some(retry_after) => std::cmp::max(Duration::zero(), timestamp_diff(retry_after, now())),
            // Default
            None => seconds(i64::from(DEFAULT_RATE_LIMIT_TIMEOUT)),
        },
        // No error, or not an error that triggers retries.
        _ => Duration::max(),
    }
}

/// `expirationTimeout(expires, expiredRequests)`
pub fn expiration_timeout(expires: Option<Timestamp>, expired_requests: u32) -> Duration {
    if expired_requests > 0 {
        return seconds(1i64 << (expired_requests - 1).min(31));
    }
    if let Some(expires) = expires {
        let d = timestamp_diff(expires, now());
        return if d < Duration::zero() { Duration::zero() } else { d };
    }
    Duration::max()
}

#[cfg(test)]
mod tests {
    // Ports test/util/http_timeout.test.cpp.
    use super::*;

    #[test]
    fn error_retry_timeouts() {
        assert_eq!(error_retry_timeout(Reason::Success, 1, None), Duration::max());
        assert_eq!(error_retry_timeout(Reason::NotFound, 1, None), Duration::max());
        assert_eq!(error_retry_timeout(Reason::Server, 1, None), seconds(1));
        assert_eq!(error_retry_timeout(Reason::Server, 2, None), seconds(1));
        assert_eq!(error_retry_timeout(Reason::Server, 3, None), seconds(1));
        assert_eq!(error_retry_timeout(Reason::Server, 4, None), seconds(2));
        assert_eq!(error_retry_timeout(Reason::Server, 5, None), seconds(4));
        assert_eq!(error_retry_timeout(Reason::Connection, 1, None), seconds(1));
        assert_eq!(error_retry_timeout(Reason::Connection, 2, None), seconds(2));
        assert_eq!(error_retry_timeout(Reason::Connection, 3, None), seconds(4));
        assert_eq!(error_retry_timeout(Reason::Connection, 4, None), seconds(8));
        assert_eq!(error_retry_timeout(Reason::Connection, 50, None), seconds(1 << 31));
        assert_eq!(error_retry_timeout(Reason::RateLimit, 1, None), seconds(5));
        let in_one_minute = Timestamp::from_secs(now().secs_since_epoch() + 60);
        let rate_limited = error_retry_timeout(Reason::RateLimit, 1, Some(in_one_minute));
        assert!(rate_limited >= seconds(59) && rate_limited <= seconds(60));
    }

    #[test]
    fn expiration_timeouts() {
        assert_eq!(expiration_timeout(None, 0), Duration::max());
        assert_eq!(expiration_timeout(None, 1), seconds(1));
        assert_eq!(expiration_timeout(None, 2), seconds(2));
        assert_eq!(expiration_timeout(None, 3), seconds(4));
        let past = Timestamp::from_secs(now().secs_since_epoch() - 10);
        assert_eq!(expiration_timeout(Some(past), 0), Duration::zero());
    }
}
