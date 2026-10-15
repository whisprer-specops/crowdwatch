// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Time as Python's `datetime` handles it.
//!
//! CrowdWatch keys observations by their UTC timestamps, written with microseconds and a
//! `Z`. Every moment here is a UTC `Instant` in Python's range (years 1 to 9999). Parsing
//! is a port of CPython 3.14's `datetime.fromisoformat` (`Modules/_datetimemodule.c`), and
//! arithmetic, timestamps and rounding follow CPython too, so 0.2.0 reads and writes the
//! same times as 0.1.0 did on Python 3.14.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Fail, Result};
use crate::json::Value;
use crate::pyfmt::is_space;

/// Microseconds in a day.
const DAY_US: i64 = 86_400_000_000;
/// Days from 0001-01-01 to 1970-01-01.
const UNIX_EPOCH_DAYS: i64 = 719_162;
/// The last microsecond Python's datetime can hold: 9999-12-31T23:59:59.999999.
const MAX_US: i64 = 3_652_059 * DAY_US - 1;

/// A moment in UTC: microseconds since 0001-01-01T00:00:00.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(i64);

/// A span of time in microseconds (Python's `timedelta`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span(pub i128);

fn out_of_range() -> Fail {
    Fail::overflow("date value out of range")
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const DAYS_BEFORE_MONTH: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Days from 0001-01-01 to the given date (Python's `toordinal() - 1`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - 1;
    let mut days = y * 365 + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400);
    days += DAYS_BEFORE_MONTH[month as usize];
    if month > 2 && is_leap(year) {
        days += 1;
    }
    days + day - 1
}

/// The date `days` days after 0001-01-01 (Python's `_ord2ymd`), for any integer.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    const DI400Y: i64 = 146_097;
    const DI100Y: i64 = 36_524;
    const DI4Y: i64 = 1_461;
    let n400 = days.div_euclid(DI400Y);
    let mut n = days.rem_euclid(DI400Y);
    let mut year = n400 * 400 + 1;
    let n100 = n / DI100Y;
    n %= DI100Y;
    let n4 = n / DI4Y;
    n %= DI4Y;
    let n1 = n / 365;
    n %= 365;
    year += n100 * 100 + n4 * 4 + n1;
    if n1 == 4 || n100 == 4 {
        return (year - 1, 12, 31);
    }
    let leap = n1 == 3 && (n4 != 24 || n100 == 3);
    let mut month = (n + 50) >> 5;
    let mut preceding = DAYS_BEFORE_MONTH[month as usize] + i64::from(month > 2 && leap);
    if preceding > n {
        month -= 1;
        preceding -= days_in_month(if leap { 4 } else { 1 }, month);
    }
    n -= preceding;
    (year, month, n + 1)
}

impl Instant {
    fn checked(us: i128) -> Result<Instant> {
        if (0..=i128::from(MAX_US)).contains(&us) {
            Ok(Instant(us as i64))
        } else {
            Err(out_of_range())
        }
    }

