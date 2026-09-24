//! One shape for the lines the daemon's liveness path writes.
//!
//! Stamped, so a bridge log can be laid beside a phone's own diagnostics: on
//! 2026-09-24 the only way to place a relay reconnect against the phone's
//! "ICE disconnected" was to walk from the nearest `rtc:` line that happened
//! to carry a timestamp. And throttled where a fault repeats, so a thousand
//! identical TURN warnings (eight thousand `ChannelBind 400`s in one evening)
//! cannot bury the one line that says a session ended.

use std::fmt::Display;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The moment, as `2026-09-24T02:10:11.123Z`: UTC, milliseconds, the same
/// clock and the same precision the `rtc:` lines' `timestamp_ms` carries.
pub fn stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{}T{:02}:{:02}:{:02}.{:03}Z",
        now.date(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

/// One stamped line on stderr.
pub fn say(line: impl Display) {
    eprintln!("{} {line}", stamp());
}

/// Which of a stream of repeating lines get written: the first of each kind,
/// then one per `window`, each saying how many it stands for.
///
/// A kind is whatever key the caller gives it: a message with its numbers
/// blanked out ([`key_of`]), a session id, a reason. The map is bounded by
/// how many kinds the caller can produce, which for a fixed set of fault
/// messages is a handful.
pub struct Throttle {
    window: Duration,
    /// A list rather than a map so a throttle can be a `static`: the kinds
    /// are a handful of message templates, and a scan of them costs less
    /// than the line it gates.
    seen: Mutex<Vec<(String, Seen)>>,
}

struct Seen {
    last_said: Instant,
    suppressed: u64,
}

impl Throttle {
    pub const fn new(window: Duration) -> Throttle {
        Throttle {
            window,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Whether a line of this kind may be written now. `Some(n)` says yes, and
    /// that `n` lines of the kind were swallowed since the last one written;
    /// `None` swallows this one.
    pub fn admit(&self, key: &str) -> Option<u64> {
        self.admit_at(key, Instant::now())
    }

    fn admit_at(&self, key: &str, now: Instant) -> Option<u64> {
        let mut seen = self.seen.lock().unwrap();
        match seen
            .iter_mut()
            .find(|(kind, _)| kind == key)
            .map(|(_, entry)| entry)
        {
            None => {
                seen.push((
                    key.to_string(),
                    Seen {
                        last_said: now,
                        suppressed: 0,
                    },
                ));
                Some(0)
            }
            Some(entry) if now.duration_since(entry.last_said) >= self.window => {
                let suppressed = std::mem::take(&mut entry.suppressed);
                entry.last_said = now;
                Some(suppressed)
            }
            Some(entry) => {
                entry.suppressed += 1;
                None
            }
        }
    }
}

/// The kind of a message for throttling: the message with every run of digits
/// replaced by one `N`, so a transaction id, a port or a byte count does not
/// make each repetition its own kind.
pub fn key_of(message: &str) -> String {
    let mut key = String::with_capacity(message.len());
    let mut in_digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                key.push('N');
                in_digits = true;
            }
        } else {
            in_digits = false;
            key.push(c);
        }
    }
    key
}

/// How a throttled line says what it stands for.
pub fn suppressed_suffix(suppressed: u64, window: Duration) -> String {
    if suppressed == 0 {
        String::new()
    } else {
        format!(" (+{suppressed} alike in the last {}s)", window.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_utc_to_the_millisecond() {
        let stamp = stamp();
        assert_eq!(stamp.len(), "2026-09-24T02:10:11.123Z".len(), "{stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn the_first_of_a_kind_is_said_and_the_repeats_are_counted() {
        let throttle = Throttle::new(Duration::from_secs(30));
        let start = Instant::now();
        assert_eq!(throttle.admit_at("bind failed N", start), Some(0));
        assert_eq!(
            throttle.admit_at("bind failed N", start + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            throttle.admit_at("bind failed N", start + Duration::from_secs(2)),
            None
        );
        assert_eq!(
            throttle.admit_at("other", start + Duration::from_secs(2)),
            Some(0)
        );
        assert_eq!(
            throttle.admit_at("bind failed N", start + Duration::from_secs(31)),
            Some(2)
        );
        assert_eq!(
            throttle.admit_at("bind failed N", start + Duration::from_secs(32)),
            None
        );
    }

    #[test]
    fn a_kind_ignores_the_numbers_in_a_message() {
        assert_eq!(
            key_of("TURN transaction timed out: TransactionId([148, 7, 67])"),
            "TURN transaction timed out: TransactionId([N, N, N])"
        );
        assert_eq!(key_of("bind() failed: error 400"), "bind() failed: error N");
    }

    #[test]
    fn the_suffix_names_what_was_swallowed() {
        assert_eq!(suppressed_suffix(0, Duration::from_secs(30)), "");
        assert_eq!(
            suppressed_suffix(41, Duration::from_secs(30)),
            " (+41 alike in the last 30s)"
        );
    }
}
