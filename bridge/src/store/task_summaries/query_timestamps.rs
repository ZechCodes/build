//! Deterministic `ecmascript-iso-v1` domain, independent from legacy replay.
//! Native 1..9-digit fractions are a documented Node/Chromium-tested extension;
//! see docs/task-query-timestamp-domain.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryTimestamp {
    Known(i64),
    Unknown,
    Unsupported,
}

pub(super) fn parse(value: Option<&str>) -> QueryTimestamp {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return QueryTimestamp::Unknown;
    };
    parse_known(value).map_or(QueryTimestamp::Unsupported, QueryTimestamp::Known)
}

fn parse_known(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    // Longest form: signed six-digit year + full clock + nine digits + offset.
    if bytes.len() > 38 {
        return None;
    }
    let year_width = if matches!(bytes.first(), Some(b'+' | b'-')) {
        7
    } else {
        4
    };
    let date_width = year_width + 6;
    let date = bytes.get(..date_width)?;
    let days = date_days(date, year_width)?;
    let time = if bytes.len() == date_width {
        0
    } else {
        time_millis(bytes.get(date_width..)?)?
    };
    let millis = days * 86_400_000 + time;
    (millis.abs() <= 8_640_000_000_000_000).then_some(millis)
}

fn decimal(bytes: &[u8]) -> Option<i64> {
    bytes.iter().try_fold(0, |value, digit| {
        digit
            .is_ascii_digit()
            .then(|| value * 10 + i64::from(digit - b'0'))
    })
}

fn date_days(date: &[u8], year_width: usize) -> Option<i64> {
    if date[year_width] != b'-' || date[year_width + 3] != b'-' {
        return None;
    }
    let year = if year_width == 4 {
        decimal(&date[..4])?
    } else {
        let magnitude = decimal(&date[1..7])?;
        if date[0] == b'-' {
            if magnitude == 0 {
                return None;
            }
            -magnitude
        } else {
            magnitude
        }
    };
    let month = decimal(&date[year_width + 1..year_width + 3])?;
    let day = decimal(&date[year_width + 4..])?;
    if !(1..=12).contains(&month) || day < 1 || day > month_days(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// March-based Gregorian eras keep leap days at the end of each 400-year era.
// Euclidean division is essential for years before zero.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let march_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * march_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn time_millis(time: &[u8]) -> Option<i64> {
    let clock = time.get(..9)?;
    if clock[0] != b'T' || clock[3] != b':' || clock[6] != b':' {
        return None;
    }
    let hours = decimal(&clock[1..3])?;
    let minutes = decimal(&clock[4..6])?;
    let seconds = decimal(&clock[7..9])?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let (fraction, zone_start) = if time.get(9) == Some(&b'.') {
        fraction_millis(time.get(10..)?)?
    } else {
        (0, 9)
    };
    Some(
        ((hours * 60 + minutes) * 60 + seconds) * 1000 + fraction
            - offset_millis(time.get(zone_start..)?)?,
    )
}

fn fraction_millis(after_dot: &[u8]) -> Option<(i64, usize)> {
    let digits = after_dot
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let retained = digits.min(3);
    let millis = decimal(&after_dot[..retained])? * 10_i64.pow((3 - retained) as u32);
    Some((millis, 10 + digits))
}

fn offset_millis(zone: &[u8]) -> Option<i64> {
    if zone == b"Z" {
        return Some(0);
    }
    if zone.len() != 6 || !matches!(zone[0], b'+' | b'-') || zone[3] != b':' {
        return None;
    }
    let hours = decimal(&zone[1..3])?;
    let minutes = decimal(&zone[4..6])?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let sign = if zone[0] == b'+' { 1 } else { -1 };
    Some(sign * (hours * 60 + minutes) * 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_standardized_and_native_fraction_instants() {
        for (text, millis) in [
            ("1970-01-01", 0),
            ("2026-10-02", 1_790_899_200_000),
            ("1970-01-01T00:00:00Z", 0),
            ("1970-01-01T00:00:00.001Z", 1),
            ("1970-01-01T00:00:00.1Z", 100),
            ("1970-01-01T00:00:00.12Z", 120),
            ("1970-01-01T00:00:00.123456789Z", 123),
            ("1969-12-31T23:59:59.999999999Z", -1),
            ("1969-12-31T23:59:59.999Z", -1),
            ("1970-01-01T01:30:00+01:30", 0),
            ("1970-01-01T00:00:00-01:30", 5_400_000),
            ("2000-02-29T00:00:00Z", 951_782_400_000),
            ("0000-01-01", -62_167_219_200_000),
            ("+010000-01-01", 253_402_300_800_000),
            ("-000001-01-01", -62_198_755_200_000),
            ("+275760-09-13T00:00:00.000Z", 8_640_000_000_000_000),
            ("-271821-04-20T00:00:00.000Z", -8_640_000_000_000_000),
        ] {
            assert_eq!(parse(Some(text)), QueryTimestamp::Known(millis), "{text}");
        }
    }
    #[test]
    fn missing_is_distinct_from_unsupported() {
        for value in [None, Some("")] {
            assert_eq!(parse(value), QueryTimestamp::Unknown);
        }
        for text in [
            " ",
            "2026",
            "2026-10",
            "10/02/2026",
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-00-01",
            "2026-13-01",
            "2026-01-00",
            "2026-01-32",
            "2026-10-02t00:00:00z",
            "2026-10-02T00:00:00",
            "2026-10-02T24:00:00Z",
            "2026-10-02T00:00:60Z",
            "2026-10-02T00:00:00.Z",
            "2026-10-02T00:00:00.1234567890Z",
            "2026-10-02T00:00:00+24:00",
            "2026-10-02T00:00:00+00:60",
            "2026-10-02T00:00:00+0000",
            "-000000-01-01",
            "+275760-09-13T00:00:00.001Z",
            "-271821-04-19T23:59:59.999Z",
            "é000-01-01",
            "2026-10-02T00:00:00Z ",
            "2026-10-02T00:00:00.0000000001Z",
        ] {
            assert_eq!(parse(Some(text)), QueryTimestamp::Unsupported, "{text}");
        }
    }
}