    /// A UTC date and time; `None` outside Python's range.
    pub fn from_parts(
        year: i64,
        month: i64,
        day: i64,
        hour: i64,
        minute: i64,
        second: i64,
        micro: i64,
    ) -> Option<Instant> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || !(0..24).contains(&hour)
            || !(0..60).contains(&minute)
            || !(0..60).contains(&second)
            || !(0..1_000_000).contains(&micro)
        {
            return None;
        }
        let days = days_from_civil(year, month, day);
        Some(Instant(
            days * DAY_US + ((hour * 60 + minute) * 60 + second) * 1_000_000 + micro,
        ))
    }

    /// Year, month, day, hour, minute, second and microsecond.
    pub fn parts(self) -> (i64, i64, i64, i64, i64, i64, i64) {
        let days = self.0.div_euclid(DAY_US);
        let rest = self.0.rem_euclid(DAY_US);
        let (year, month, day) = civil_from_days(days);
        let micro = rest % 1_000_000;
        let seconds = rest / 1_000_000;
        (
            year,
            month,
            day,
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60,
            micro,
        )
    }

    /// The current time, to the microsecond.
    pub fn now() -> Instant {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let us = i64::try_from(since.as_micros()).unwrap_or(i64::MAX);
        Instant((UNIX_EPOCH_DAYS * DAY_US).saturating_add(us).min(MAX_US))
    }

    /// `stamp()`: `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
    pub fn stamp(self) -> String {
        let (y, mo, d, h, mi, s, us) = self.parts();
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{us:06}Z")
    }

    /// `strftime("%Y-%m-%dT%H:%M:%SZ")`.
    pub fn strftime_seconds(self) -> String {
        let (y, mo, d, h, mi, s, _) = self.parts();
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    }

    /// `datetime + timedelta`. Not the `+` operator: like Python, it fails with an
    /// OverflowError outside years 1 to 9999.
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, span: Span) -> Result<Instant> {
        Instant::checked(i128::from(self.0) + span.0)
    }

    /// `datetime - timedelta`, with the same OverflowError.
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, span: Span) -> Result<Instant> {
        Instant::checked(i128::from(self.0) - span.0)
    }

    /// `self - other`.
    pub fn since(self, other: Instant) -> Span {
        Span(i128::from(self.0) - i128::from(other.0))
    }

    /// `weekday()`: Monday is 0.
    pub fn weekday(self) -> i64 {
        self.0.div_euclid(DAY_US) % 7
    }

    /// The date part, for comparing days.
    pub fn date(self) -> i64 {
        self.0.div_euclid(DAY_US)
    }

    /// Minutes since midnight (`hour * 60 + minute`).
    pub fn minute_of_day(self) -> i64 {
        self.0.rem_euclid(DAY_US) / 60_000_000
    }

    /// `timestamp()`: POSIX seconds as a float.
    pub fn timestamp(self) -> f64 {
        self.since(Instant(UNIX_EPOCH_DAYS * DAY_US))
            .total_seconds()
    }

    /// The truncation `replace(second=0, microsecond=0)`.
    pub fn to_minute(self) -> Instant {
        Instant(self.0 - self.0.rem_euclid(60_000_000))
    }
}

impl Span {
    /// `total_seconds()`: the exact microseconds divided by a million, correctly rounded.
    pub fn total_seconds(self) -> f64 {
        let us = self.0;
        if us.unsigned_abs() < (1u128 << 53) {
            return us as f64 / 1e6;
        }
        let sign = if us < 0 { "-" } else { "" };
        let magnitude = us.unsigned_abs();
        format!(
            "{sign}{}.{:06}",
            magnitude / 1_000_000,
            magnitude % 1_000_000
        )
        .parse()
        .unwrap_or(f64::NAN)
    }

    pub fn seconds(seconds: i64) -> Span {
        Span(i128::from(seconds) * 1_000_000)
    }

    pub fn minutes(minutes: i64) -> Span {
        Span(i128::from(minutes) * 60_000_000)
    }

    pub fn hours(hours: i64) -> Span {
        Span(i128::from(hours) * 3_600_000_000)
    }

    pub fn days(days: i64) -> Span {
        Span(i128::from(days) * i128::from(DAY_US))
    }
}

/// A Python number used as a `timedelta` argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    /// The number as a JSON value, keeping its Python type.
    pub fn to_value(self) -> Value {
        match self {
            Num::Int(i) => Value::Int(i),
            Num::Float(f) => Value::Float(f),
        }
    }

    /// A configuration value (already checked to be a finite number) as a `Num`.
    pub fn from_value(value: &Value) -> Num {
        match value {
            Value::Int(i) => Num::Int(*i),
            Value::Bool(b) => Num::Int(i64::from(*b)),
            other => Num::Float(other.py_float().unwrap_or(f64::NAN)),
        }
    }
}

