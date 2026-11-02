//! When the daily idle-workspace digest is due (#958).
//!
//! Each project agent hears about its quiet workspaces once a day, at a time
//! of day on the device's own clock. The digest is due from that time until
//! the local day ends: a bridge that was down at the time sends it at its
//! first sweep after, and a day's digest that never went out is not sent the
//! next morning before that day's time.

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use std::fmt;
use std::time::Duration;

/// The shortest the service waits for the digest.
const MIN_WAIT: Duration = Duration::from_secs(60);

/// 09:00 on the device's clock.
pub const DEFAULT_DIGEST_AT: DigestTime = DigestTime { hour: 9, minute: 0 };

/// A time of day, as the `workspace_idle_digest_at` setting writes it:
/// `HH:MM`, 24-hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestTime {
    hour: i8,
    minute: i8,
}

impl DigestTime {
    pub fn new(hour: i8, minute: i8) -> Option<DigestTime> {
        ((0..24).contains(&hour) && (0..60).contains(&minute))
            .then_some(DigestTime { hour, minute })
    }

    /// `HH:MM` exactly, two digits each; anything else is `None`.
    pub fn parse(raw: &str) -> Option<DigestTime> {
        let (hour, minute) = raw.split_once(':')?;
        let two_digits = |part: &str| {
            (part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<i8>().ok())
                .flatten()
        };
        DigestTime::new(two_digits(hour)?, two_digits(minute)?)
    }

    fn on(self, date: Date) -> DateTime {
        date.at(self.hour, self.minute, 0, 0)
    }
}

impl fmt::Display for DigestTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// When the digest goes out: a time of day in the device's zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestSchedule {
    pub at: DigestTime,
    /// `None` is the device's zone, read again each time it is needed, so a
    /// machine that moves zones while the bridge runs follows it.
    pub zone: Option<TimeZone>,
}

impl Default for DigestSchedule {
    fn default() -> Self {
        Self {
            at: DEFAULT_DIGEST_AT,
            zone: None,
        }
    }
}

impl DigestSchedule {
    /// The local day whose digest is due at `now_ms`: today, once today's
    /// time has come. `None` before it.
    pub fn due_day(&self, now_ms: i64) -> Option<Date> {
        let local = self.local(now_ms)?;
        (local >= self.at.on(local.date())).then(|| local.date())
    }

    /// How long from `now_ms` until the digest is next due: today's time if
    /// it is still ahead, else tomorrow's. A time the clocks repeat counts
    /// from its first showing, unless that has passed. Never less than a
    /// minute, so a clock that cannot be read never has the service sweep
    /// back to back.
    pub fn until_next(&self, now_ms: i64) -> Duration {
        let Some(local) = self.local(now_ms) else {
            return Duration::from_secs(24 * 60 * 60);
        };
        let today = self.at.on(local.date());
        let next = if local < today {
            today
        } else {
            self.at.on(local.date().tomorrow().unwrap_or(local.date()))
        };
        let showings = self.zone().to_ambiguous_timestamp(next);
        let next_ms = [showings.compatible(), showings.later()]
            .into_iter()
            .filter_map(Result::ok)
            .map(|at| at.as_millisecond())
            .find(|at| *at > now_ms)
            .unwrap_or(now_ms);
        let wait = Duration::from_millis(u64::try_from(next_ms - now_ms).unwrap_or(0));
        wait.max(MIN_WAIT)
    }

    fn local(&self, now_ms: i64) -> Option<DateTime> {
        let now = Timestamp::from_millisecond(now_ms).ok()?;
        Some(self.zone().to_datetime(now))
    }

    fn zone(&self) -> TimeZone {
        self.zone.clone().unwrap_or_else(TimeZone::system)
    }
}

#[cfg(test)]
mod tests;
