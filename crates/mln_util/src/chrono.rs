// Ports include/mln/util/chrono.hpp: `Clock` (= std::chrono::steady_clock),
// `TimePoint`, `Duration`, `Seconds`, `Milliseconds`. The wall-clock
// timestamp helpers (rfc1123, iso8601, parseTimestamp) arrive with storage.
//
// `Duration` is `steady_clock::duration`, i.e. `std::chrono::nanoseconds`: a
// *signed* 64-bit count of nanoseconds. `TimePoint` is a time since the clock's
// epoch in the same unit. C++ signed overflow is undefined; the arithmetic here
// wraps, which is what the C++ code compiles to on our targets (it can happen
// with `TimePoint::max()`, which `PropertyEvaluationParameters(z)` uses).

use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// `mln::Duration` (`std::chrono::nanoseconds`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Duration(i64);

impl Duration {
    pub const fn zero() -> Self {
        Duration(0)
    }

    pub const fn max() -> Self {
        Duration(i64::MAX)
    }

    pub const fn min() -> Self {
        Duration(i64::MIN)
    }

    pub const fn from_nanos(nanos: i64) -> Self {
        Duration(nanos)
    }

    /// `Milliseconds(ms)` converted to `Duration`.
    pub const fn from_millis(millis: i64) -> Self {
        Duration(millis.wrapping_mul(1_000_000))
    }

    /// `Seconds(s)` converted to `Duration`.
    pub const fn from_secs(secs: i64) -> Self {
        Duration(secs.wrapping_mul(1_000_000_000))
    }

    /// The tick count (nanoseconds).
    pub const fn count(self) -> i64 {
        self.0
    }

    /// `std::chrono::duration_cast<std::chrono::milliseconds>(d).count()`
    /// (truncates toward zero).
    pub const fn as_millis(self) -> i64 {
        self.0 / 1_000_000
    }

    /// `std::chrono::duration<float>(d).count()`: libstdc++'s duration_cast
    /// computes `float(count) / float(1e9)`.
    pub fn as_secs_f32(self) -> f32 {
        self.0 as f32 / 1_000_000_000.0_f32
    }

    /// `std::chrono::duration<float>(lhs) / rhs` where `rhs` is a `Duration`,
    /// as `Transitioning::evaluate` (style/properties.hpp) computes its
    /// progress: both operands go to the common type
    /// `duration<float, nano>`, so the float seconds are scaled back by 1e9
    /// before the float division.
    pub fn float_seconds_div(lhs: Duration, rhs: Duration) -> f32 {
        let lhs_ns = lhs.as_secs_f32() * 1_000_000_000.0_f32;
        let rhs_ns = rhs.0 as f32;
        lhs_ns / rhs_ns
    }
}

impl Add for Duration {
    type Output = Duration;
    fn add(self, rhs: Duration) -> Duration {
        Duration(self.0.wrapping_add(rhs.0))
    }
}

impl Sub for Duration {
    type Output = Duration;
    fn sub(self, rhs: Duration) -> Duration {
        Duration(self.0.wrapping_sub(rhs.0))
    }
}

impl Neg for Duration {
    type Output = Duration;
    fn neg(self) -> Duration {
        Duration(self.0.wrapping_neg())
    }
}

impl AddAssign for Duration {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl SubAssign for Duration {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

/// `mln::Seconds(n)`.
pub const fn seconds(n: i64) -> Duration {
    Duration::from_secs(n)
}

/// `mln::Milliseconds(n)`.
pub const fn milliseconds(n: i64) -> Duration {
    Duration::from_millis(n)
}

/// `mln::TimePoint` (`steady_clock::time_point`). The default value is the
/// clock's epoch, as for a default-constructed `time_point`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TimePoint(Duration);

impl TimePoint {
    /// `TimePoint(d)`: the epoch plus `d`.
    pub const fn new(since_epoch: Duration) -> Self {
        TimePoint(since_epoch)
    }

    pub const fn time_since_epoch(self) -> Duration {
        self.0
    }

    /// `Clock::time_point::max()`.
    pub const fn max() -> Self {
        TimePoint(Duration::max())
    }