/// CPython's `timedelta(**{unit: value})` for one argument: integers exactly, floats split
/// into whole and fractional parts with the leftover rounded half to even.
pub fn span_of(value: Num, unit_us: i64) -> Result<Span> {
    const MAX_DAYS: i128 = 999_999_999;
    let us: i128 = match value {
        Num::Int(i) => i128::from(i) * i128::from(unit_us),
        Num::Float(f) => {
            if f.is_nan() {
                return Err(Fail::value_error("cannot convert float NaN to integer"));
            }
            if f.is_infinite() {
                return Err(Fail::overflow("cannot convert float infinity to integer"));
            }
            let whole = f.trunc();
            let fraction = f - whole;
            if whole.abs() > 1e30 {
                return Err(Fail::overflow("Python int too large to convert to C int"));
            }
            let mut total = (whole as i128) * i128::from(unit_us);
            if fraction != 0.0 {
                let scaled = unit_us as f64 * fraction;
                let scaled_whole = scaled.trunc();
                total += scaled_whole as i128;
                let leftover = scaled - scaled_whole;
                if leftover != 0.0 {
                    let mut rounded = leftover.round();
                    if (rounded - leftover).abs() == 0.5 {
                        let odd = f64::from(u8::from(total.rem_euclid(2) == 1));
                        rounded = 2.0 * ((leftover + odd) * 0.5).round() - odd;
                    }
                    total += rounded as i128;
                }
            }
            total
        }
    };
    let days = us.div_euclid(i128::from(DAY_US));
    if days.abs() > MAX_DAYS {
        return Err(Fail::overflow(if i32::try_from(days).is_ok() {
            format!("days={days}; must have magnitude <= 999999999")
        } else {
            "Python int too large to convert to C int".to_owned()
        }));
    }
    Ok(Span(us))
}

// ---------------------------------------------------------------------------------------
// Parsing: datetime.fromisoformat (CPython 3.14)
// ---------------------------------------------------------------------------------------

/// A parsed ISO 8601 date and time; `offset_us` is set when a UTC offset was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub micro: i64,
    pub offset_us: Option<i64>,
}

impl Parsed {
    /// `astimezone(UTC)` of an aware datetime.
    pub fn to_utc(self) -> Result<Instant> {
        let local = Instant::from_parts(
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.micro,
        )
        .ok_or_else(out_of_range)?;
        Instant::checked(i128::from(local.0) - i128::from(self.offset_us.unwrap_or(0)))
    }
}

/// The C parser reads the NUL byte after the end of the string.
fn at(bytes: &[u8], index: usize) -> u8 {
    bytes.get(index).copied().unwrap_or(0)
}

fn digits(bytes: &[u8], pos: &mut usize, count: usize) -> Option<i64> {
    let mut value = 0i64;
    for _ in 0..count {
        let digit = at(bytes, *pos).wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        *pos += 1;
        value = value * 10 + i64::from(digit);
    }
    Some(value)
}

/// Python 3.14's `datetime.fromisoformat(text)`; `None` where Python raises ValueError.
pub fn fromisoformat(text: &str) -> Option<Parsed> {
    if text.chars().count() < 7 {
        return None;
    }
    let bytes = text.as_bytes();
    let len = bytes.len();
    let separator = find_separator(bytes)?;
    let (mut year, mut month, mut day) = parse_date(bytes, separator)?;
    let mut time = Clock::default();
    if len > separator {
        let width = match bytes[separator] {
            b if b & 0x80 == 0 => 1,
            b if b & 0xf0 == 0xe0 => 3,
            b if b & 0xf0 == 0xf0 => 4,
            _ => 2,
        };
        let start = (separator + width).min(len);
        time = parse_clock(&bytes[start..])?;
    }
    if let Some(offset) = time.offset_us {
        if offset <= -DAY_US || offset >= DAY_US {
            return None;
        }
    }
    let mut hour = time.hour;
    if hour == 24 && (1..=12).contains(&month) {
        let month_days = days_in_month(year, month);
        if day <= month_days {
            if time.minute != 0 || time.second != 0 || time.micro != 0 {
                return None;
            }
            hour = 0;
            day += 1;
            if day > month_days {
                day = 1;
                month += 1;
                if month > 12 {
                    month = 1;
                    year += 1;
                }
            }
        }
    }
    Instant::from_parts(year, month, day, hour, time.minute, time.second, time.micro)?;
    Some(Parsed {
        year,
        month,
        day,
        hour,
        minute: time.minute,
        second: time.second,
        micro: time.micro,
        offset_us: time.offset_us,
    })
}

#[derive(Default)]
struct Clock {
    hour: i64,
    minute: i64,
    second: i64,
    micro: i64,
    offset_us: Option<i64>,
}

