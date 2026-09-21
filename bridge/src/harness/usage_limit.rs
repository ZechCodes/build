//! What a harness said when it ran out of usage, and when it says the limit
//! lifts.
//!
//! # How a limit is recognised: structurally, not by its words
//!
//! On 2026-09-20 every Claude-harness agent on one machine stopped between
//! 21:30Z and 22:25Z. What each conversation showed was one line of assistant
//! text:
//!
//! ```text
//! You've hit your session limit · resets 6:20pm (America/New_York)
//! ```
//!
//! Claude's own transcript of the same moment marks that message for what it
//! is: `model: "<synthetic>"`, `isApiErrorMessage: true`, `error: "rate_limit"`.
//! The CLI's stream-json schema carries `error` on every `assistant` line, so
//! the ADK reader recognises a limit by `error == "rate_limit"` and never by
//! prose (issue #58, decision of 2026-09-21 16:06Z). An agent that quotes the
//! sentence cannot set that field, which is what makes a false banner
//! impossible rather than merely unlikely.
//!
//! So this module decides nothing about WHETHER a limit was hit. It keeps what
//! the harness said, verbatim, and reads a reset clock out of it when there is
//! one, whatever the limit is called: "session limit", "weekly limit", "Opus
//! limit", "usage limit" — the CLI names each window differently, and a
//! weekly reset more than a day out carries a date ("Sep 25, 6pm"). When the
//! CLI's `rate_limit_event` supplies the reset as an instant, that wins, and
//! this parse is the fallback.

use time::OffsetDateTime;

/// What a harness said, and when it says the limit lifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimitSaid {
    /// The harness's own sentence, kept whole: it is what the reader is shown
    /// behind the banner.
    pub said: String,
    /// The wall clock it named, or `None` when it named none that could be read.
    pub reset_clock: Option<ResetClock>,
}

/// A reset time as the harness states it: a wall clock, the date when it gave
/// one, and usually the zone to read it in. Deliberately not an instant: that
/// needs the zone's rules on the day it falls, which [`resolved_reset`] applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetClock {
    /// 0–23, converted from the 12-hour clock the harness writes.
    pub hour: u8,
    pub minute: u8,
    /// The date, when the reset is far enough out for the harness to name one.
    pub date: Option<ResetDate>,
    /// The IANA zone in parentheses, when there was one.
    pub zone: Option<String>,
}

/// A month and day, and the year when the harness wrote one (it does only when
/// the reset falls in another year).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetDate {
    /// 1–12.
    pub month: u8,
    pub day: u8,
    pub year: Option<i16>,
}

/// A harness that has run out of usage, as everything above the adapter reads it:
/// what it said, and when it lifts.
///
/// The resolved form of [`UsageLimitSaid`] — the clock and zone turned into an
/// instant — because nothing above this module should be doing zone arithmetic,
/// and because `resets_at: None` is a state the banner has words for ("reset time
/// unknown") rather than a gap to paper over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimited {
    /// The harness's own sentence, shown to the reader behind the banner.
    pub said: String,
    /// When the limit lifts, when it could be known.
    pub resets_at: Option<OffsetDateTime>,
}

impl UsageLimitSaid {
    /// This limit as the rest of the bridge reads it, with the reset resolved.
    pub fn resolved(&self, now: OffsetDateTime) -> UsageLimited {
        UsageLimited {
            said: self.said.clone(),
            resets_at: resolved_reset(self, now),
        }
    }
}

/// What a message the harness marked as a usage limit said, with the reset
/// clock read out of it when it names one.
///
/// Called only on a message already recognised as a limit (see the module
/// docs); `text` is kept verbatim, whatever it says.
pub fn usage_limit_said(text: &str) -> UsageLimitSaid {
    let said = text.trim();
    UsageLimitSaid {
        said: said.to_string(),
        reset_clock: reset_clock_in(said),
    }
}

