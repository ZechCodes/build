use super::*;

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// 2026-11-02T00:00:00Z.
const MIDNIGHT_UTC_MS: i64 = 1_793_577_600_000;

fn utc_at(hour: i8, minute: i8) -> DigestSchedule {
    DigestSchedule {
        at: DigestTime::new(hour, minute).unwrap(),
        zone: Some(TimeZone::UTC),
    }
}

#[test]
fn a_digest_time_is_two_digit_hours_and_minutes() {
    assert_eq!(DigestTime::parse("09:00"), DigestTime::new(9, 0));
    assert_eq!(DigestTime::parse("23:59"), DigestTime::new(23, 59));
    assert_eq!(DigestTime::parse("00:00"), DigestTime::new(0, 0));
    for refused in [
        "9:00", "24:00", "12:60", "12:5", "1200", "12:00:00", " 09:00", "ab:cd", "",
    ] {
        assert_eq!(DigestTime::parse(refused), None, "{refused:?}");
    }
    assert_eq!(DigestTime::new(7, 5).unwrap().to_string(), "07:05");
    assert_eq!(DEFAULT_DIGEST_AT.to_string(), "09:00");
}

#[test]
fn the_digest_is_due_from_its_time_until_the_day_ends() {
    let schedule = utc_at(9, 0);
    let today = jiff::civil::date(2026, 11, 2);
    assert_eq!(schedule.due_day(MIDNIGHT_UTC_MS + 9 * HOUR_MS - 1), None);
    assert_eq!(schedule.due_day(MIDNIGHT_UTC_MS + 9 * HOUR_MS), Some(today));
    assert_eq!(
        schedule.due_day(MIDNIGHT_UTC_MS + DAY_MS - 1),
        Some(today),
        "a bridge started late in the day still owes today's digest"
    );
    assert_eq!(
        schedule.due_day(MIDNIGHT_UTC_MS + DAY_MS + HOUR_MS),
        None,
        "yesterday's missed digest is not sent before today's time"
    );
}

#[test]
fn the_day_is_the_devices_local_day() {
    let schedule = DigestSchedule {
        at: DigestTime::new(9, 0).unwrap(),
        zone: Some(TimeZone::fixed(jiff::tz::offset(-5))),
    };
    // 09:00 at UTC-5 is 14:00Z.
    assert_eq!(schedule.due_day(MIDNIGHT_UTC_MS + 13 * HOUR_MS), None);
    assert_eq!(
        schedule.due_day(MIDNIGHT_UTC_MS + 14 * HOUR_MS),
        Some(jiff::civil::date(2026, 11, 2))
    );
    // 03:00Z on the 3rd is still the 2nd at UTC-5.
    assert_eq!(
        schedule.due_day(MIDNIGHT_UTC_MS + DAY_MS + 3 * HOUR_MS),
        Some(jiff::civil::date(2026, 11, 2))
    );
}

#[test]
fn the_next_digest_is_today_until_its_time_then_tomorrow() {
    let schedule = utc_at(9, 0);
    assert_eq!(
        schedule.until_next(MIDNIGHT_UTC_MS + 8 * HOUR_MS + 30 * MINUTE_MS),
        Duration::from_secs(30 * 60)
    );
    assert_eq!(
        schedule.until_next(MIDNIGHT_UTC_MS + 9 * HOUR_MS),
        Duration::from_secs(24 * 3600)
    );
    assert_eq!(
        schedule.until_next(MIDNIGHT_UTC_MS + 10 * HOUR_MS),
        Duration::from_secs(23 * 3600)
    );
}

#[test]
fn a_digest_time_skipped_by_a_clock_change_still_comes_that_day() {
    let zone = TimeZone::get("America/New_York").unwrap();
    let schedule = DigestSchedule {
        at: DigestTime::new(2, 30).unwrap(),
        zone: Some(zone.clone()),
    };
    // 2026-03-08: New York skips from 02:00 to 03:00.
    let before = jiff::civil::date(2026, 3, 8)
        .at(1, 0, 0, 0)
        .to_zoned(zone.clone())
        .unwrap()
        .timestamp()
        .as_millisecond();
    assert_eq!(schedule.due_day(before), None);
    let waited = schedule.until_next(before);
    assert!(
        waited > Duration::ZERO && waited <= Duration::from_secs(2 * 3600),
        "{waited:?}"
    );
    let after = before + i64::try_from(waited.as_millis()).unwrap();
    assert_eq!(schedule.due_day(after), Some(jiff::civil::date(2026, 3, 8)));
}

/// On the night the clocks go back, a time in the repeated hour has passed
/// once already: the wait runs to its second showing, never to nothing, so
/// the service does not sweep back to back.
#[test]
fn a_digest_time_repeated_by_a_clock_change_is_waited_for_again() {
    let zone = TimeZone::get("America/New_York").unwrap();
    let schedule = DigestSchedule {
        at: DigestTime::new(1, 45).unwrap(),
        zone: Some(zone.clone()),
    };
    // 2026-11-01: New York repeats 01:00–02:00. 01:30 the second time is EST.
    let second_half_past_one = jiff::civil::date(2026, 11, 1)
        .at(1, 30, 0, 0)
        .to_zoned(zone)
        .unwrap()
        .timestamp()
        .as_millisecond()
        + HOUR_MS;
    assert_eq!(
        schedule.until_next(second_half_past_one),
        Duration::from_secs(15 * 60)
    );
}