/// `_find_isoformat_datetime_separator`.
fn find_separator(s: &[u8]) -> Option<usize> {
    let len = s.len();
    if len == 7 {
        return Some(7);
    }
    if at(s, 4) == b'-' {
        if at(s, 5) == b'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && s[8] == b'-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && s[10].is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if at(s, 4) == b'W' {
        let mut idx = 7;
        while idx < len && s[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// `parse_isoformat_date`.
fn parse_date(s: &[u8], len: usize) -> Option<(i64, i64, i64)> {
    let mut pos = 0;
    let year = digits(s, &mut pos, 4)?;
    let uses_separator = at(s, pos) == b'-';
    if uses_separator {
        pos += 1;
    }
    if at(s, pos) == b'W' {
        pos += 1;
        let week = digits(s, &mut pos, 2)?;
        let weekday = if pos < len {
            if uses_separator {
                if at(s, pos) != b'-' {
                    return None;
                }
                pos += 1;
            }
            digits(s, &mut pos, 1)?
        } else {
            1
        };
        return iso_to_ymd(year, week, weekday);
    }
    let month = digits(s, &mut pos, 2)?;
    if uses_separator {
        if at(s, pos) != b'-' {
            return None;
        }
        pos += 1;
    }
    let day = digits(s, &mut pos, 2)?;
    Some((year, month, day))
}

/// `parse_isoformat_time`.
fn parse_clock(t: &[u8]) -> Option<Clock> {
    let end = t.len();
    let mut tz = 0;
    loop {
        if matches!(at(t, tz), b'Z' | b'+' | b'-') {
            break;
        }
        tz += 1;
        if tz >= end {
            break;
        }
    }
    let (clock, more) = parse_hh_mm_ss_ff(t, 0, tz)?;
    let mut result = Clock {
        hour: clock[0],
        minute: clock[1],
        second: clock[2],
        micro: clock[3],
        offset_us: None,
    };
    if tz >= end {
        return if more { None } else { Some(result) };
    }
    if t[tz] == b'Z' {
        if at(t, tz + 1) != 0 {
            return None;
        }
        result.offset_us = Some(0);
        return Some(result);
    }
    let sign: i64 = if t[tz] == b'-' { -1 } else { 1 };
    let (offset, more) = parse_hh_mm_ss_ff(t, tz + 1, end)?;
    if more {
        return None;
    }
    let seconds = offset[0] * 3600 + offset[1] * 60 + offset[2];
    result.offset_us = Some(sign * (seconds * 1_000_000 + offset[3]));
    Some(result)
}

/// `parse_hh_mm_ss_ff`, between `start` and `end`. The flag says whether anything other
/// than the end of the string follows.
fn parse_hh_mm_ss_ff(t: &[u8], start: usize, end: usize) -> Option<([i64; 4], bool)> {
    let mut values = [0i64; 4];
    let mut pos = start;
    let mut has_separator = true;
    let mut index = 0;
    while index < 3 {
        values[index] = digits(t, &mut pos, 2)?;
        let c = at(t, pos);
        pos += 1;
        if index == 0 {
            has_separator = c == b':';
        }
        if c == b'.' || c == b',' {
            if index < 2 || pos >= end {
                return None;
            }
            break;
        } else if pos >= end {
            return Some((values, c != 0));
        } else if has_separator && c == b':' {
            if index == 2 {
                return None;
            }
        } else if !has_separator {
            pos -= 1;
        } else {
            return None;
        }
        index += 1;
    }
    let to_parse = end.saturating_sub(pos).min(6);
    if to_parse == 0 {
        return None;
    }
    let mut fraction = digits(t, &mut pos, to_parse)?;
    const CORRECTION: [i64; 5] = [100_000, 10_000, 1_000, 100, 10];
    if to_parse < 6 {
        fraction *= CORRECTION[to_parse - 1];
    }
    values[3] = fraction;
    while at(t, pos).is_ascii_digit() {
        pos += 1;
    }
    Some((values, at(t, pos) != 0))
}

/// `iso_to_ymd`: an ISO year, week and weekday to a calendar date.
fn iso_to_ymd(iso_year: i64, week: i64, weekday: i64) -> Option<(i64, i64, i64)> {
    if !(1..=9999).contains(&iso_year) {
        return None;
    }
    let january_first = days_from_civil(iso_year, 1, 1);
    let first_weekday = january_first % 7;
    if week <= 0 || week >= 53 {
        let long_year = first_weekday == 3 || (first_weekday == 2 && is_leap(iso_year));
        if week != 53 || !long_year {
            return None;
        }
    }
    if !(1..=7).contains(&weekday) {
        return None;
    }
    let mut week1_monday = january_first - first_weekday;
    if first_weekday > 3 {
        week1_monday += 7;
    }
    Some(civil_from_days(week1_monday + (week - 1) * 7 + weekday - 1))
}

// ---------------------------------------------------------------------------------------
// CrowdWatch's parse_time and stamp
// ---------------------------------------------------------------------------------------

/// 0.1.0's `parse_time(value)` for text: ISO 8601 with `Z` or an explicit offset, in UTC.
pub fn parse_time_str(text: &str) -> Result<Instant> {
    let parsed = fromisoformat(&text.replace('Z', "+00:00"))
        .ok_or_else(|| Fail::input("timestamp must be an ISO 8601 time with a UTC offset"))?;
    if parsed.offset_us.is_none() {
        return Err(Fail::input(
            "timestamp requires Z or an explicit UTC offset",
        ));
    }
    parsed.to_utc()
}

/// 0.1.0's `parse_time(value)` for a JSON value: only strings are times.
pub fn parse_time(value: &Value) -> Result<Instant> {
    match value {
        Value::Str(text) => parse_time_str(text),
        _ => Err(Fail::input("timestamp must be an ISO 8601 string")),
    }
}

/// 0.1.0's `stamp(text)`.
pub fn stamp_str(text: &str) -> Result<String> {
    Ok(parse_time_str(text)?.stamp())
}

/// Python's `datetime.fromtimestamp(t, timezone.utc)` for a JSON number, with the
/// exception CPython raises on Linux for each kind of bad value.
pub fn from_timestamp(value: &Value) -> Result<Instant> {
    const TIME_T_OVERFLOW: &str = "timestamp out of range for platform time_t";
    let (seconds, micro): (i128, i64) = match value {
        Value::Int(i) => (i128::from(*i), 0),
        Value::Bool(b) => (i128::from(*b), 0),
        Value::BigInt(_) => return Err(Fail::overflow(TIME_T_OVERFLOW)),
        Value::Float(f) => {
            if f.is_nan() {
                return Err(Fail::value_error("Invalid value NaN (not a number)"));
            }
            let mut whole = f.trunc();
            let mut fraction = round_half_even((f - whole) * 1e6);
            if fraction >= 1e6 {
                fraction -= 1e6;
                whole += 1.0;
            } else if fraction < 0.0 {
                fraction += 1e6;
                whole -= 1.0;
            }
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&whole) {
                return Err(Fail::overflow(TIME_T_OVERFLOW));
            }
            (whole as i128, fraction as i64)
        }
        other => {
            return Err(Fail::type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                other.type_name()
            )))
        }
    };
    let days = seconds.div_euclid(86_400);
    let year_estimate = days / 365 + 1970;
    // glibc's gmtime fails once the year no longer fits a C int.
    if year_estimate.abs() > i128::from(i32::MAX) {
        return Err(Fail::py(
            "OSError",
            "[Errno 75] Value too large for defined data type",
        ));
    }
    let days = days as i64 + UNIX_EPOCH_DAYS;
    let (year, _, _) = civil_from_days(days);
    if !(1..=9999).contains(&year) {
        return Err(Fail::value_error(format!("year {year} is out of range")));
    }
    let rest = seconds.rem_euclid(86_400) as i64;
    Ok(Instant(days * DAY_US + rest * 1_000_000 + micro))
}

/// CPython's `_PyTime_RoundHalfEven`.
fn round_half_even(x: f64) -> f64 {
    let rounded = x.round();
    if (x - rounded).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        rounded
    }
}

