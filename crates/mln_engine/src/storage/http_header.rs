//! HTTP response headers: what caching headers allow (RFC 9111),
//! and when a rate-limited request may be retried.

use crate::storage::chrono::{Timestamp, now, parse_http_date, parse_timestamp_i32};

/// The directives of a response's `Cache-Control` that a private cache
/// heeds (RFC 9111 §5.2.2). Durations are in seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub max_age: Option<u64>,
    pub must_revalidate: bool,
    /// Only the unqualified form: `no-cache="field"` restricts the named
    /// fields, not the response (§5.2.2.4).
    pub no_cache: bool,
    pub no_store: bool,
}

/// The largest delta-seconds a cache needs to tell apart (RFC 9111 §1.2.2).
const MAX_DELTA_SECONDS: u64 = 1 << 31;

/// `value` as delta-seconds: digits, saturating at [`MAX_DELTA_SECONDS`].
fn delta_seconds(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(value.parse::<u64>().map_or(MAX_DELTA_SECONDS, |seconds| seconds.min(MAX_DELTA_SECONDS)))
}

/// The comma-separated elements of a field value, splitting only outside
/// quoted strings.
fn list_elements(value: &str) -> Vec<&str> {
    let mut elements = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                elements.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    elements.push(&value[start..]);
    elements
}

/// The content of `value` if it is a quoted string, else `value`.
fn unquote(value: &str) -> String {
    match value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(inner) => {
            let mut unquoted = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                unquoted.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
            }
            unquoted
        }
        None => value.to_owned(),
    }
}

impl CacheControl {
    /// Reads the directives of a `Cache-Control` field value, or of the
    /// values of its field lines joined with commas (RFC 9110 §5.3).
    /// Directive names are case-insensitive, and a value may be a token or a
    /// quoted string. A directive given twice keeps its first value (RFC
    /// 9111 §4.2.1); one whose value doesn't parse, and unknown ones, don't
    /// count.
    #[must_use]
    pub fn parse(value: &str) -> CacheControl {
        let mut result = CacheControl::default();
        for element in list_elements(value) {
            let (name, argument) = match element.split_once('=') {
                Some((name, argument)) => (name.trim(), Some(unquote(argument.trim()))),
                None => (element.trim(), None),
            };
            let seconds = || argument.as_deref().and_then(delta_seconds);
            match name.to_ascii_lowercase().as_str() {
                "max-age" => result.max_age = result.max_age.or_else(seconds),
                "must-revalidate" => result.must_revalidate = true,
                "no-cache" if argument.is_none() => result.no_cache = true,
                "no-store" => result.no_store = true,
                _ => {}
            }
        }
        result
    }
}

/// The longest freshness lifetime a heuristic gives a response: a resource
/// unchanged for years is still checked weekly.
const MAX_HEURISTIC_LIFETIME: u64 = 7 * 24 * 60 * 60;

/// The caching headers of a response, as it arrived.
#[derive(Clone, Debug)]
pub struct CachingHeaders<'a> {
    pub cache_control: &'a CacheControl,
    /// The `Expires` field value; one that isn't a date means expired.
    pub expires: Option<&'a str>,
    pub date: Option<Timestamp>,
    /// The `Age` in seconds.
    pub age: Option<u64>,
    /// When the resource last changed, from `Last-Modified` or the cached
    /// copy a 304 revalidates; with it a heuristic gives a response without
    /// an explicit lifetime one.
    pub last_modified: Option<Timestamp>,
}

/// What a response's caching headers allow, as times on the local clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freshness {
    /// When the response stops being fresh.
    pub expires: Timestamp,
    /// A stale copy is never used without revalidating it (`must-revalidate`
    /// or `no-cache`).
    pub must_revalidate: bool,
    pub no_store: bool,
}

impl CachingHeaders<'_> {
    /// The response's freshness, received at `received` (RFC 9111 §4.2):
    ///
    /// - The lifetime is `no-cache`'s none, else `max-age`, else `Expires`
    ///   less `Date` (less the local clock without `Date`), else a tenth of
    ///   the time since `last_modified`, at most a week, else none.
    /// - It counts from the response's `Age`. The apparent age from `Date`
    ///   isn't added, since it holds the difference between the clocks.
    #[must_use]
    pub fn freshness(&self, received: Timestamp) -> Freshness {
        let cc = self.cache_control;
        let origin = self.date.unwrap_or(received);
        let lifetime = if cc.no_cache {
            0
        } else if let Some(max_age) = cc.max_age {
            max_age
        } else if let Some(expires) = self.expires {
            let expires = parse_http_date(expires).unwrap_or_default();
            u64::try_from(expires.secs_since_epoch() - origin.secs_since_epoch()).unwrap_or(0)
        } else if let Some(modified) = self.last_modified {
            (u64::try_from(origin.secs_since_epoch() - modified.secs_since_epoch()).unwrap_or(0) / 10)
                .min(MAX_HEURISTIC_LIFETIME)
        } else {
            0
        };
        let remaining = lifetime as i64 - self.age.unwrap_or(0).min(MAX_DELTA_SECONDS) as i64;
        Freshness {
            expires: Timestamp::from_secs(received.secs_since_epoch() + remaining),
            must_revalidate: cc.must_revalidate || cc.no_cache,
            no_store: cc.no_store,
        }
    }
}

