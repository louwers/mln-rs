// Ports vendor/parsedate/parsedate.cpp (`parse_date`, curl's date parser as
// vendored by the C++ core): RFC 1123, RFC 850, asctime and the other
// free-form dates curl accepts, with the vendored copy's 32-bit `time_t`
// limits (years before 1970 give 0, years after 2037 give INT_MAX).
//
// Copyright (C) 1998 - 2018, Daniel Stenberg, <daniel@haxx.se>, et al.
// Licensed as described in LICENSE-curl (curl's COPYING).

const WKDAY: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTH: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// offset for daylight savings time
const T_DAYZONE: i32 = -60;

/// `tz[]`: name and offset in minutes.
const TZ: &[(&str, i32)] = &[
    ("GMT", 0),
    ("UT", 0),
    ("UTC", 0),
    ("WET", 0),
    ("BST", T_DAYZONE),
    ("WAT", 60),
    ("AST", 240),
    ("ADT", 240 + T_DAYZONE),
    ("EST", 300),
    ("EDT", 300 + T_DAYZONE),
    ("CST", 360),
    ("CDT", 360 + T_DAYZONE),
    ("MST", 420),
    ("MDT", 420 + T_DAYZONE),
    ("PST", 480),
    ("PDT", 480 + T_DAYZONE),
    ("YST", 540),
    ("YDT", 540 + T_DAYZONE),
    ("HST", 600),
    ("HDT", 600 + T_DAYZONE),
    ("CAT", 600),
    ("AHST", 600),
    ("NT", 660),
    ("IDLW", 720),
    ("CET", -60),
    ("MET", -60),
    ("MEWT", -60),
    ("MEST", -60 + T_DAYZONE),
    ("CEST", -60 + T_DAYZONE),
    ("MESZ", -60 + T_DAYZONE),
    ("FWT", -60),
    ("FST", -60 + T_DAYZONE),
    ("EET", -120),
    ("WAST", -420),
    ("WADT", -420 + T_DAYZONE),
    ("CCT", -480),
    ("JST", -540),
    ("EAST", -600),
    ("EADT", -600 + T_DAYZONE),
    ("GST", -600),
    ("NZT", -720),
    ("NZST", -720),
    ("NZDT", -720 + T_DAYZONE),
    ("IDLE", -720),
    // Military timezone names, with the signs of actual military usage.
    ("A", 60),
    ("B", 2 * 60),
    ("C", 3 * 60),
    ("D", 4 * 60),
    ("E", 5 * 60),
    ("F", 6 * 60),
    ("G", 7 * 60),
    ("H", 8 * 60),
    ("I", 9 * 60),
    ("K", 10 * 60),
    ("L", 11 * 60),
    ("M", 12 * 60),
    ("N", -60),
    ("O", -2 * 60),
    ("P", -3 * 60),
    ("Q", -4 * 60),
    ("R", -5 * 60),
    ("S", -6 * 60),
    ("T", -7 * 60),
    ("U", -8 * 60),
    ("V", -9 * 60),
    ("W", -10 * 60),
    ("X", -11 * 60),
    ("Y", -12 * 60),
    ("Z", 0),
];

const TIME_T_MIN: i64 = 0;
const TIME_T_MAX: i64 = i32::MAX as i64;

/// `strcasecompare`: ASCII case-insensitive equality.
fn strcasecompare(a: &[u8], b: &str) -> bool {
    a.eq_ignore_ascii_case(b.as_bytes())
}

/// `checkday`: 0 (Monday) to 6, or -1.
fn checkday(check: &[u8]) -> i32 {
    let what: &[&str; 7] = if check.len() > 3 { &WEEKDAY } else { &WKDAY };
    what.iter().position(|d| strcasecompare(check, d)).map_or(-1, |i| i as i32)
}

/// `checkmonth`: 0 to 11, or -1.
fn checkmonth(check: &[u8]) -> i32 {
    MONTH.iter().position(|m| strcasecompare(check, m)).map_or(-1, |i| i as i32)
}

/// `checktz`: the offset in seconds, or -1.
fn checktz(check: &[u8]) -> i32 {
    TZ.iter().find(|(name, _)| strcasecompare(check, name)).map_or(-1, |(_, offset)| offset * 60)
}

