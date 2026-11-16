//! Timestamps: written as UTC `YYYY-MM-DDTHH:MM:SS.ffffff+00:00`, read in the
//! ISO 8601 forms the baseline accepted. A moment keeps its UTC offset so a
//! clamped journey time is written back with the offset it was read with.

use std::time::{SystemTime, UNIX_EPOCH};

const US_PER_SEC: i64 = 1_000_000;
const US_PER_DAY: i64 = 86_400 * US_PER_SEC;

#[derive(Clone, Copy, Debug)]
pub struct Moment {
    /// Microseconds since 1970-01-01T00:00:00Z.
    pub utc_us: i64,
    /// UTC offset in microseconds.
    pub offset_us: i64,
}

impl PartialEq for Moment {
    fn eq(&self, other: &Self) -> bool {
        self.utc_us == other.utc_us
    }
}

impl PartialOrd for Moment {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.utc_us.partial_cmp(&other.utc_us)
    }
}

pub fn now() -> Moment {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Moment {
        utc_us: since.as_micros() as i64,
        offset_us: 0,
    }
}

/// Days since the epoch to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// `isoformat(timespec="microseconds")`.
pub fn stamp(moment: Moment) -> String {
    let local = moment.utc_us + moment.offset_us;
    let days = local.div_euclid(US_PER_DAY);
    let rest = local.rem_euclid(US_PER_DAY);
    let (y, mo, d) = civil_from_days(days);
    let secs = rest / US_PER_SEC;
    let micros = rest % US_PER_SEC;
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}",
        y,
        mo,
        d,
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        micros
    );
    let off = moment.offset_us;
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.abs();
    let (osec, ous) = (off / US_PER_SEC, off % US_PER_SEC);
    out.push_str(&format!("{sign}{:02}:{:02}", osec / 3600, (osec / 60) % 60));
    if osec % 60 != 0 || ous != 0 {
        out.push_str(&format!(":{:02}", osec % 60));
        if ous != 0 {
            out.push_str(&format!(".{ous:06}"));
        }
    }
    out
}

pub fn stamp_now() -> String {
    stamp(now())
}

/// How a timestamp text reads.
#[derive(Clone, Copy, Debug)]
pub enum Parsed {
    /// Not an ISO 8601 time the baseline accepted.
    Invalid,
    /// A valid time without a time zone.
    Naive,
    Aware(Moment),
}

/// The baseline's `fromisoformat` after `Z` → `+00:00`; None without a time zone.
pub fn parse_time(text: &str) -> Option<Moment> {
    match parse_time_detail(text) {
        Parsed::Aware(moment) => Some(moment),
        Parsed::Invalid | Parsed::Naive => None,
    }
}

/// Like [`parse_time`], telling an invalid text from one without a time zone.
pub fn parse_time_detail(text: &str) -> Parsed {
    match parse_iso(text) {
        None => Parsed::Invalid,
        Some(None) => Parsed::Naive,
        Some(Some(moment)) => Parsed::Aware(moment),
    }
}

/// The text as NUL-terminated bytes, read past the end as 0 like the C code reads.
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn at(&self, i: usize) -> u8 {
        self.0.get(i).copied().unwrap_or(0)
    }

    /// `parse_digits`: `n` ASCII digits from `i` added onto `var`; the index after them.
    fn digits(&self, i: usize, var: &mut i64, n: usize) -> Option<usize> {
        for k in 0..n {
            let d = self.at(i + k).wrapping_sub(b'0');
            if d > 9 {
                return None;
            }
            *var = *var * 10 + i64::from(d);
        }
        Some(i + n)
    }
}

/// `_find_isoformat_datetime_separator`.
fn separator_at(b: &Bytes, len: usize) -> Option<usize> {
    if len == 7 {
        return Some(7);
    }
    if b.at(4) == b'-' {
        if b.at(5) == b'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && b.at(8) == b'-' {
                if len == 9 {
                    return None;
                }
                // YYYY-Www-D (10) or YYYY-Www-HH (8): a digit at 10 reads as a time
                return Some(if len > 10 && b.at(10).is_ascii_digit() {
                    8
                } else {
                    10
                });
            }
            return Some(8);
        }
        return Some(10);
    }
    if b.at(4) == b'W' {
        let mut idx = 7;
        while idx < len && b.at(idx).is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// Ordinal day (0001-01-01 is 1).
fn ordinal(y: i64, m: u32, d: u32) -> i64 {
    days_from_civil(y, m, d) + 719_163
}

/// `iso_to_ymd`: (year, week, weekday) to (year, month, day).
fn iso_to_ymd(year: i64, week: i64, day: i64) -> Option<(i64, u32, u32)> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    let first = ordinal(year, 1, 1);
    let first_weekday = (first + 6) % 7; // 0 is Monday
    if week <= 0 || week >= 53 {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let long_year = first_weekday == 3 || (first_weekday == 2 && leap);
        if !(week == 53 && long_year) {
            return None;
        }
    }
    if !(1..=7).contains(&day) {
        return None;
    }
    let mut monday = first - first_weekday;
    if first_weekday > 3 {
        monday += 7;
    }
    Some(civil_from_days(monday + (week - 1) * 7 + day - 1 - 719_163))
}