// ---------------------------------------------------------------------------------------
// email.utils.parsedate_to_datetime (Python 3.13 and 3.14)
// ---------------------------------------------------------------------------------------

/// The result of `parsedate_to_datetime`: a naive time, or an aware one given as
/// microseconds since 0001-01-01 UTC. An aware time near the ends of Python's range can lie
/// outside it once moved to UTC, as in Python, where only `astimezone` then fails.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MailDate {
    Naive,
    Aware(i128),
}

impl MailDate {
    /// `astimezone(timezone.utc)`; `None` for a naive time.
    pub fn to_utc(self) -> Result<Option<Instant>> {
        match self {
            MailDate::Naive => Ok(None),
            MailDate::Aware(us) => Instant::checked(us).map(Some),
        }
    }

    /// `(self - other).total_seconds()` for an aware time.
    pub fn seconds_since(self, other: Instant) -> Option<f64> {
        match self {
            MailDate::Naive => None,
            MailDate::Aware(us) => Some(Span(us - i128::from(other.0)).total_seconds()),
        }
    }
}

const MONTHS: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYNAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn zone_offset(zone: &str) -> Option<i64> {
    Some(match zone {
        "UT" | "UTC" | "GMT" | "Z" => 0,
        "AST" => -400,
        "ADT" => -300,
        "EST" => -500,
        "EDT" => -400,
        "CST" => -600,
        "CDT" => -500,
        "MST" => -700,
        "MDT" => -600,
        "PST" => -800,
        "PDT" => -700,
        _ => return None,
    })
}