/// `sscanf(s, "%02d")`: optional whitespace and sign, then at most two
/// digits. Returns (value, bytes consumed).
fn scan_02d(s: &[u8]) -> Option<(i32, usize)> {
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut negative = false;
    let start = i;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let digits_start = i;
    let mut value = 0i32;
    // The field width (2) counts the sign.
    while i < s.len() && i - start < 2 && s[i].is_ascii_digit() {
        value = value * 10 + i32::from(s[i] - b'0');
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    Some((if negative { -value } else { value }, i))
}

/// `sscanf(s, "%02d:%02d:%02d%n")` (or two fields): the parsed fields and the
/// bytes consumed.
fn scan_time(s: &[u8], fields: usize) -> Option<(Vec<i32>, usize)> {
    let mut values = Vec::with_capacity(fields);
    let mut pos = 0;
    for field in 0..fields {
        if field > 0 {
            if s.get(pos) != Some(&b':') {
                return None;
            }
            pos += 1;
        }
        let (value, len) = scan_02d(&s[pos..])?;
        values.push(value);
        pos += len;
    }
    Some((values, pos))
}

/// `my_timegm`
fn my_timegm(sec: i32, min: i32, hour: i32, mday: i32, mon: i32, year: i32) -> i64 {
    const MONTH_DAYS_CUMULATIVE: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut year_ = year;
    let mut month = mon;
    if month < 0 {
        year_ += (11 - month) / 12;
        month = 11 - (11 - month) % 12;
    } else if month >= 12 {
        year_ -= month / 12;
        month %= 12;
    }
    let mut leap_days = year_ - i32::from(mon <= 1);
    leap_days = (leap_days / 4) - (leap_days / 100) + (leap_days / 400) - (1969 / 4) + (1969 / 100) - (1969 / 400);
    ((((i64::from(year_) - 1970) * 365
        + i64::from(leap_days)
        + i64::from(MONTH_DAYS_CUMULATIVE[month as usize])
        + i64::from(mday)
        - 1)
        * 24
        + i64::from(hour))
        * 60
        + i64::from(min))
        * 60
        + i64::from(sec)
}

enum Assume {
    Mday,
    Year,
}

/// `parsedate`: `Ok(t)` for `PARSEDATE_OK`, `Err(Some(t))` for
/// `PARSEDATE_SOONER/LATER`, `Err(None)` for `PARSEDATE_FAIL`.
fn parsedate(date: &[u8]) -> Result<i64, Option<i64>> {
    let mut wdaynum = -1;
    let mut monnum = -1;
    let mut mdaynum = -1;
    let mut hournum = -1;
    let mut minnum = -1;
    let mut secnum = -1;
    let mut yearnum = -1;
    let mut tzoff = -1;
    let mut dignext = Assume::Mday;
    let mut pos = 0usize;
    let mut part = 0;

    while pos < date.len() && date[pos] != 0 && part < 6 {
        let mut found = false;
        // skip(): everything that isn't a letter or digit
        while pos < date.len() && date[pos] != 0 && !date[pos].is_ascii_alphanumeric() {
            pos += 1;
        }
        if pos >= date.len() || date[pos] == 0 {
            // Neither a name nor a digit: the loop ends.
            part += 1;
            continue;
        }

        if date[pos].is_ascii_alphabetic() {
            // a name coming up: sscanf "%31[A-Za-z]"
            let mut len = 0;
            while pos + len < date.len() && len < 31 && date[pos + len].is_ascii_alphabetic() {
                len += 1;
            }
            let buf = &date[pos..pos + len];
            if wdaynum == -1 {
                wdaynum = checkday(buf);
                if wdaynum != -1 {
                    found = true;
                }
            }
            if !found && monnum == -1 {
                monnum = checkmonth(buf);
                if monnum != -1 {
                    found = true;
                }
            }
            if !found && tzoff == -1 {
                // this just must be a time zone string
                tzoff = checktz(buf);
                if tzoff != -1 {
                    found = true;
                }
            }
            if !found {
                return Err(None); // bad string
            }
            pos += len;
        } else {
            // a digit
            let rest = &date[pos..];
            if secnum == -1
                && let Some((v, len)) = scan_time(rest, 3)
            {
                // time stamp!
                (hournum, minnum, secnum) = (v[0], v[1], v[2]);
                pos += len;
            } else if secnum == -1
                && let Some((v, len)) = scan_time(rest, 2)
            {
                // time stamp without seconds
                (hournum, minnum) = (v[0], v[1]);
                pos += len;
                secnum = 0;
            } else {
                // strtol(date, &end, 10)
                let mut end = pos;
                let mut lval: i64 = 0;
                while end < date.len() && date[end].is_ascii_digit() {
                    lval = match lval.checked_mul(10).and_then(|v| v.checked_add(i64::from(date[end] - b'0'))) {
                        Some(v) => v,
                        None => return Err(None), // ERANGE
                    };
                    end += 1;
                }
                if lval > i64::from(i32::MAX) {
                    return Err(None);
                }
                let val = lval as i32;
                let digits = end - pos;

                if tzoff == -1
                    && digits == 4
                    && val <= 1400
                    && pos > 0
                    && (date[pos - 1] == b'+' || date[pos - 1] == b'-')
                {
                    // four digits preceded by a sign: a time zone indication
                    found = true;
                    tzoff = (val / 100 * 60 + val % 100) * 60;
                    // the + and - prefix indicates the local time compared
                    // to GMT, this we need their reversed math
                    if date[pos - 1] == b'+' {
                        tzoff = -tzoff;
                    }
                }

                if digits == 8 && yearnum == -1 && monnum == -1 && mdaynum == -1 {
                    // 8 digits, no year, month or day yet. This is YYYYMMDD
                    found = true;
                    yearnum = val / 10000;
                    monnum = (val % 10000) / 100 - 1; // month is 0 - 11
                    mdaynum = val % 100;
                }

                if !found && matches!(dignext, Assume::Mday) && mdaynum == -1 {
                    if val > 0 && val < 32 {
                        mdaynum = val;
                        found = true;
                    }
                    dignext = Assume::Year;
                }

                if !found && matches!(dignext, Assume::Year) && yearnum == -1 {
                    yearnum = val;
                    found = true;
                    if yearnum < 100 {
                        if yearnum > 70 {
                            yearnum += 1900;
                        } else {
                            yearnum += 2000;
                        }
                    }
                    if mdaynum == -1 {
                        dignext = Assume::Mday;
                    }
                }

                if !found {
                    return Err(None);
                }

                pos = end;
            }
        }

        part += 1;
    }

    if secnum == -1 {
        // no time, make it zero
        secnum = 0;
        minnum = 0;
        hournum = 0;
    }

    if mdaynum == -1 || monnum == -1 || yearnum == -1 {
        // lacks vital info, fail
        return Err(None);
    }

    if yearnum < 1970 {
        // only positive numbers cannot return earlier
        return Err(Some(TIME_T_MIN));
    }

    // a signed 32 bit time_t can only hold dates to the beginning of 2038
    if yearnum > 2037 {
        return Err(Some(TIME_T_MAX));
    }

    if mdaynum > 31 || monnum > 11 || hournum > 23 || minnum > 59 || secnum > 60 {
        return Err(None); // clearly an illegal date
    }

    let mut t = my_timegm(secnum, minnum, hournum, mdaynum, monnum, yearnum);

    // Add the time zone diff between local time zone and GMT.
    if tzoff == -1 {
        tzoff = 0;
    }

    if tzoff > 0 && t > TIME_T_MAX - i64::from(tzoff) {
        return Err(Some(TIME_T_MAX)); // time_t overflow
    }

    t += i64::from(tzoff);
    Ok(t)
}

/// `parse_date(const char*)`: seconds since the epoch, or -1 on failure.
#[must_use]
pub fn parse_date(p: &str) -> i64 {
    match parsedate(p.as_bytes()) {
        // avoid returning -1 for a working scenario
        Ok(-1) => 0,
        Ok(parsed) => parsed,
        // everything else is fail
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_date;

    #[test]
    fn http_formats() {
        // RFC 1123, RFC 850, asctime
        assert_eq!(parse_date("Sun, 06 Nov 1994 08:49:37 GMT"), 784_111_777);
        assert_eq!(parse_date("Sunday, 06-Nov-94 08:49:37 GMT"), 784_111_777);
        assert_eq!(parse_date("Sun Nov  6 08:49:37 1994"), 784_111_777);
        assert_eq!(parse_date("Wed, 21 Oct 2015 07:28:00 GMT"), 1_445_412_480);
    }

    #[test]
    fn zones_and_limits() {
        assert_eq!(parse_date("Sun, 06 Nov 1994 08:49:37 +0100"), 784_111_777 - 3600);
        assert_eq!(parse_date("Sun, 06 Nov 1994 08:49:37 PST"), 784_111_777 + 8 * 3600);
        assert_eq!(parse_date("20151021"), 1_445_385_600);
        assert_eq!(parse_date("Thu, 01 Jan 1960 00:00:00 GMT"), -1);
        assert_eq!(parse_date("Fri, 01 Jan 2100 00:00:00 GMT"), -1);
        assert_eq!(parse_date("not a date"), -1);
        assert_eq!(parse_date(""), -1);
    }
}