/// The clock after the last "resets" in the sentence: `6:20pm (America/New_York)`,
/// `Sep 25, 6pm (America/New_York)`, `Jan 2, 2027, 9:30am`, `6 PM`. Anything it
/// cannot read wholly is no clock at all, rather than a guess.
fn reset_clock_in(said: &str) -> Option<ResetClock> {
    let (_, clause) = said.rsplit_once("resets ")?;
    let (before_zone, zone) = match clause.split_once('(') {
        Some((before, after)) => (before, Some(after.split_once(')')?.0.trim())),
        None => (clause, None),
    };
    if zone.is_some_and(str::is_empty) {
        return None;
    }
    let when = before_zone.trim().trim_end_matches('.');
    let (date, time) = split_date(when)?;
    let (hour, minute) = parse_twelve_hour(time)?;
    Some(ResetClock {
        hour,
        minute,
        date,
        zone: zone.map(str::to_string),
    })
}

/// `Sep 25, 6pm` into its date and its time; a bare `6pm` has no date.
fn split_date(when: &str) -> Option<(Option<ResetDate>, &str)> {
    let Some((month, rest)) = when.split_once(' ') else {
        return Some((None, when));
    };
    let Some(month) = month_number(month) else {
        return Some((None, when));
    };
    let mut parts = rest.split(',').map(str::trim);
    let day: u8 = parts.next()?.parse().ok()?;
    let mut next = parts.next()?;
    let year = match next.parse::<i16>() {
        Ok(year) => {
            next = parts.next()?;
            Some(year)
        }
        Err(_) => None,
    };
    if parts.next().is_some() || !(1..=31).contains(&day) {
        return None;
    }
    let time = next.strip_prefix("at ").unwrap_or(next);
    Some((Some(ResetDate { month, day, year }), time))
}

fn month_number(name: &str) -> Option<u8> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lowered = name.trim_end_matches('.').to_ascii_lowercase();
    let short = lowered.get(..3)?;
    let number = MONTHS.iter().position(|month| *month == short)?;
    // "Sept" and full names are fine; "Separately" is not a month.
    let full = [
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
    ][number];
    (full.starts_with(lowered.as_str()) || lowered == "sept").then(|| number as u8 + 1)
}

/// A 12-hour clock to 24-hour parts. Rejects anything with a character left over.
fn parse_twelve_hour(text: &str) -> Option<(u8, u8)> {
    let lowered = text.trim().to_ascii_lowercase();
    let (digits, after_noon) = match (lowered.strip_suffix("am"), lowered.strip_suffix("pm")) {
        (Some(morning), _) => (morning.trim_end(), false),
        (_, Some(afternoon)) => (afternoon.trim_end(), true),
        _ => return None,
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((hour, minute)) => (hour, Some(minute)),
        None => (digits, None),
    };
    let hour: u8 = hour.parse().ok()?;
    if !(1..=12).contains(&hour) {
        return None;
    }
    let minute: u8 = match minute {
        Some(minute) if minute.len() == 2 => minute.parse().ok()?,
        Some(_) => return None,
        None => 0,
    };
    if minute > 59 {
        return None;
    }
    // 12am is midnight and 12pm is noon: the one case where the arithmetic is not
    // "add twelve for the afternoon".
    let hour = match (hour, after_noon) {
        (12, false) => 0,
        (12, true) => 12,
        (hour, false) => hour,
        (hour, true) => hour + 12,
    };
    Some((hour, minute))
}