/// Python's `int(text)` as `_parsedate_tz` uses it: `None` on ValueError; very large
/// numbers saturate, which only matters for the range checks that follow.
fn py_int_lenient(text: &str) -> Option<i128> {
    let digits = crate::pyfmt::int_from_str(text).ok()?;
    Some(
        digits
            .parse::<i128>()
            .unwrap_or(if digits.starts_with('-') {
                i128::MIN / 4
            } else {
                i128::MAX / 4
            }),
    )
}

/// Python's `str.isdigit()` for a first character.
fn is_digit_char(c: char) -> bool {
    crate::pyfmt::is_digit(c)
}

/// `email.utils._parsedate_tz`: (year, month, day, hour, minute, second, offset seconds).
#[allow(clippy::type_complexity)]
fn parsedate_tz(data: &str) -> Option<(i128, i128, i128, i128, i128, i128, Option<i128>)> {
    if data.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = data
        .split(is_space)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    if parts.is_empty() {
        return None;
    }
    if parts[0].ends_with(',') || DAYNAMES.contains(&parts[0].to_lowercase().as_str()) {
        parts.remove(0);
    } else if let Some(i) = parts[0].rfind(',') {
        parts[0] = parts[0][i + 1..].to_owned();
    }
    if parts.len() == 3 {
        let stuff: Vec<String> = parts[0].split('-').map(str::to_owned).collect();
        if stuff.len() == 3 {
            let mut joined = stuff;
            joined.extend(parts[1..].iter().cloned());
            parts = joined;
        }
    }
    if parts.len() == 4 {
        let s = parts[3].clone();
        let i = s.find('+').or_else(|| s.find('-'));
        match i {
            Some(i) if i > 0 => {
                parts.truncate(3);
                parts.push(s[..i].to_owned());
                parts.push(s[i..].to_owned());
            }
            _ => parts.push(String::new()),
        }
    }
    if parts.len() < 5 {
        return None;
    }
    parts.truncate(5);
    let mut dd = parts[0].clone();
    let mut mm = parts[1].clone();
    let mut yy = parts[2].clone();
    let mut tm = parts[3].clone();
    let mut tz = parts[4].clone();
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = mm.to_lowercase();
    if !MONTHS.contains(&mm.as_str()) {
        let swapped = dd.to_lowercase();
        dd = mm;
        mm = swapped;
        if !MONTHS.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut month = MONTHS.iter().position(|m| *m == mm)? as i128 + 1;
    if month > 12 {
        month -= 12;
    }
    if dd.ends_with(',') {
        dd.pop();
    }
    if let Some(i) = yy.find(':') {
        if i > 0 {
            std::mem::swap(&mut yy, &mut tm);
        }
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    if !yy.chars().next().is_some_and(is_digit_char) {
        std::mem::swap(&mut yy, &mut tz);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let pieces: Vec<&str> = tm.split(':').collect();
    let (thh, tmm, tss): (String, String, String) = match pieces.len() {
        2 => (pieces[0].to_owned(), pieces[1].to_owned(), "0".to_owned()),
        3 => (
            pieces[0].to_owned(),
            pieces[1].to_owned(),
            pieces[2].to_owned(),
        ),
        1 if pieces[0].contains('.') => {
            let dotted: Vec<&str> = pieces[0].split('.').collect();
            match dotted.len() {
                2 => (dotted[0].to_owned(), dotted[1].to_owned(), "0".to_owned()),
                3 => (
                    dotted[0].to_owned(),
                    dotted[1].to_owned(),
                    dotted[2].to_owned(),
                ),
                _ => return None,
            }
        }
        _ => return None,
    };
    let mut year = py_int_lenient(&yy)?;
    let day = py_int_lenient(&dd)?;
    let hour = py_int_lenient(&thh)?;
    let minute = py_int_lenient(&tmm)?;
    let second = py_int_lenient(&tss)?;
    if year < 100 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let zone = tz.to_uppercase();
    let mut offset: Option<i128> = match zone_offset(&zone) {
        Some(o) => Some(i128::from(o)),
        None => {
            let parsed = py_int_lenient(&zone);
            if parsed == Some(0) && zone.starts_with('-') {
                None
            } else {
                parsed
            }
        }
    };
    if let Some(o) = offset {
        if o != 0 {
            let (sign, magnitude) = if o < 0 { (-1, -o) } else { (1, o) };
            let seconds = (magnitude / 100)
                .saturating_mul(3600)
                .saturating_add((magnitude % 100) * 60);
            offset = Some(sign * seconds);
        }
    }
    Some((year, month, day, hour, minute, second, offset))
}

/// `email.utils.parsedate_to_datetime(text)` as Python 3.13 and 3.14 define it: ValueError
/// for anything it cannot read, including values beyond the C integer range.
pub fn parsedate_to_datetime(text: &str) -> Result<MailDate> {
    let invalid = || Fail::value_error(format!("Invalid date value or format \"{text}\""));
    let (year, month, day, hour, minute, second, offset) =
        parsedate_tz(text).ok_or_else(invalid)?;
    let int_range = i128::from(i32::MIN)..=i128::from(i32::MAX);
    for value in [year, month, day, hour, minute, second] {
        if !int_range.contains(&value) {
            return Err(invalid());
        }
    }
    let as_i64 = |v: i128| v as i64;
    if !(1..=9999).contains(&year) {
        return Err(Fail::value_error(format!("year {year} is out of range")));
    }
    if !(1..=12).contains(&month) {
        return Err(Fail::value_error("month must be in 1..12"));
    }
    if day < 1 || day > i128::from(days_in_month(as_i64(year), as_i64(month))) {
        return Err(Fail::value_error("day is out of range for month"));
    }
    if !(0..24).contains(&hour) {
        return Err(Fail::value_error("hour must be in 0..23"));
    }
    if !(0..60).contains(&minute) {
        return Err(Fail::value_error("minute must be in 0..59"));
    }
    if !(0..60).contains(&second) {
        return Err(Fail::value_error("second must be in 0..59"));
    }
    let Some(offset) = offset else {
        return Ok(MailDate::Naive);
    };
    // timedelta(seconds=offset): a huge offset overflows (now ValueError), and timezone()
    // takes only offsets strictly inside one day.
    if offset.abs() >= 86_400 * 1_000_000_000 {
        return Err(invalid());
    }
    if offset.abs() >= 86_400 {
        return Err(Fail::value_error(
            "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24).",
        ));
    }
    let local = Instant::from_parts(
        as_i64(year),
        as_i64(month),
        as_i64(day),
        as_i64(hour),
        as_i64(minute),
        as_i64(second),
        0,
    )
    .ok_or_else(invalid)?;
    Ok(MailDate::Aware(i128::from(local.0) - offset * 1_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> String {
        parse_time_str(text).unwrap().stamp()
    }

    #[test]
    fn parses_like_python_314() {
        assert_eq!(
            utc("2026-10-01T13:00:00+01:00"),
            "2026-10-01T12:00:00.000000Z"
        );
        assert_eq!(utc("2026-10-01T12:00Z"), "2026-10-01T12:00:00.000000Z");
        assert_eq!(utc("20261001T120000.5Z"), "2026-10-01T12:00:00.500000Z");
        assert_eq!(utc("2026-W40-4T12:00Z"), "2026-10-01T12:00:00.000000Z");
        assert_eq!(utc("2026-12-31T24:00Z"), "2027-01-01T00:00:00.000000Z");
        assert_eq!(utc("2026-10-01é08:00+00:00"), "2026-10-01T08:00:00.000000Z");
        for bad in [
            "2026-10-01",
            "2026-10-01T12:00",
            "2026-13-01T00:00Z",
            "2026-10-01T12.5Z",
            "x",
        ] {
            assert!(parse_time_str(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_time_str("2026-10-01T12:00").unwrap_err(),
            Fail::input("timestamp requires Z or an explicit UTC offset")
        );
        assert_eq!(
            parse_time_str("0001-01-01T00:00:00+01:00")
                .unwrap_err()
                .kind(),
            "OverflowError"
        );
    }

    #[test]
    fn calendar_and_arithmetic() {
        for days in [0, 1, 58, 59, 365, 1460, 146_096, 719_162, 3_652_058] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        let t = parse_time_str("2026-10-01T12:00:00Z").unwrap();
        assert_eq!(t.weekday(), 3);
        assert_eq!(t.timestamp(), 1_790_856_000.0);
        assert_eq!(
            t.add(Span::minutes(90)).unwrap().stamp(),
            "2026-10-01T13:30:00.000000Z"
        );
        assert_eq!(
            span_of(Num::Float(0.5), 60_000_000).unwrap(),
            Span(30_000_000)
        );
        // Values from CPython's timedelta(seconds=...).
        for (seconds, us) in [
            (1.0000005, 1_000_001),
            (1.0000015, 1_000_001),
            (0.5e-6, 0),
            (1.5e-6, 2),
            (2.5e-6, 2),
            (-0.5e-6, 0),
            (-1.5e-6, -2),
        ] {
            assert_eq!(
                span_of(Num::Float(seconds), 1_000_000).unwrap(),
                Span(us),
                "{seconds}"
            );
        }
        assert!(span_of(Num::Int(1_000_000_000), DAY_US).is_err());
        assert_eq!(Span(-1).total_seconds(), -1e-6);
    }

    #[test]
    fn timestamps_like_python() {
        let at = |v: Value| from_timestamp(&v).map(Instant::stamp);
        assert_eq!(at(Value::Int(0)).unwrap(), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            at(Value::Float(1.5e-6)).unwrap(),
            "1970-01-01T00:00:00.000002Z"
        );
        assert_eq!(
            at(Value::Float(-0.5)).unwrap(),
            "1969-12-31T23:59:59.500000Z"
        );
        assert_eq!(at(Value::Float(f64::NAN)).unwrap_err().kind(), "ValueError");
        assert_eq!(at(Value::Float(1e20)).unwrap_err().kind(), "OverflowError");
        assert_eq!(
            at(Value::Int(300_000_000_000)).unwrap_err().kind(),
            "ValueError"
        );
        assert_eq!(at(Value::from("1")).unwrap_err().kind(), "TypeError");
    }

    #[test]
    fn mail_dates_like_python() {
        let utc_of = |s: &str| match parsedate_to_datetime(s).unwrap().to_utc().unwrap() {
            Some(t) => t.stamp(),
            None => "naive".to_owned(),
        };
        assert_eq!(
            utc_of("Thu, 01 Oct 2026 11:55:00 GMT"),
            "2026-10-01T11:55:00.000000Z"
        );
        assert_eq!(
            utc_of("Thu, 01 Oct 2026 11:55:00 -0500"),
            "2026-10-01T16:55:00.000000Z"
        );
        assert_eq!(utc_of("01 Oct 26 11:55 EST"), "2026-10-01T16:55:00.000000Z");
        assert_eq!(utc_of("Thu, 01 Oct 2026 11:55:00 -0000"), "naive");
        assert!(parsedate_to_datetime("not a date").is_err());
        assert!(parsedate_to_datetime("Thu, 31 Feb 2026 11:55:00 GMT").is_err());
        // Two-digit and small years are moved to 1969-2068, so only the far end overflows.
        let late = parsedate_to_datetime("Fri, 31 Dec 9999 23:59:00 -0100").unwrap();
        assert_eq!(late.to_utc().unwrap_err().kind(), "OverflowError");
        let now = parse_time_str("2026-10-01T12:00:00Z").unwrap();
        assert!(late.seconds_since(now).unwrap() > 0.0);
        assert_eq!(
            utc_of("Mon, 01 Jan 0001 00:00:00 +0100"),
            "2000-12-31T23:00:00.000000Z"
        );
    }
}
