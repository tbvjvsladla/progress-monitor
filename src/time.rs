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

fn digits(text: &[u8], at: usize, len: usize) -> Option<i64> {
    let part = text.get(at..at + len)?;
    if !part.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(part).ok()?.parse().ok()
}

/// (hour, minute, second, microsecond) from `HH[:MM[:SS[.f+]]]` or `HH[MM[SS[.f+]]]`.
fn parse_clock(text: &[u8]) -> Option<(i64, i64, i64, i64)> {
    let mut comps = [0i64; 3];
    let mut pos = 0;
    let mut extended: Option<bool> = None;
    let mut count = 0;
    while count < 3 {
        comps[count] = digits(text, pos, 2)?;
        pos += 2;
        count += 1;
        if pos >= text.len() {
            break;
        }
        let next = text[pos];
        if count == 3 {
            break;
        }
        if next == b':' {
            if extended == Some(false) {
                return None;
            }
            extended = Some(true);
            pos += 1;
        } else if next == b'.' || next == b',' {
            break;
        } else {
            if extended == Some(true) {
                return None;
            }
            extended = Some(false);
        }
    }
    let mut micro = 0;
    if pos < text.len() {
        if count < 3 || !(text[pos] == b'.' || text[pos] == b',') {
            return None;
        }
        pos += 1;
        let frac = &text[pos..];
        if frac.is_empty() || !frac.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let take = frac.len().min(6);
        let value: i64 = std::str::from_utf8(&frac[..take]).ok()?.parse().ok()?;
        micro = value * 10i64.pow((6 - take) as u32);
    }
    let (h, m, s) = (comps[0], comps[1], comps[2]);
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    Some((h, m, s, micro))
}

/// The baseline's `fromisoformat` after `Z` → `+00:00`; None without a time zone.
pub fn parse_time(text: &str) -> Option<Moment> {
    let text = text.replace('Z', "+00:00");
    let bytes = text.as_bytes();
    let (y, mo, d, date_len) = if bytes.get(4) == Some(&b'-') {
        if bytes.get(7) != Some(&b'-') {
            return None;
        }
        (
            digits(bytes, 0, 4)?,
            digits(bytes, 5, 2)?,
            digits(bytes, 8, 2)?,
            10,
        )
    } else {
        (
            digits(bytes, 0, 4)?,
            digits(bytes, 4, 2)?,
            digits(bytes, 6, 2)?,
            8,
        )
    };
    if y < 1 || !(1..=12).contains(&mo) {
        return None;
    }
    let (mo, d) = (mo as u32, d as u32);
    if d < 1 || d > days_in_month(y, mo) {
        return None;
    }
    if bytes.len() <= date_len {
        return None; // a date alone has no time zone
    }
    // one separator character (any), then the time
    let sep_len = text[date_len..].chars().next()?.len_utf8();
    let rest = &bytes[date_len + sep_len..];
    let tz_at = rest.iter().position(|b| *b == b'+' || *b == b'-');
    let Some(tz_at) = tz_at else {
        parse_clock(rest)?;
        return None; // naive
    };
    let (h, mi, s, us) = parse_clock(&rest[..tz_at])?;
    let sign: i64 = if rest[tz_at] == b'-' { -1 } else { 1 };
    let (oh, om, os, ous) = parse_offset(&rest[tz_at + 1..])?;
    let offset = sign * (((oh * 60 + om) * 60 + os) * US_PER_SEC + ous);
    if offset.abs() >= US_PER_DAY {
        return None;
    }
    let local = days_from_civil(y, mo, d) * US_PER_DAY + ((h * 60 + mi) * 60 + s) * US_PER_SEC + us;
    Some(Moment {
        utc_us: local - offset,
        offset_us: offset,
    })
}

fn parse_offset(text: &[u8]) -> Option<(i64, i64, i64, i64)> {
    match text.len() {
        2 => Some((digits(text, 0, 2)?, 0, 0, 0)),
        4 => Some((digits(text, 0, 2)?, digits(text, 2, 2)?, 0, 0)),
        5 if text[2] == b':' => Some((digits(text, 0, 2)?, digits(text, 3, 2)?, 0, 0)),
        6 => Some((
            digits(text, 0, 2)?,
            digits(text, 2, 2)?,
            digits(text, 4, 2)?,
            0,
        )),
        8 if text[2] == b':' && text[5] == b':' => Some((
            digits(text, 0, 2)?,
            digits(text, 3, 2)?,
            digits(text, 6, 2)?,
            0,
        )),
        n if n > 9 && text[2] == b':' && text[5] == b':' && text[8] == b'.' => {
            let frac = &text[9..];
            if frac.len() != 6 || !frac.iter().all(u8::is_ascii_digit) {
                return None;
            }
            Some((
                digits(text, 0, 2)?,
                digits(text, 3, 2)?,
                digits(text, 6, 2)?,
                digits(text, 9, 6)?,
            ))
        }
        _ => None,
    }
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
        assert_eq!(
            stamp(parse_time("1970-01-01 00:00:00.5-01:30").unwrap()),
            "1970-01-01T00:00:00.500000-01:30"
        );
    }
}