/// The instant a harness's reset clause means, resolved in the zone it named.
///
/// # Why this is zone-aware rather than offset-aware
///
/// The harness gives a wall clock and a zone and no date, so two things have to
/// be worked out: which day that clock next falls on, and what offset the zone is
/// on THAT day. A fixed offset cannot do the second. A limit reported at 01:30 on
/// the night America/New_York leaves daylight saving, resetting at 6:20pm, resets
/// at 23:20Z and not 22:20Z — the offset changes between the two moments, and a
/// countdown built on the offset at the time of reading would be an hour wrong
/// exactly twice a year.
///
/// `jiff` is used for this and confined to this module: it carries a bundled zone
/// database (so a Windows host or a container with no `/usr/share/zoneinfo` still
/// resolves), prefers the system copy where there is one, and applies the zone's
/// own rules to a civil datetime. Everything crossing this module's boundary
/// stays `time::OffsetDateTime`, as the rest of the crate speaks.
///
/// `None` covers every way the answer can be unknowable: the harness named no
/// time, it named no zone, or it named a zone no database has.
pub fn resolved_reset(said: &UsageLimitSaid, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let clock = said.reset_clock.as_ref()?;
    let zone = jiff::tz::TimeZone::get(clock.zone.as_deref()?).ok()?;
    let now_there = jiff::Timestamp::from_second(now.unix_timestamp())
        .ok()?
        .to_zoned(zone.clone());

    if let Some(date) = clock.date {
        return dated_reset(date, clock, &zone, &now_there);
    }

    // Today in that zone, at that clock. `to_zoned` applies the zone's rules,
    // which is what makes the two awkward nights right rather than approximate:
    // a clock in the hour that does not exist on the spring change is moved
    // forward, and one in the hour that happens twice on the autumn change takes
    // the first of them.
    let today = at_clock(now_there.date(), clock, &zone)?;
    let resets = if today.timestamp() > now_there.timestamp() {
        today
    } else {
        // Tomorrow's date resolved in the zone afresh, NOT today's instant plus
        // twenty-four hours: on a change day those differ by an hour, and the
        // wall clock the harness named is the thing to honour.
        at_clock(now_there.date().tomorrow().ok()?, clock, &zone)?
    };
    OffsetDateTime::from_unix_timestamp(resets.timestamp().as_second()).ok()
}

/// A reset the harness dated. The year is the one it wrote, or this year in
/// that zone — unless that is already more than a day gone, when the harness
/// can only have meant next year (it writes the year only for another year).
fn dated_reset(
    date: ResetDate,
    clock: &ResetClock,
    zone: &jiff::tz::TimeZone,
    now_there: &jiff::Zoned,
) -> Option<OffsetDateTime> {
    let month = i8::try_from(date.month).ok()?;
    let day = i8::try_from(date.day).ok()?;
    let on = |year: i16| {
        jiff::civil::Date::new(year, month, day)
            .ok()
            .and_then(|civil| at_clock(civil, clock, zone))
    };
    let resets = match date.year {
        Some(year) => on(year)?,
        None => {
            let this_year = on(now_there.year())?;
            let a_day_ago = now_there.timestamp() - jiff::SignedDuration::from_hours(24);
            if this_year.timestamp() < a_day_ago {
                on(now_there.year() + 1)?
            } else {
                this_year
            }
        }
    };
    OffsetDateTime::from_unix_timestamp(resets.timestamp().as_second()).ok()
}

