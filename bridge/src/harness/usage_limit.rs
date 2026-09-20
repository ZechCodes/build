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

use time::{Duration, OffsetDateTime, UtcOffset};

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

/// The instant a clock means, given the offset its zone is on.
///
/// The harness names a time and no date, so the date is inferred: the next time
/// that clock comes round. A limit reported at 21:30Z that resets at "6:20pm" is
/// resetting in fifty minutes, not twenty-three hours ago.
///
/// The offset is the caller's to supply because resolving an IANA zone needs a
/// zone database, and this crate carries none — see `resets_at_in_zone`.
pub fn resets_at(clock: &ResetClock, now: OffsetDateTime, offset: UtcOffset) -> OffsetDateTime {
    let local = now.to_offset(offset);
    let at = local.replace_time(
        time::Time::from_hms(clock.hour, clock.minute, 0).unwrap_or(time::Time::MIDNIGHT),
    );
    if at > now {
        at
    } else {
        at + Duration::days(1)
    }
}

/// The UTC offset a stated zone is on, when it can be known without a zone
/// database.
///
/// # The gap this leaves, deliberately visible
///
/// `time` is compiled here with `formatting` and `parsing` only: no bundled tz
/// database, and not even `local-offset`. So `America/New_York` — the zone the
/// harness actually named — cannot be turned into an offset by this crate, and
/// this returns `None` for it. A limit whose zone cannot be resolved has no
/// instant, and the reader is told the reset time is unknown rather than shown a
/// countdown computed from a guess.
///
/// That is the honest behaviour and not the wanted one: a countdown is the point
/// of the banner. Closing it needs one small dependency (`time-tz` with its
/// bundled database composes with the `time` types already here), which is a
/// supply-chain decision for whoever owns the daemon rather than something to
/// slip into a feature branch. The seam is this function: give it a real lookup
/// and every caller gets the countdown with no other change.
pub fn offset_for_zone(zone: &str) -> Option<UtcOffset> {
    match zone {
        "UTC" | "Etc/UTC" | "Etc/GMT" | "GMT" | "Z" => Some(UtcOffset::UTC),
        _ => None,
    }
}

/// The instant a harness's reset clause means, when the zone can be resolved.
///
/// `None` covers all three ways it can be unknowable: the harness named no time,
/// it named no zone, or it named a zone this build cannot look up
/// ([`offset_for_zone`]).
pub fn resolved_reset(said: &UsageLimitSaid, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let clock = said.reset_clock.as_ref()?;
    let offset = offset_for_zone(clock.zone.as_deref()?)?;
    Some(resets_at(clock, now, offset))
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

    /// The date the harness did not give. A time still ahead is today's; one
    /// already past is tomorrow's, which is what stops a limit reported at 21:30Z
    /// from reading as having reset twenty-three hours ago.
    #[test]
    fn the_reset_date_is_the_next_time_that_clock_comes_round() {
        let utc = UtcOffset::UTC;
        let now = OffsetDateTime::from_unix_timestamp(1_789_939_878).expect("2026-09-20T21:31:18Z");

        let ahead = resets_at(
            &ResetClock {
                hour: 22,
                minute: 20,
                zone: None,
            },
            now,
            utc,
        );
        // 2026-09-20T22:20:00Z — midnight of the 20th plus 22h20m.
        assert_eq!(
            ahead.unix_timestamp(),
            1_789_862_400 + 80_400,
            "22:20Z the same day"
        );

        let passed = resets_at(
            &ResetClock {
                hour: 6,
                minute: 0,
                zone: None,
            },
            now,
            utc,
        );
        assert!(
            passed > now,
            "a time already past today resolves to tomorrow"
        );
        // 2026-09-21T06:00:00Z — midnight of the 20th, plus a day, plus six hours.
        assert_eq!(
            passed.unix_timestamp(),
            1_789_862_400 + 86_400 + 21_600,
            "06:00Z tomorrow"
        );
    }

    /// The seam, and the gap behind it stated as a test so nobody mistakes it for
    /// an oversight: UTC resolves, a real zone does not, and an unresolvable zone
    /// yields no instant rather than a wrong one.
    #[test]
    fn a_zone_this_build_cannot_look_up_yields_no_instant() {
        let now = OffsetDateTime::from_unix_timestamp(1_789_939_878).unwrap();

        let utc = usage_limit_said("You've hit your session limit · resets 11:00pm (UTC)").unwrap();
        assert!(resolved_reset(&utc, now).is_some(), "UTC needs no database");

        let eastern = usage_limit_said(EVIDENCED).unwrap();
        assert_eq!(
            resolved_reset(&eastern, now),
            None,
            "America/New_York needs a zone database this build does not carry"
        );

        let zoneless = usage_limit_said("You've hit your session limit · resets 6:20pm").unwrap();
        assert_eq!(resolved_reset(&zoneless, now), None, "no zone, no instant");

        let timeless = usage_limit_said("You've hit your session limit").unwrap();
        assert_eq!(resolved_reset(&timeless, now), None, "no clock, no instant");
    }

    /// The evidenced case end to end: the sentence, read in the zone it names,
    /// resolves to the instant the outage actually ended.
    #[test]
    fn the_evidenced_sentence_resolves_to_the_instant_the_outage_lifted() {
        let said = usage_limit_said(EVIDENCED).expect("the evidenced sentence");
        let clock = said.reset_clock.expect("a reset clock");
        assert_eq!(clock.zone.as_deref(), Some("America/New_York"));

        // America/New_York was on UTC-4 that day; the caller supplies that.
        let eastern = UtcOffset::from_hms(-4, 0, 0).unwrap();
        let now = OffsetDateTime::from_unix_timestamp(1_789_939_878).unwrap();

        let resets = resets_at(&clock, now, eastern);

        assert_eq!(
            resets.unix_timestamp(),
            1_789_942_800,
            "6:20pm EDT is 22:20Z"
        );
    }
}