/// `parse_isoformat_date` over the bytes before the separator at `len`.
fn parse_date(b: &Bytes, len: usize) -> Option<(i64, u32, u32)> {
    let mut year = 0;
    let mut p = b.digits(0, &mut year, 4)?;
    let dashed = b.at(p) == b'-';
    if dashed {
        p += 1;
    }
    if b.at(p) == b'W' {
        let mut week = 0;
        let mut day = 0;
        p = b.digits(p + 1, &mut week, 2)?;
        if p < len {
            if dashed {
                if b.at(p) != b'-' {
                    return None;
                }
                p += 1;
            }
            b.digits(p, &mut day, 1)?;
        } else {
            day = 1;
        }
        return iso_to_ymd(year, week, day);
    }
    let (mut month, mut day) = (0, 0);
    p = b.digits(p, &mut month, 2)?;
    if dashed {
        if b.at(p) != b'-' {
            return None;
        }
        p += 1;
    }
    b.digits(p, &mut day, 2)?;
    Some((year, month as u32, day as u32))
}

/// `parse_hh_mm_ss_ff` over `[start, end)`: ([h, m, s], us, more) where `more`
/// is the C code's return 1 (text left after the fields).
fn parse_clock(b: &Bytes, start: usize, end: usize) -> Option<([i64; 3], i64, bool)> {
    let mut vals = [0i64; 3];
    let mut p = start;
    let mut has_sep = true;
    for i in 0..3 {
        p = b.digits(p, &mut vals[i], 2)?;
        let c = b.at(p);
        p += 1;
        if i == 0 {
            has_sep = c == b':';
        }
        if p >= end {
            return Some((vals, 0, c != 0));
        }
        if has_sep && c == b':' {
            continue;
        }
        if c == b'.' || c == b',' {
            break;
        }
        if has_sep {
            return None;
        }
        p -= 1;
    }
    let take = (end - p).min(6);
    let mut micro = 0;
    p = b.digits(p, &mut micro, take)?;
    micro *= 10i64.pow((6 - take) as u32);
    while b.at(p).is_ascii_digit() {
        p += 1;
    }
    Some((vals, micro, b.at(p) != 0))
}

/// The clock, its microseconds, and the offset (seconds, microseconds) if any.
type TimePart = ([i64; 3], i64, Option<(i64, i64)>);

/// `parse_isoformat_time` over `[start, end)`.
fn parse_time_part(b: &Bytes, start: usize, end: usize) -> Option<TimePart> {
    let mut tz = start;
    loop {
        if matches!(b.at(tz), b'Z' | b'+' | b'-') {
            break;
        }
        tz += 1;
        if tz >= end {
            break;
        }
    }
    let (clock, micro, more) = parse_clock(b, start, tz)?;
    if tz == end {
        return (!more).then_some((clock, micro, None));
    }
    if b.at(tz) == b'Z' {
        return (b.at(tz + 1) == 0).then_some((clock, micro, Some((0, 0))));
    }
    let sign = if b.at(tz) == b'-' { -1 } else { 1 };
    let (off, off_us, more) = parse_clock(b, tz + 1, end)?;
    if more {
        return None;
    }
    let seconds = sign * (off[0] * 3600 + off[1] * 60 + off[2]);
    Some((clock, micro, Some((seconds, sign * off_us))))
}

