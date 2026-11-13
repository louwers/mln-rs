// Ports src/mln/util/http_header.hpp and src/mln/util/http_header.cpp
// (`http::CacheControl`, `http::parseRetryHeaders`).
//
// The boost::spirit grammar of `CacheControl::parse` is ported as a small
// hand-written parser of the same grammar: a comma-separated list (spaces
// skipped) of `must-revalidate`, `max-age=<unsigned long long>` or any
// other token (quoted strings may contain commas). Like spirit's
// alternatives, `must-revalidate`/`max-age` match as prefixes of a longer
// directive, and the list stops at the first element that fails to parse.

use crate::chrono::{Timestamp, now, parse_http_date, parse_timestamp_i32};

/// `http::CacheControl`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub max_age: Option<u64>,
    pub must_revalidate: bool,
}

/// A cursor over the header value that skips ASCII spaces like
/// `qi::ascii::space` (the skipper of `qi::phrase_parse`).
struct Cursor<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn skip(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.pos).copied()
    }

    /// `qi::lit(s)` (pre-skipped, then matched byte by byte).
    fn lit(&mut self, lit: &str) -> bool {
        self.skip();
        if self.s[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    /// `qi::ulong_long`
    fn ulong_long(&mut self) -> Option<u64> {
        self.skip();
        let start = self.pos;
        let mut value: u64 = 0;
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_digit() {
            value = value.checked_mul(10)?.checked_add(u64::from(self.s[self.pos] - b'0'))?;
            self.pos += 1;
        }
        (self.pos > start).then_some(value)
    }
}

impl CacheControl {
    /// `CacheControl::parse(const std::string&)`
    #[must_use]
    pub fn parse(value: &str) -> CacheControl {
        let mut result = CacheControl::default();
        let mut c = Cursor { s: value.as_bytes(), pos: 0 };
        loop {
            // element: must-revalidate | max-age '=' ulong_long | other
            let start = c.pos;
            if c.lit("must-revalidate") {
                result.must_revalidate = true;
            } else {
                c.pos = start;
                let mut matched = false;
                if c.lit("max-age")
                    && c.lit("=")
                    && let Some(max_age) = c.ulong_long()
                {
                    result.max_age = Some(max_age);
                    matched = true;
                }
                if !matched {
                    c.pos = start;
                    // *( '"' *( '\\' char | char - '"' ) '"' | char - '"' - ',' )
                    loop {
                        match c.peek() {
                            Some(b'"') => {
                                let save = c.pos;
                                c.pos += 1;
                                let mut closed = false;
                                while c.pos < c.s.len() {
                                    match c.s[c.pos] {
                                        b'\\' if c.pos + 1 < c.s.len() => c.pos += 2,
                                        b'"' => {
                                            c.pos += 1;
                                            closed = true;
                                            break;
                                        }
                                        _ => c.pos += 1,
                                    }
                                }
                                if !closed {
                                    c.pos = save;
                                    break;
                                }
                            }
                            Some(b',') | None => break,
                            Some(_) => c.pos += 1,
                        }
                    }
                }
            }
            // `% ','`: another element follows a comma.
            if !c.lit(",") {
                break;
            }
        }
        result
    }

    /// `toTimePoint()`
    #[must_use]
    pub fn to_time_point(&self) -> Option<Timestamp> {
        self.max_age.map(|max_age| Timestamp::from_secs(now().secs_since_epoch().wrapping_add(max_age as i64)))
    }
}

/// `std::stoi`: leading whitespace, sign, digits; `None` where it throws.
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

/// `parseRetryHeaders(retryAfter, xRateLimitReset)`: when to retry a
/// rate-limited request, from `Retry-After` (seconds or an HTTP date) or else
/// `X-Rate-Limit-Reset` (a Unix time). `None` when the header it reads is
/// absent or doesn't parse.
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

    // gtest: HTTPHeader.Parsing
    #[test]
    fn parsing() {
        // Ports test/storage/headers.test.cpp `HTTPHeader.Parsing`.
        let cases: &[(&str, Option<u64>, bool)] = &[
            (r"", None, false),
            (r"max-age =34", Some(34), false),
            (r",max-age=1", Some(1), false),
            (r"max-age=-1", None, false),
            (r"max-age=foo", None, false),
            (r#"max-age="34,max-age="22,max-age=28"#, Some(28), false),
            (r#"max-age=3,max-age="34"#, Some(3), false),
            (r#"max-age="\",max-age=4,""#, None, false),
            (r"private, max-age=0, no-cache", Some(0), false),
            (r"max-age=0, no-cache, no-store", Some(0), false),
            (r#",   private   , max-bar=3 , no-cache,  "\,",,foo=",",,max-age=32"#, Some(32), false),
            (r"max-age=3600, must-revalidate", Some(3600), true),
            (r#"no-cache="Expires,Via",max-age=3600, must-revalidate"#, Some(3600), true),
        ];
        for &(value, max_age, must_revalidate) in cases {
            let cc = CacheControl::parse(value);
            assert_eq!(cc.max_age, max_age, "{value}");
            assert_eq!(cc.must_revalidate, must_revalidate, "{value}");
        }
    }
}