    /// `Clock::time_point::min()`.
    pub const fn min() -> Self {
        TimePoint(Duration::min())
    }
}

impl Add<Duration> for TimePoint {
    type Output = TimePoint;
    fn add(self, rhs: Duration) -> TimePoint {
        TimePoint(self.0 + rhs)
    }
}

impl Sub<Duration> for TimePoint {
    type Output = TimePoint;
    fn sub(self, rhs: Duration) -> TimePoint {
        TimePoint(self.0 - rhs)
    }
}

impl Sub for TimePoint {
    type Output = Duration;
    fn sub(self, rhs: TimePoint) -> Duration {
        self.0 - rhs.0
    }
}

impl AddAssign<Duration> for TimePoint {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}

impl SubAssign<Duration> for TimePoint {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

/// `mln::Clock` (`std::chrono::steady_clock`).
pub struct Clock;

impl Clock {
    /// `Clock::now()`. Monotonic. std exposes no raw CLOCK_MONOTONIC reading,
    /// so the epoch is placed at the wall-clock time of the first call; like
    /// the C++ steady clock, values are then far from the epoch (code that
    /// compares against `TimePoint(Duration::zero())`, such as
    /// `ZoomHistory`, sees a long-past time, as in C++).
    pub fn now() -> TimePoint {
        static ANCHOR: OnceLock<(crate::time::Instant, i64)> = OnceLock::new();
        let (instant, base) = ANCHOR.get_or_init(|| {
            let since_unix = crate::time::SystemTime::now()
                .duration_since(crate::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            (crate::time::Instant::now(), since_unix)
        });
        let elapsed = instant.elapsed().as_nanos() as i64;
        TimePoint(Duration(base.wrapping_add(elapsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let t = TimePoint::new(Duration::from_millis(5));
        assert_eq!((t + Duration::from_millis(3)) - t, Duration::from_millis(3));
        assert_eq!(Duration::from_millis(1500).as_millis(), 1500);
        assert_eq!(Duration::from_nanos(-1_500_000).as_millis(), -1);
        assert!(TimePoint::max() > Clock::now());
    }

    #[test]
    fn float_seconds_div() {
        let t = Duration::float_seconds_div(Duration::from_millis(500), Duration::from_millis(1000));
        assert_eq!(t, 0.5);
    }

    #[test]
    fn now_is_monotonic() {
        let a = Clock::now();
        let b = Clock::now();
        assert!(b >= a);
        assert!(a.time_since_epoch() > Duration::from_secs(1));
    }
}

// Wall-clock timestamps (chrono.hpp, added by storage).

/// `mln::Timestamp`: `std::chrono::time_point<std::chrono::system_clock,
/// Seconds>`, a signed count of seconds since the Unix epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// `Timestamp(Seconds(secs))`.
    pub const fn from_secs(secs: i64) -> Self {
        Timestamp(secs)
    }

    /// `time_since_epoch().count()`.
    pub const fn secs_since_epoch(self) -> i64 {
        self.0
    }
}

/// `util::now()` (`platform::now()`: `time_point_cast<Seconds>(system_clock::now())`,
/// which truncates toward zero).
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
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (wday, mday, (month - 1) as usize, year, hour, min, sec)
}

/// `util::rfc1123(Timestamp)`: e.g. "Sun, 06 Nov 1994 08:49:37 GMT".
pub fn rfc1123(timestamp: Timestamp) -> String {
    const WEEK: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (wday, mday, month, year, hour, min, sec) = gmtime(timestamp.secs_since_epoch());
    format!("{}, {:02} {} {:4} {:02}:{:02}:{:02} GMT", WEEK[wday], mday, MONTHS[month], year, hour, min, sec)
}

/// `util::parseTimestamp(const char*)`: vendor/parsedate's `parse_date`
/// (-1 when the date does not parse).
pub fn parse_timestamp_str(timestamp: &str) -> Timestamp {
    Timestamp::from_secs(crate::parsedate::parse_date(timestamp))
}

/// `util::parseTimestamp(int32_t)`.
pub fn parse_timestamp_i32(timestamp: i32) -> Timestamp {
    Timestamp::from_secs(i64::from(timestamp))
}

#[cfg(test)]
mod wall_clock_tests {
    use super::*;

    #[test]
    fn rfc1123_format() {
        // test/util/timestamp... the C++ formats with gmtime_r + snprintf.
        assert_eq!(rfc1123(Timestamp::from_secs(784111777)), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(rfc1123(Timestamp::from_secs(0)), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(rfc1123(Timestamp::from_secs(951782400)), "Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(parse_timestamp_str(&rfc1123(Timestamp::from_secs(1445412480))), Timestamp::from_secs(1445412480));
    }
}