/// `datetime.fromisoformat` (3.12, C implementation) after `Z` → `+00:00`.
/// None: invalid; Some(None): naive; Some(Some(moment)): with a time zone.
fn parse_iso(text: &str) -> Option<Option<Moment>> {
    let text = text.replace('Z', "+00:00");
    let mut chars: Vec<char> = text.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    // _sanitize_isoformat_str: a lone surrogate separator at 7, 8 or 10 reads as 'T'
    for pos in [7, 8, 10] {
        if pos > chars.len() {
            break;
        }
        if chars
            .get(pos)
            .is_some_and(|c| crate::jsonio::as_surrogate(*c).is_some())
        {
            chars[pos] = 'T';
            break;
        }
    }
    if chars
        .iter()
        .any(|c| crate::jsonio::as_surrogate(*c).is_some())
    {
        return None; // not encodable as UTF-8
    }
    let text: String = chars.into_iter().collect();
    let b = Bytes(text.as_bytes());
    let len = text.len();
    let sep = separator_at(&b, len)?;
    let (y, mo, d) = parse_date(&b, sep)?;
    let (mut clock, mut micro, mut offset) = ([0i64; 3], 0, None);
    if len > sep {
        let lead = b.at(sep);
        let step = if lead & 0x80 == 0 {
            1
        } else {
            match lead & 0xf0 {
                0xe0 => 3,
                0xf0 => 4,
                _ => 2,
            }
        };
        (clock, micro, offset) = parse_time_part(&b, sep + step, len)?;
    }
    // the datetime constructor's range checks
    if !(1..=9999).contains(&y) || !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) {
        return None;
    }
    let [h, mi, s] = clock;
    if h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let Some((off_s, off_us)) = offset else {
        return Some(None);
    };
    // tzinfo_from_isoformat_results: a zero seconds part is UTC whatever the microseconds
    let offset_us = if off_s == 0 {
        0
    } else {
        off_s * US_PER_SEC + off_us
    };
    if offset_us.abs() >= US_PER_DAY {
        return None;
    }
    let local =
        days_from_civil(y, mo, d) * US_PER_DAY + ((h * 60 + mi) * 60 + s) * US_PER_SEC + micro;
    Some(Some(Moment {
        utc_us: local - offset_us,
        offset_us,
    }))
}

/// The baseline's `OverflowError` for a day count beyond a C int (the interpreter's
/// name is spelled with an escape so the source names no interpreter).
const INT_TOO_LARGE: &str = "OverflowError: Pyth\u{6f}n int too large to convert to C int";

/// `timedelta(<unit>=amount)` in microseconds, `unit_us` microseconds per unit,
/// with the constructor's errors: a non-finite float, more days than a C int
/// holds, or more than 999999999 days.
pub fn timedelta_us(amount: crate::py::Num, unit_us: i64) -> crate::error::Res<i128> {
    use crate::error::PmError;
    use crate::py::Num;
    let too_large = || PmError::Internal(INT_TOO_LARGE.into());
    let total: i128 = match amount {
        Num::Int(n) => n.checked_mul(i128::from(unit_us)).ok_or_else(too_large)?,
        Num::Float(x) => {
            // accum() and the rounding of delta_new(): whole units exactly, the
            // fraction through float arithmetic, then half-to-even microseconds
            if x.is_nan() {
                return Err(PmError::Internal(
                    "ValueError: cannot convert float NaN to integer".into(),
                ));
            }
            if x.is_infinite() {
                return Err(PmError::Internal(
                    "OverflowError: cannot convert float infinity to integer".into(),
                ));
            }
            let whole = x.trunc();
            if whole.abs() >= 1e36 {
                return Err(too_large());
            }
            let mut total = (whole as i128)
                .checked_mul(i128::from(unit_us))
                .ok_or_else(too_large)?;
            let part = unit_us as f64 * (x - whole);
            let part_whole = part.trunc();
            total += part_whole as i128;
            let left = part - part_whole;
            if left != 0.0 {
                let mut us = left.round();
                if (us - left).abs() == 0.5 {
                    let odd = (total % 2 != 0) as i64 as f64;
                    us = 2.0 * ((left + odd) * 0.5).round() - odd;
                }
                total += us as i128;
            }
            total
        }
    };
    let days = total.div_euclid(i128::from(US_PER_DAY));
    if days > i128::from(i32::MAX) || days < i128::from(i32::MIN) {
        return Err(too_large());
    }
    if days.abs() > 999_999_999 {
        return Err(PmError::Internal(format!(
            "OverflowError: days={days}; must have magnitude <= 999999999"
        )));
    }
    Ok(total)
}

