//! Recognising a harness saying it has run out of usage.
//!
//! # Why this is a string match, which is not what anyone would choose
//!
//! On 2026-09-20 every Claude-harness agent on one machine stopped between
//! 21:30Z and 22:25Z, and the only record of why — anywhere, in any log or
//! store — was one line per agent in the conversation timeline:
//!
//! ```text
//! You've hit your session limit · resets 6:20pm (America/New_York)
//! ```
//!
//! The Claude ADK puts that sentence in the assistant's ordinary text output, so
//! `adk/reader.rs` read it and filed it, faithfully, as the agent talking. There
//! was no `result` record with `is_error`, no 429, nothing on stderr, and no
//! non-zero exit anybody had recorded. The turn simply stopped: one agent went
//! silent for fifty-five minutes with an uncommitted tree and nothing above it
//! was told.
//!
//! So until a harness gives us something typed, the sentence IS the signal, and
//! this module is the one place that decides what counts as it.
//!
//! # The rule, and why it is deliberately narrow
//!
//! The whole trimmed text must BE the sentence. Not contain it — be it.
//!
//! An agent that merely writes about the limit must not trip this, and that is
//! not a hypothetical: the issue comment reporting this finding quoted the
//! sentence twice, and a substring match would have hung a "you are out of
//! usage" banner over every conversation on the device because somebody
//! described the feature. Under-detecting an odd future wording costs one
//! outage's worth of silence; over-detecting costs every reader's trust in the
//! banner. So the match is anchored at both ends, and the caller applies it only
//! to the last thing a turn produced.
//!
//! Tolerant only where the harness might reasonably vary and the meaning cannot:
//! "session limit" or "usage limit", either apostrophe, a reset clause that
//! names a zone or does not, and no reset clause at all.

use time::OffsetDateTime;

/// What a harness said, and when it says the limit lifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimitSaid {
    /// The harness's own sentence, kept whole: it is what the reader is shown
    /// behind the banner, and inventing a paraphrase of a thing we recognised by
    /// its exact shape would be the wrong way round.
    pub said: String,
    /// The wall clock it named, or `None` when it named none.
    pub reset_clock: Option<ResetClock>,
}

/// A reset time as the harness states it: a wall clock and, usually, the zone to
/// read it in. Deliberately not an instant — resolving one needs a zone database
/// this crate does not carry, and that is the caller's problem, not the parse's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetClock {
    /// 0–23, converted from the 12-hour clock the harness writes.
    pub hour: u8,
    pub minute: u8,
    /// The IANA zone in parentheses, when there was one.
    pub zone: Option<String>,
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

/// The two nouns seen or plausible for the same thing.
const LIMIT_NOUNS: [&str; 2] = ["session limit", "usage limit"];

/// Whether this text is a harness saying it is out of usage, and nothing else.
///
/// `text` is one assistant text block. The caller is responsible for it being
/// the LAST thing the turn produced: a limit sentence in the middle of a turn
/// that then carried on working is a quote, not a verdict.
pub fn usage_limit_said(text: &str) -> Option<UsageLimitSaid> {
    let said = text.trim();
    let body = strip_opening(said)?;
    let (noun, rest) = LIMIT_NOUNS
        .iter()
        .find_map(|noun| body.strip_prefix(*noun).map(|rest| (*noun, rest)))?;
    let _ = noun;
    Some(UsageLimitSaid {
        said: said.to_string(),
        reset_clock: reset_clause(rest.trim_start())?,
    })
}

/// `You've hit your ` / `You’ve hit your `, and nothing before it. Both
/// apostrophes, because which one a harness emits is not something to depend on.
fn strip_opening(said: &str) -> Option<&str> {
    for opening in ["You've hit your ", "You’ve hit your "] {
        if let Some(rest) = said.strip_prefix(opening) {
            return Some(rest);
        }
    }
    None
}

/// What follows the noun: nothing at all, or a reset clause and nothing after it.
///
/// Returns `Some(None)` for "no reset time was named" and `None` for "this is not
/// the sentence" — the difference between a limit whose lift time is unknown and
/// a piece of prose that merely began like one.
fn reset_clause(rest: &str) -> Option<Option<ResetClock>> {
    if rest.is_empty() {
        return Some(None);
    }
    // The separator the harness uses is a middle dot; a hyphen is the obvious
    // variant and costs nothing to accept.
    let after = rest
        .strip_prefix('·')
        .or_else(|| rest.strip_prefix('-'))
        .or_else(|| rest.strip_prefix('—'))?
        .trim_start();
    let clock = after.strip_prefix("resets")?.trim_start();
    parse_clock(clock).map(Some)
}

