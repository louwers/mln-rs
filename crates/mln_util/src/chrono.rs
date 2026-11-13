// Ports the wall-clock half of include/mln/util/chrono.hpp: `Timestamp` and
// its helpers (rfc1123, iso8601, parseTimestamp). Durations and the monotonic
// clock are std::time's `Duration` and crate::time's `Instant`.

/// `mln::Timestamp`: `std::chrono::time_point<std::chrono::system_clock,
/// Seconds>`, a signed count of seconds since the Unix epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// `Timestamp(Seconds(secs))`.
    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Timestamp(secs)
    }

    /// `time_since_epoch().count()`.
    #[must_use]
    pub const fn secs_since_epoch(self) -> i64 {
        self.0
    }
}

/// `util::now()` (`platform::now()`: `time_point_cast<Seconds>(system_clock::now())`,
/// which truncates toward zero).
#[must_use]
pub fn now() -> Timestamp {
    let secs = match crate::time::SystemTime::now().duration_since(crate::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    Timestamp(secs)
}

// Wall-clock formatting and parsing (src/mln/util/chrono.cpp; added by glfw).

/// `gmtime_r` fields for `rfc1123`: (weekday 0 = Sunday, mday, month 0-11,
/// year, hour, minute, second).
fn gmtime(secs: i64) -> (usize, i64, usize, i64, i64, i64, i64) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 1970-01-01 was a Thursday.
    let wday = (days + 4).rem_euclid(7) as usize;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (wday, mday, (month - 1) as usize, year, hour, min, sec)
}

/// `util::rfc1123(Timestamp)`: e.g. "Sun, 06 Nov 1994 08:49:37 GMT". Unlike
/// `httpdate::fmt_http_date`, which panics outside the years 1970 to 9999,
/// it formats any timestamp, such as a cached response's modification date.
#[must_use]
pub fn rfc1123(timestamp: Timestamp) -> String {
    const WEEK: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (wday, mday, month, year, hour, min, sec) = gmtime(timestamp.secs_since_epoch());
    format!("{}, {:02} {} {:4} {:02}:{:02}:{:02} GMT", WEEK[wday], mday, MONTHS[month], year, hour, min, sec)
}

/// `util::iso8601(Timestamp)`: e.g. "1994-11-06 08:49:37" (UTC).
#[must_use]
pub fn iso8601(timestamp: Timestamp) -> String {
    let (_, mday, month, year, hour, min, sec) = gmtime(timestamp.secs_since_epoch());
    format!("{year:04}-{:02}-{mday:02} {hour:02}:{min:02}:{sec:02}", month + 1)
}

/// An HTTP date (`Expires`, `Last-Modified`, `Retry-After`, ...) in any of
/// the three formats RFC 9110 §5.6.7 has recipients accept: IMF-fixdate
/// ("Sun, 06 Nov 1994 08:49:37 GMT"), RFC 850 and asctime. `None` when it is
/// none of them or falls outside the years 1970 to 9999.
#[must_use]
pub fn parse_http_date(date: &str) -> Option<Timestamp> {
    let since_epoch = httpdate::parse_http_date(date).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(Timestamp::from_secs(i64::try_from(since_epoch.as_secs()).ok()?))
}

/// `util::parseTimestamp(int32_t)`.
#[must_use]
pub fn parse_timestamp_i32(timestamp: i32) -> Timestamp {
    Timestamp::from_secs(i64::from(timestamp))
}

#[cfg(test)]
mod wall_clock_tests {
    use super::*;

    #[test]
    fn rfc1123_format() {
        // test/util/timestamp... the C++ formats with gmtime_r + snprintf.
        assert_eq!(rfc1123(Timestamp::from_secs(784_111_777)), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(rfc1123(Timestamp::from_secs(0)), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(rfc1123(Timestamp::from_secs(951_782_400)), "Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(
            parse_http_date(&rfc1123(Timestamp::from_secs(1_445_412_480))),
            Some(Timestamp::from_secs(1_445_412_480))
        );
    }

    #[test]
    fn http_dates() {
        let date = Some(Timestamp::from_secs(784_111_777));
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), date);
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), date);
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), date);
        assert_eq!(parse_http_date("Fri, 01 Jan 2100 00:00:00 GMT"), Some(Timestamp::from_secs(4_102_444_800)));
        assert_eq!(parse_http_date("Thu, 01 Jan 1960 00:00:00 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 +0100"), None);
        assert_eq!(parse_http_date("0"), None);
        assert_eq!(parse_http_date(""), None);
    }

    #[test]
    fn iso8601_format() {
        assert_eq!(iso8601(Timestamp::from_secs(784_111_777)), "1994-11-06 08:49:37");
        assert_eq!(iso8601(Timestamp::from_secs(0)), "1970-01-01 00:00:00");
        assert_eq!(iso8601(Timestamp::from_secs(951_782_400)), "2000-02-29 00:00:00");
    }
}
