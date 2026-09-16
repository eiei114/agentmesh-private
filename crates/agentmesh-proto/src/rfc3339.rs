//! Deterministic RFC 3339 timestamp parsing for canonical ledger ordering.

/// Parse an RFC 3339 timestamp into epoch seconds for canonical ordering.
pub fn parse_rfc3339_epoch(ts: &str) -> Option<i64> {
    let bytes = ts.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { ts.get(range)?.parse().ok() };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let offset_seconds: i64 = match ts.get(19..)?.chars().next()? {
        'Z' | 'z' => 0,
        sign @ ('+' | '-') => {
            let offset_hours: i64 = ts.get(20..22)?.parse().ok()?;
            let offset_minutes: i64 = ts.get(23..25)?.parse().ok()?;
            if ts.as_bytes().get(22)? != &b':' {
                return None;
            }
            let magnitude = offset_hours * 3600 + offset_minutes * 60;
            if sign == '+' {
                magnitude
            } else {
                -magnitude
            }
        }
        _ => return None,
    };
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
            - offset_seconds,
    )
}

/// Days-from-civil (Howard Hinnant) for proleptic Gregorian dates.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let month_of_year = (month + 9) % 12;
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_rfc3339_epoch;

    #[test]
    fn rfc3339_instants_normalize_offsets() {
        // 2026-08-21T00:00:00+09:00 == 2026-08-20T15:00:00Z
        assert_eq!(
            parse_rfc3339_epoch("2026-08-21T00:00:00+09:00"),
            parse_rfc3339_epoch("2026-08-20T15:00:00Z")
        );
        // ...and that instant is earlier than 2026-08-20T20:00:00Z even though
        // the raw string sorts lexically after it.
        assert!(
            parse_rfc3339_epoch("2026-08-21T00:00:00+09:00")
                < parse_rfc3339_epoch("2026-08-20T20:00:00Z")
        );
    }

    #[test]
    fn rfc3339_rejects_malformed_timestamps() {
        assert_eq!(parse_rfc3339_epoch("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_epoch("not-a-ts"), None);
        assert_eq!(parse_rfc3339_epoch("2026-08-22T10:00:00"), None);
    }
}