/// `6:20pm (America/New_York)`, `6:20 PM`, `6pm` — and nothing trailing.
fn parse_clock(clock: &str) -> Option<ResetClock> {
    let (time_part, zone) = match clock.split_once('(') {
        Some((before, after)) => (before.trim_end(), Some(after.strip_suffix(')')?.trim())),
        None => (clock, None),
    };
    if zone.is_some_and(str::is_empty) {
        return None;
    }
    let (hour, minute) = parse_twelve_hour(time_part.trim())?;
    Some(ResetClock {
        hour,
        minute,
        zone: zone.map(str::to_string),
    })
}

/// A 12-hour clock to 24-hour parts. Rejects anything with a character left over,
/// which is what keeps a sentence with prose after the time from matching.
fn parse_twelve_hour(text: &str) -> Option<(u8, u8)> {
    let lowered = text.to_ascii_lowercase();
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
            zone: zone.map(str::to_string),
        })
    }

    /// The table. Left: what an assistant text block held. Right: what it means.
    #[test]
    fn the_sentences_that_are_a_usage_limit_and_the_prose_that_is_not() {
        let cases: Vec<(&str, Option<Option<ResetClock>>)> = vec![
            // The one we have evidence for, and the variants that cannot mean
            // anything else.
            (EVIDENCED, Some(clock(18, 20, Some("America/New_York")))),
            (
                "You've hit your usage limit · resets 6:20pm (America/New_York)",
                Some(clock(18, 20, Some("America/New_York"))),
            ),
            (
                "You’ve hit your session limit · resets 6:20pm (America/New_York)",
                Some(clock(18, 20, Some("America/New_York"))),
            ),
            ("You've hit your session limit · resets 6:20pm", Some(clock(18, 20, None))),
            ("You've hit your session limit · resets 6:20 PM", Some(clock(18, 20, None))),
            ("You've hit your session limit · resets 6pm", Some(clock(18, 0, None))),
            ("You've hit your session limit · resets 12am", Some(clock(0, 0, None))),
            ("You've hit your session limit · resets 12pm", Some(clock(12, 0, None))),
            ("You've hit your session limit - resets 7:05am (UTC)", Some(clock(7, 5, Some("UTC")))),
            // A limit with no reset time is still a limit; the banner says the
            // time is unknown rather than guessing one.
            ("You've hit your session limit", Some(None)),
            ("   You've hit your session limit   ", Some(None)),
            // And the prose. The first of these is the case that matters: the
            // issue comment reporting this finding quoted the sentence, and a
            // substring match would have bannered the whole device for it.
            (
                "The harness says \"You've hit your session limit · resets 6:20pm (America/New_York)\" and nothing else records it.",
                None,
            ),
            (
                "You've hit your session limit · resets 6:20pm (America/New_York) — so I stopped there.",
                None,
            ),
            (
                "Quoting for the record: You've hit your session limit · resets 6:20pm",
                None,
            ),
            ("You've hit your stride", None),
            ("You've hit your session limit yesterday too", None),
            ("You've hit your session limit · resets soon", None),
            ("You've hit your session limit · resets 25:00pm", None),
            ("You've hit your session limit · resets 6:20pm ()", None),
            ("I have hit my session limit", None),
            ("", None),
            ("Reading the transport modules now.", None),
        ];

        for (text, expected) in cases {
            let found = usage_limit_said(text);
            match expected {
                Some(reset_clock) => {
                    let found = found.unwrap_or_else(|| panic!("not recognised: {text:?}"));
                    assert_eq!(found.reset_clock, reset_clock, "reset clock for {text:?}");
                    assert_eq!(
                        found.said,
                        text.trim(),
                        "the harness's own words for {text:?}"
                    );
                }
                None => assert!(found.is_none(), "should not be a limit: {text:?}"),
            }
        }
    }

    /// A clock in a zone, as an instant, for the tests to read.
    fn resolve(text: &str, now_unix: i64) -> Option<i64> {
        let said = usage_limit_said(text).expect("a limit sentence");
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
