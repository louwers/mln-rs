use std::time::Duration;

use mln_storage::http_timeout::{error_retry_timeout, expiration_timeout};
use mln_storage::response::Reason;
use mln_util::chrono::{Timestamp, now};

fn now_plus(s: i64) -> Timestamp {
    Timestamp::from_secs(now().secs_since_epoch() + s)
}

// gtest: HttpRetry.OtherError
#[test]
fn other_error() {
    // Non-retryable
    assert_eq!(None, error_retry_timeout(Reason::Other, 1, None));
}

// gtest: HttpRetry.NotFound
#[test]
fn not_found() {
    // Non-retryable
    assert_eq!(None, error_retry_timeout(Reason::NotFound, 1, None));
}

// gtest: HttpRetry.ServerError
#[test]
fn server_error() {
    // 1-3 failures -> 1 sec
    assert_eq!(Some(Duration::from_secs(1)), error_retry_timeout(Reason::Server, 1, None));
    assert_eq!(Some(Duration::from_secs(1)), error_retry_timeout(Reason::Server, 3, None));

    // After 3, exponential backoff
    assert_eq!(Some(Duration::from_secs(2)), error_retry_timeout(Reason::Server, 4, None));
    assert_eq!(Some(Duration::from_secs(1 << 31)), error_retry_timeout(Reason::Server, 50, None));
}

// gtest: HttpRetry.ConnectionError
#[test]
fn connection_error() {
    // Exponential backoff
    assert_eq!(Some(Duration::from_secs(1)), error_retry_timeout(Reason::Server, 1, None));
    assert_eq!(Some(Duration::from_secs(1 << 31)), error_retry_timeout(Reason::Connection, 50, None));
}

// gtest: HttpRetry.RateLimit
#[test]
fn rate_limit() {
    // Pre-set value from header
    assert_eq!(Some(Duration::from_secs(1)), error_retry_timeout(Reason::Server, 1, Some(now_plus(1))));

    // Default
    assert_eq!(Some(Duration::from_secs(5)), error_retry_timeout(Reason::RateLimit, 1, None));

    // A retry time that has already passed retries at once
    assert_eq!(Some(Duration::ZERO), error_retry_timeout(Reason::RateLimit, 1, Some(now_plus(-1))));
}

// gtest: HttpRetry.ExpiredInitial
#[test]
fn expired_initial() {
    // 1 sec timeout
    assert_eq!(Some(Duration::from_secs(1)), expiration_timeout(Some(now_plus(1)), 0));
}

// gtest: HttpRetry.ExpiredSubsequent
#[test]
fn expired_subsequent() {
    // exponential backoff
    assert_eq!(Some(Duration::from_secs(1)), expiration_timeout(None, 1));
    assert_eq!(Some(Duration::from_secs(1 << 31)), expiration_timeout(None, 50));
}

// gtest: HttpRetry.ExpiredNotSet
#[test]
fn expired_not_set() {
    // No expires header set
    assert_eq!(None, expiration_timeout(None, 0));
}
