//! RFC 3339 timestamps ("2026-10-09T15:30:00.050249+00:00") to and from Unix seconds, without a
//! date crate. Parsing and writing work on bytes, so the Kani proofs below cover them directly.

/// Parses an RFC 3339 timestamp into Unix seconds. Returns None for anything else.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    parse_bytes(s.trim().as_bytes())
}

/// `len` ASCII digits starting at `at`, as a number.
fn digits(b: &[u8], at: usize, len: usize) -> Option<i64> {
    let mut n: i64 = 0;
    for &c in b.get(at..at + len)? {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + i64::from(c - b'0');
    }
    Some(n)
}

/// A UTC timestamp split into its six fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fields {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    min: i64,
    sec: i64,
}

impl Fields {
    fn is_valid(self) -> bool {
        (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour <= 23
            && self.min <= 59
            && self.sec <= 60
    }

    /// Unix seconds of these fields, in UTC.
    fn secs(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + self.hour * 3600
            + self.min * 60
            + self.sec
    }
}

/// The fields of "YYYY-MM-DDTHH:MM:SS" (a space or a lower-case t also separates date and time).
fn read_fields(b: &[u8]) -> Option<Fields> {
    let f = Fields {
        year: digits(b, 0, 4)?,
        month: digits(b, 5, 2)?,
        day: digits(b, 8, 2)?,
        hour: digits(b, 11, 2)?,
        min: digits(b, 14, 2)?,
        sec: digits(b, 17, 2)?,
    };
    let separators = b[4] == b'-'
        && b[7] == b'-'
        && matches!(b[10], b'T' | b't' | b' ')
        && b[13] == b':'
        && b[16] == b':';
    separators.then_some(f)
}

fn parse_bytes(b: &[u8]) -> Option<i64> {
    let fields = read_fields(b)?;
    // Skip fractional seconds, then read the offset.
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let o = digits(b, i + 1, 2)? * 3600 + digits(b, i + 4, 2)? * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    fields.is_valid().then(|| fields.secs() - offset)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`] for days on or after 1970-01-01: (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days.max(0) + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The latest second the 4-digit year format can hold: 9999-12-31T23:59:59Z.
pub const MAX_SECS: i64 = 253_402_300_799;

/// The fields of `secs`, clamped to 1970…9999.
fn fields_of(secs: i64) -> Fields {
    let secs = secs.clamp(0, MAX_SECS);
    let (year, month, day) = civil_from_days(secs / 86_400);
    let rem = secs % 86_400;
    Fields {
        year,
        month,
        day,
        hour: rem / 3600,
        min: rem % 3600 / 60,
        sec: rem % 60,
    }
}

/// Fields as the 20 bytes "YYYY-MM-DDTHH:MM:SSZ". Each field must fit its width.
fn write_fields(f: Fields) -> [u8; 20] {
    let mut out = *b"0000-00-00T00:00:00Z";
    for (at, len, mut v) in [
        (0, 4, f.year),
        (5, 2, f.month),
        (8, 2, f.day),
        (11, 2, f.hour),
        (14, 2, f.min),
        (17, 2, f.sec),
    ] {
        for k in (at..at + len).rev() {
            out[k] = b'0' + u8::try_from(v % 10).unwrap_or(0);
            v /= 10;
        }
    }
    out
}

/// `secs` (clamped to 1970…9999) as the 20 bytes "YYYY-MM-DDTHH:MM:SSZ".
fn write_bytes(secs: i64) -> [u8; 20] {
    write_fields(fields_of(secs))
}

/// Unix seconds as RFC 3339 in UTC: "2026-10-09T15:30:00Z".
#[must_use]
pub fn format_rfc3339(secs: i64) -> String {
    write_bytes(secs).iter().map(|&b| char::from(b)).collect()
}

/// Unix seconds as "2026-10-09 15:30:00" (UTC), for logs.
#[must_use]
pub fn format_utc(secs: i64) -> String {
    let b = write_bytes(secs);
    b[..19]
        .iter()
        .map(|&c| if c == b'T' { ' ' } else { char::from(c) })
        .collect()
}

/// (days, hours, minutes) from `now` until `target`, or None when `target` is not in the future.
#[must_use]
pub fn until_parts(target: i64, now: i64) -> Option<(i64, i64, i64)> {
    let secs = target.checked_sub(now).filter(|s| *s > 0)?;
    Some((secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60))
}

/// "in 3h 05m", "in 2d 14h", "now" — reset times without needing the local time zone.
#[must_use]
pub fn until(target: i64, now: i64) -> String {
    match until_parts(target, now) {
        None => "now".into(),
        Some((d, h, _)) if d > 0 => format!("in {d}d {h}h"),
        Some((_, h, m)) if h > 0 => format!("in {h}h {m:02}m"),
        Some((_, _, m)) => format!("in {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_timestamps() {
        // Values seen from the usage endpoint on 2026-10-09.
        assert_eq!(
            parse_rfc3339("2026-10-09T15:30:00.050249+00:00"),
            Some(1_791_559_800)
        );
        assert_eq!(
            parse_rfc3339("2026-10-11T03:59:59.574705+00:00"),
            Some(1_791_691_199)
        );
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-10-09T19:30:00+04:00"),
            Some(1_791_559_800)
        );
        assert_eq!(
            parse_rfc3339("  2026-10-09T15:30:00Z\n"),
            Some(1_791_559_800)
        );
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "tomorrow",
            "2026-13-01T00:00:00Z",
            "2026/10/09T15:30:00Z",
            "2026-10-09T15:30:00+0400",
            "2026-10-09T15:30:00",
            "2026-10-09T25:30:00Z",
            "2026-10-09T15:30:00Zjunk",
            "2026-10-09T15:30:00+04:00junk",
            "+026-10-09T15:30:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
        assert_eq!(
            parse_rfc3339("2026-10-09t15:30:00.5-01:30"),
            Some(1_791_559_800 + 5400)
        );
    }

    #[test]
    fn rejects_a_non_digit_in_every_field() {
        for bad in [
            "2026-1x-09T15:30:00Z",
            "2026-10-x9T15:30:00Z",
            "2026-10-09T1x:30:00Z",
            "2026-10-09T15:x0:00Z",
            "2026-10-09T15:30:x0Z",
            "2026-10-09T15:30:00+x4:00",
            "2026-10-09T15:30:00+04:x0",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
        assert_eq!(
            parse_rfc3339("2026-10-09 15:30:00Z"),
            Some(1_791_559_800),
            "a space separates too"
        );
        assert_eq!(
            parse_rfc3339("2026-10-09X15:30:00Z"),
            None,
            "only T, t or a space"
        );
    }

    #[test]
    fn checks_each_separator_on_its_own() {
        for bad in [
            "2026x10-09T15:30:00Z",
            "2026-10x09T15:30:00Z",
            "2026-10-09T15x30:00Z",
            "2026-10-09T15:30x00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn checks_each_field_at_its_limits() {
        let at = |s: &str| parse_rfc3339(s).is_some();
        assert!(at("2026-12-31T23:59:59Z") && at("2026-01-01T00:00:00Z"));
        assert!(at("2016-12-31T23:59:60Z"), "a leap second is accepted");
        for bad in [
            "2026-00-01T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "2026-01-32T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:60:00Z",
            "2026-01-01T00:00:61Z",
        ] {
            assert!(!at(bad), "{bad:?}");
        }
    }

    #[test]
    fn leap_days_and_century_rules_round_trip() {
        // The last day of a 400-year cycle, a 100-year non-leap year, and ordinary leap days.
        for s in [
            "2000-02-29T12:00:00Z",
            "2100-02-28T23:59:59Z",
            "2100-03-01T00:00:00Z",
            "2400-02-29T00:00:00Z",
            "1972-02-29T00:00:00Z",
            "9999-12-31T23:59:59Z",
        ] {
            let secs = parse_rfc3339(s).unwrap();
            assert_eq!(format_rfc3339(secs), s);
        }
    }

    #[test]
    fn reads_years_before_1970() {
        // Year 0 January runs the negative-era branch of the date conversion.
        assert_eq!(parse_rfc3339("0000-01-01T00:00:00Z"), Some(-62_167_219_200));
        assert_eq!(parse_rfc3339("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn formats_and_round_trips() {
        assert_eq!(format_rfc3339(1_791_559_800), "2026-10-09T15:30:00Z");
        assert_eq!(format_utc(1_791_559_800), "2026-10-09 15:30:00");
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_rfc3339(-5), "1970-01-01T00:00:00Z", "clamped");
        assert_eq!(format_rfc3339(i64::MAX), "9999-12-31T23:59:59Z", "clamped");
        for secs in [0, 951_782_400, 1_791_559_800, 4_102_444_800, MAX_SECS] {
            assert_eq!(parse_rfc3339(&format_rfc3339(secs)), Some(secs));
        }
    }

    #[test]
    fn formats_durations() {
        assert_eq!(until(100, 200), "now");
        assert_eq!(until(200, 200), "now");
        assert_eq!(until(200 + 5 * 60, 200), "in 5m");
        assert_eq!(until(3 * 3600 + 5 * 60, 0), "in 3h 05m");
        assert_eq!(until(2 * 86_400 + 14 * 3600, 0), "in 2d 14h");
        assert_eq!(
            until(i64::MAX, i64::MIN),
            "now",
            "overflow is not a future time"
        );
    }
}

/// Formal proofs, checked by the Kani model checker. Run with `cargo kani -p acctswap-core`.
#[cfg(kani)]
mod proofs {
    use super::*;

    /// Parsing never panics or overflows, whatever 32 bytes it is given.
    #[kani::proof]
    #[kani::unwind(34)]
    fn parse_never_panics() {
        let bytes: [u8; 32] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= 32);
        let _ = parse_bytes(&bytes[..len]);
    }

    /// Lemma 1: writing six fields then reading them back gives the same fields, for every
    /// value that fits its width.
    #[kani::proof]
    #[kani::unwind(8)] // six fields of at most four digits each
    fn written_fields_read_back() {
        let f = Fields {
            year: kani::any(),
            month: kani::any(),
            day: kani::any(),
            hour: kani::any(),
            min: kani::any(),
            sec: kani::any(),
        };
        kani::assume((0..=9999).contains(&f.year));
        for v in [f.month, f.day, f.hour, f.min, f.sec] {
            kani::assume((0..=99).contains(&v));
        }
        assert_eq!(read_fields(&write_fields(f)), Some(f));
    }

    /// Lemma 2: every second sw can write splits into valid fields that add back up to it.
    /// With lemma 1, this is the round trip: `parse(format(secs)) == secs` for 1970…9999.
    #[kani::proof]
    fn seconds_split_into_valid_fields_and_back() {
        let secs: i64 = kani::any();
        kani::assume((0..=MAX_SECS).contains(&secs));
        let f = fields_of(secs);
        assert!(f.is_valid() && (1970..=9999).contains(&f.year));
        assert_eq!(f.secs(), secs);
    }

    /// The two date conversions are exact inverses (years 1970–9999).
    #[kani::proof]
    fn date_conversions_are_inverse() {
        let days: i64 = kani::any();
        kani::assume((0..=2_932_896).contains(&days));
        let (y, m, d) = civil_from_days(days);
        assert!((1..=12).contains(&m) && (1..=31).contains(&d));
        assert_eq!(days_from_civil(y, m, d), days);
    }

    /// A duration exists exactly when the target is in the future, and its parts are in range.
    #[kani::proof]
    fn durations_are_well_formed() {
        let (t, n): (i64, i64) = (kani::any(), kani::any());
        match until_parts(t, n) {
            None => assert!(t.checked_sub(n).is_none_or(|s| s <= 0)),
            Some((d, h, m)) => {
                assert!(t > n);
                assert!(d >= 0 && (0..24).contains(&h) && (0..60).contains(&m));
            }
        }
    }
}