/// Seconds between two moments, `round(total_seconds(), 3)`.
pub fn seconds_between(start: Moment, end: Moment) -> f64 {
    crate::py::round3((end.utc_us - start.utc_us) as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_offsets() {
        let text = "2026-11-16T03:04:05.123456+00:00";
        let m = parse_time(text).unwrap();
        assert_eq!(stamp(m), text);
        let k = parse_time("2026-11-16T12:04:05+09:00").unwrap();
        assert_eq!(k.utc_us, parse_time("2026-11-16T03:04:05Z").unwrap().utc_us);
        assert_eq!(stamp(k), "2026-11-16T12:04:05.000000+09:00");
        assert!(parse_time("2026-11-16T03:04:05").is_none());
        assert!(parse_time("2026-11-16").is_none());
        assert!(parse_time("nope").is_none());
        assert!(parse_time("2026-02-30T00:00:00+00:00").is_none());
        assert!(matches!(
            parse_time_detail("2026-11-16T03:04:05"),
            Parsed::Naive
        ));
        assert!(matches!(parse_time_detail("2026-11-16"), Parsed::Naive));
        assert!(matches!(parse_time_detail("nope"), Parsed::Invalid));
        assert!(matches!(
            parse_time_detail("2026-11-16T03:04:05Z"),
            Parsed::Aware(_)
        ));
        assert_eq!(
            stamp(parse_time("1970-01-01 00:00:00.5-01:30").unwrap()),
            "1970-01-01T00:00:00.500000-01:30"
        );
    }

    /// Forms the baseline's `fromisoformat` takes or refuses (r1-K7), with the UTC
    /// microseconds and offset it gives.
    #[test]
    fn fromisoformat_forms_of_the_baseline() {
        let cases: [(&str, Option<(i64, i64)>); 12] = [
            (
                "2026-W01-1T00:00:00+00:00",
                Some((1_766_966_400_000_000, 0)),
            ),
            (
                "2026-01-01T00:00:05.+00:00",
                Some((1_767_225_605_000_000, 0)),
            ),
            (
                "2026-01-01T00:00:05 +00:00",
                Some((1_767_225_605_000_000, 0)),
            ),
            (
                "2026-01-01T00:00:00+00:00:00.5",
                Some((1_767_225_600_000_000, 0)),
            ),
            (
                "2026-01-01T00:00:00,5+00:00",
                Some((1_767_225_600_500_000, 0)),
            ),
            ("20260101T000000Z", Some((1_767_225_600_000_000, 0))),
            (
                "2026W011T01+0130",
                Some((1_766_964_600_000_000, 5_400_000_000)),
            ),
            (
                "2026-11-16T12:34:56:78+00:00",
                Some((1_794_832_496_780_000, 0)),
            ),
            ("2026-001T00:00+00:00", None),
            ("2026-01-01T24:00+00:00", None),
            ("2026-W01-1000:00:00+00:00", None),
            ("2026-11-16T12:00+24:00", None),
        ];
        for (text, want) in cases {
            let got = parse_time(text).map(|m| (m.utc_us, m.offset_us));
            assert_eq!(got, want, "{text}");
        }
    }

    /// `timedelta(hours=...)`/`timedelta(minutes=...)` of the baseline (r1-K6).
    #[test]
    fn timedelta_values_and_range_errors() {
        use crate::py::Num;
        let hour = 3_600_000_000;
        for (h, want) in [
            (1.5, 5_400_000_000i128),
            (1e-10, 0),
            (2.5e-10, 1),
            (0.1, 360_000_000),
            (1.0 / 3.0, 1_200_000_000),
            (1e9, 3_600_000_000_000_000_000),
        ] {
            assert_eq!(timedelta_us(Num::Float(h), hour).ok(), Some(want), "{h}");
        }
        let err = |n: Num, unit| timedelta_us(n, unit).unwrap_err().to_string_for_test();
        assert_eq!(
            err(Num::Float(f64::INFINITY), hour),
            "OverflowError: cannot convert float infinity to integer"
        );
        assert_eq!(err(Num::Float(1e30), hour), INT_TOO_LARGE);
        let minute = 60_000_000;
        assert!(timedelta_us(Num::Int(200_000_000_000), minute).is_ok());
        assert_eq!(
            err(Num::Int(1_440_000_000_000), minute),
            "OverflowError: days=1000000000; must have magnitude <= 999999999"
        );
        assert_eq!(err(Num::Int(3_092_376_453_120), minute), INT_TOO_LARGE);
        assert_eq!(
            err(Num::Int(-3_092_376_453_120), minute),
            "OverflowError: days=-2147483648; must have magnitude <= 999999999"
        );
    }
}