/// One date in one zone at the harness's clock.
fn at_clock(
    date: jiff::civil::Date,
    clock: &ResetClock,
    zone: &jiff::tz::TimeZone,
) -> Option<jiff::Zoned> {
    let hour = i8::try_from(clock.hour).ok()?;
    let minute = i8::try_from(clock.minute).ok()?;
    date.at(hour, minute, 0, 0).to_zoned(zone.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentence exactly as the Claude ADK produced it on 2026-09-20.
    const EVIDENCED: &str = "You've hit your session limit · resets 6:20pm (America/New_York)";

    fn clock(hour: u8, minute: u8, zone: Option<&str>) -> Option<ResetClock> {
        Some(ResetClock {
            hour,
            minute,
            date: None,
            zone: zone.map(str::to_string),
        })
    }

    fn dated(
        month: u8,
        day: u8,
        year: Option<i16>,
        hour: u8,
        minute: u8,
        zone: Option<&str>,
    ) -> Option<ResetClock> {
        Some(ResetClock {
            hour,
            minute,
            date: Some(ResetDate { month, day, year }),
            zone: zone.map(str::to_string),
        })
    }

    /// The table. Left: what a message the harness marked as a limit said.
    /// Right: the reset clock read out of it. Every one is a limit — whether
    /// it is one was decided before this parse, by the message's `error`.
    #[test]
    fn a_reset_clock_is_read_from_any_wording_of_a_limit() {
        let cases: Vec<(&str, Option<ResetClock>)> = vec![
            (EVIDENCED, clock(18, 20, Some("America/New_York"))),
            (
                "You've hit your weekly limit · resets Sep 25, 6pm (America/New_York)",
                dated(9, 25, None, 18, 0, Some("America/New_York")),
            ),
            (
                "You've hit your Opus limit · resets Sep 25, 6:30pm (Europe/London)",
                dated(9, 25, None, 18, 30, Some("Europe/London")),
            ),
            (
                "You've hit your Sonnet limit · resets Jan 2, 2027, 9am (UTC)",
                dated(1, 2, Some(2027), 9, 0, Some("UTC")),
            ),
            (
                "You've hit your usage limit · resets 6:20pm (America/New_York)",
                clock(18, 20, Some("America/New_York")),
            ),
            (
                "You’ve hit your session limit · resets 6:20 PM",
                clock(18, 20, None),
            ),
            (
                "You've hit your session limit · resets 6pm",
                clock(18, 0, None),
            ),
            (
                "You've hit your session limit · resets 12am",
                clock(0, 0, None),
            ),
            (
                "You've hit your session limit · resets 12pm",
                clock(12, 0, None),
            ),
            (
                "You've hit your session limit - resets 7:05am (UTC).",
                clock(7, 5, Some("UTC")),
            ),
            (
                "   You've hit your session limit · resets 6pm   ",
                clock(18, 0, None),
            ),
            // A limit whose reset could not be read is still recorded, with the
            // reset unknown rather than guessed.
            ("You've hit your session limit", None),
            ("You've hit your session limit · resets soon", None),
            ("You've hit your session limit · resets 25:00pm", None),
            ("You've hit your session limit · resets 6:20pm ()", None),
            ("Usage credits required for 1M context", None),
            ("", None),
        ];

        for (text, expected) in cases {
            let found = usage_limit_said(text);
            assert_eq!(found.reset_clock, expected, "reset clock for {text:?}");
            assert_eq!(
                found.said,
                text.trim(),
                "the harness's own words for {text:?}"
            );
        }
    }

    /// A dated reset is that date, not the next time the clock comes round.
    #[test]
    fn a_weekly_reset_resolves_on_the_date_it_names() {
        let now = 1_789_939_878; // 2026-09-20T21:31:18Z
        let resets = resolve(
            "You've hit your weekly limit · resets Sep 25, 6pm (America/New_York)",
            now,
        )
        .expect("resolves");
        // 2026-09-25 18:00 EDT = 22:00Z. Midnight UTC on the 25th is
        // 1_789_862_400 + 5 days.
        assert_eq!(resets, 1_789_862_400 + 5 * 86_400 + 79_200);

        let next_year = resolve(
            "You've hit your weekly limit · resets Jan 2, 6pm (America/New_York)",
            now,
        )
        .expect("resolves");
        assert!(
            next_year > now + 90 * 86_400,
            "a date already past this year with no year written is next year's"
        );
    }

    /// A clock in a zone, as an instant, for the tests to read.
    fn resolve(text: &str, now_unix: i64) -> Option<i64> {
        let said = usage_limit_said(text);
        let now = OffsetDateTime::from_unix_timestamp(now_unix).unwrap();
        resolved_reset(&said, now).map(OffsetDateTime::unix_timestamp)
    }

    /// The evidenced case end to end: the sentence, read in the zone it names,
    /// resolves to the instant the outage actually lifted.
    #[test]
    fn the_evidenced_sentence_resolves_to_the_instant_the_outage_lifted() {
        // 2026-09-20T21:31:18Z, when the harness said it.
        let resets = resolve(EVIDENCED, 1_789_939_878).expect("America/New_York resolves");

        // 6:20pm EDT is 22:20Z: midnight of the 20th plus 22h20m.
        assert_eq!(resets, 1_789_862_400 + 80_400);
    }

    /// The date the harness did not give. A clock still ahead is today's; one
    /// already past is tomorrow's, which is what stops a limit reported at 21:31Z
    /// from reading as having reset hours ago.
    #[test]
    fn the_reset_date_is_the_next_time_that_clock_comes_round() {
        let now = 1_789_939_878; // 2026-09-20T21:31:18Z

        let ahead = resolve("You've hit your session limit · resets 11:00pm (UTC)", now).unwrap();
        assert_eq!(ahead, 1_789_862_400 + 82_800, "23:00Z the same day");

        let passed = resolve("You've hit your session limit · resets 6:00am (UTC)", now).unwrap();
        assert!(
            passed > now,
            "a clock already past today resolves to tomorrow"
        );
        assert_eq!(passed, 1_789_862_400 + 86_400 + 21_600, "06:00Z tomorrow");
    }

    /// The night the zone changes, which is the whole reason this resolves in the
    /// zone rather than against an offset read at the time of the limit.
    ///
    /// America/New_York leaves daylight saving at 02:00 local on 2026-11-01,
    /// falling back to UTC-5. A limit hit at 01:30 EDT (05:30Z) that resets at
    /// 6:20pm resets at 23:20Z — an hour later than the UTC-4 offset in force when
    /// it was read would have said.
    #[test]
    fn a_reset_across_a_daylight_saving_change_uses_the_offset_of_the_day_it_falls_on() {
        // 2026-11-01T05:30:00Z — 01:30 EDT, half an hour before the change.
        let at_the_change = 1_793_511_000;
        let resets = resolve(
            "You've hit your session limit · resets 6:20pm (America/New_York)",
            at_the_change,
        )
        .expect("resolves");

        // 18:20 EST = 23:20Z. Midnight UTC on 2026-11-01 is 1_793_491_200.
        assert_eq!(resets, 1_793_491_200 + 84_000, "6:20pm EST, not EDT");
        // And the offset in force when the limit was READ would have given 22:20Z,
        // which is the wrong answer this test exists to rule out.
        assert_ne!(resets, 1_793_491_200 + 80_400);
    }

    /// The two clocks a zone change makes strange: one that happens twice, and one
    /// that does not happen at all. Neither may produce no answer — a banner with
    /// no countdown because the reset fell in a folded hour would be a worse
    /// failure than a minute's imprecision.
    #[test]
    fn an_ambiguous_or_skipped_reset_clock_still_resolves() {
        // 01:30 happens twice on 2026-11-01 in New York.
        let folded = resolve(
            "You've hit your session limit · resets 1:30am (America/New_York)",
            1_793_500_000,
        );
        assert!(
            folded.is_some(),
            "a clock that happens twice still resolves"
        );

        // 02:30 does not exist on 2026-03-08 in New York: the clock jumps 02:00→03:00.
        // 2026-03-08T06:00:00Z is 01:00 EST, before the jump.
        let skipped = resolve(
            "You've hit your session limit · resets 2:30am (America/New_York)",
            1_772_949_600,
        );
        assert!(
            skipped.is_some(),
            "a clock that does not exist still resolves"
        );
    }

    /// No zone, no clock, or a zone no database has: no instant, and the reader is
    /// told the reset time is unknown rather than shown a countdown from a guess.
    #[test]
    fn a_limit_with_nothing_resolvable_yields_no_instant() {
        let now = 1_789_939_878;
        assert_eq!(
            resolve("You've hit your session limit · resets 6:20pm", now),
            None,
            "no zone"
        );
        assert_eq!(
            resolve("You've hit your session limit", now),
            None,
            "no clock"
        );
        assert_eq!(
            resolve(
                "You've hit your session limit · resets 6:20pm (Mars/Olympus)",
                now
            ),
            None,
            "a zone no database has"
        );
    }
}