/// Parses an integer as `std::stoi` does: leading whitespace, sign, digits;
/// `None` where it throws.
fn stoi(s: &str) -> Option<i32> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let len = digits.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let magnitude: i64 = digits[..len].parse().ok()?;
    let value = if negative { -magnitude } else { magnitude };
    i32::try_from(value).ok()
}

/// When to retry a rate-limited request, from `Retry-After` (seconds or an
/// HTTP date) or else `X-Rate-Limit-Reset` (a Unix time). `None` when the
/// header it reads is absent or doesn't parse.
pub fn parse_retry_headers(retry_after: &Option<String>, x_rate_limit_reset: &Option<String>) -> Option<Timestamp> {
    if let Some(retry_after) = retry_after {
        match stoi(retry_after) {
            Some(secs) => Some(Timestamp::from_secs(now().secs_since_epoch() + i64::from(secs))),
            None => parse_http_date(retry_after),
        }
    } else if let Some(x_rate_limit_reset) = x_rate_limit_reset {
        stoi(x_rate_limit_reset).map(parse_timestamp_i32)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        let cases: &[(&str, Option<u64>, bool)] = &[
            ("", None, false),
            ("max-age=34", Some(34), false),
            ("max-age =34", Some(34), false),
            (",max-age=1", Some(1), false),
            ("MAX-AGE=5", Some(5), false),
            ("max-age=\"7\"", Some(7), false),
            ("max-age=-1", None, false),
            ("max-age=foo", None, false),
            ("max-age=99999999999999999999", Some(MAX_DELTA_SECONDS), false),
            ("max-age=3, max-age=4", Some(3), false),
            ("foo=\"max-age=3, bar\", max-age=4", Some(4), false),
            ("max-age-x=3", None, false),
            ("private, max-age=0, no-cache", Some(0), false),
            ("max-age=3600, must-revalidate", Some(3600), true),
            ("no-cache=\"Expires,Via\",max-age=3600, must-revalidate", Some(3600), true),
        ];
        for &(value, max_age, must_revalidate) in cases {
            let cc = CacheControl::parse(value);
            assert_eq!(cc.max_age, max_age, "{value}");
            assert_eq!(cc.must_revalidate, must_revalidate, "{value}");
        }
    }

    #[test]
    fn directives() {
        let cc = CacheControl::parse("public,max-age=300,stale-if-error=86400, No-Store");
        assert_eq!(cc.max_age, Some(300));
        assert!(cc.no_store && !cc.no_cache);
        assert!(CacheControl::parse("no-cache").no_cache);
        assert!(!CacheControl::parse("no-cache=\"Set-Cookie\"").no_cache);
    }

    fn freshness(
        cache_control: &str,
        expires: Option<&str>,
        date: Option<i64>,
        age: Option<u64>,
        modified: Option<i64>,
    ) -> Freshness {
        let cc = CacheControl::parse(cache_control);
        let headers = CachingHeaders {
            cache_control: &cc,
            expires,
            date: date.map(Timestamp::from_secs),
            age,
            last_modified: modified.map(Timestamp::from_secs),
        };
        headers.freshness(Timestamp::from_secs(RECEIVED))
    }

    const RECEIVED: i64 = 100_000_000;

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_secs(RECEIVED + offset)
    }

    #[test]
    fn max_age_counts_from_the_age_and_wins_over_expires() {
        // tile.openstreetmap.org's headers.
        let f = freshness(
            "max-age=88823, stale-while-revalidate=604800, stale-if-error=604800",
            Some("Thu, 01 Jan 1970 00:00:00 GMT"),
            None,
            Some(67981),
            None,
        );
        assert_eq!(f.expires, at(88823 - 67981));
        assert!(!f.must_revalidate);
    }

    #[test]
    fn expires_counts_from_the_date() {
        // The server's clock is an hour ahead of ours.
        let server_now = RECEIVED + 3600;
        let expires = crate::storage::chrono::rfc1123(Timestamp::from_secs(server_now + 600));
        let f = freshness("public", Some(&expires), Some(server_now), Some(30), None);
        assert_eq!(f.expires, at(570));
        // Without a date it counts from our clock.
        let f = freshness("", Some(&expires), None, None, None);
        assert_eq!(f.expires, Timestamp::from_secs(server_now + 600));
        // An invalid date is in the past: stale at once.
        assert_eq!(freshness("", Some("0"), None, None, None).expires, at(0));
    }

    #[test]
    fn without_a_lifetime_a_heuristic_or_none() {
        let f = freshness("", None, None, None, Some(RECEIVED - 1000));
        assert_eq!(f.expires, at(100));
        // From the server's date.
        let f = freshness("", None, Some(RECEIVED + 1000), None, Some(RECEIVED - 1000));
        assert_eq!(f.expires, at(200));
        let f = freshness("", None, None, None, Some(0));
        assert_eq!(f.expires, at(MAX_HEURISTIC_LIFETIME as i64));
        assert_eq!(freshness("public", None, None, None, None).expires, at(0));
    }

    #[test]
    fn no_cache_is_stale_at_once_and_must_revalidate() {
        let f = freshness("no-cache, max-age=600", None, None, None, Some(0));
        assert_eq!(f.expires, at(0));
        assert!(f.must_revalidate);
    }
}
