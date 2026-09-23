//! Inbox session boundaries, shared by live message writes and startup replay.

use serde::{Deserialize, Serialize};

pub const SESSION_GAP_MS: i64 = 12 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_started_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
}

impl SessionSummary {
    pub fn updated(mut self, ts: i64) -> Self {
        let (Some(start), Some(last)) = (self.session_started_ms, self.last_activity_ms) else {
            self.session_started_ms = Some(ts);
            self.last_activity_ms = Some(ts);
            return self;
        };
        if ts >= last {
            if ts - last >= SESSION_GAP_MS {
                self.session_started_ms = Some(ts);
            }
            self.last_activity_ms = Some(ts);
        } else if ts < start && start - ts < SESSION_GAP_MS {
            // A late message may bridge to an older session we no longer hold.
            // Startup replay in timestamp order reconstructs the exact boundary.
            self.session_started_ms = Some(ts);
        }
        self
    }
}

pub fn message_millis(at: &str) -> Option<i64> {
    let instant =
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(instant.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternating_conversations_keep_one_pooled_session_past_nine_spans() {
        let hour = 60 * 60 * 1000;
        let mut summary = SessionSummary::default();
        for n in 0..=25 {
            summary = summary.updated(n * 8 * hour);
        }
        assert_eq!(summary.session_started_ms, Some(0));
        assert_eq!(summary.last_activity_ms, Some(200 * hour));
    }

    #[test]
    fn exact_gap_splits_and_just_under_gap_does_not() {
        let mut summary = SessionSummary::default();
        summary = summary.updated(0);
        summary = summary.updated(SESSION_GAP_MS - 1);
        assert_eq!(summary.session_started_ms, Some(0));
        summary = summary.updated(2 * SESSION_GAP_MS - 1);
        assert_eq!(summary.session_started_ms, Some(2 * SESSION_GAP_MS - 1));
    }

    #[test]
    fn late_messages_follow_the_documented_boundary_rule() {
        let mut summary = SessionSummary::default();
        summary = summary.updated(2 * SESSION_GAP_MS);
        summary = summary.updated(2 * SESSION_GAP_MS + 100);
        summary = summary.updated(2 * SESSION_GAP_MS + 50);
        assert_eq!(summary.session_started_ms, Some(2 * SESSION_GAP_MS));
        summary = summary.updated(SESSION_GAP_MS + 1);
        assert_eq!(summary.session_started_ms, Some(SESSION_GAP_MS + 1));
        summary = summary.updated(0);
        assert_eq!(summary.session_started_ms, Some(SESSION_GAP_MS + 1));
    }

    #[test]
    fn timestamp_order_rebuild_matches_chronological_live_updates() {
        let hour = 60 * 60 * 1000;
        let chronological = [0, 8 * hour, 16 * hour, 40 * hour, 48 * hour];
        let mut live = SessionSummary::default();
        for ts in chronological {
            live = live.updated(ts);
        }
        let mut stored = [40 * hour, 8 * hour, 48 * hour, 0, 16 * hour];
        stored.sort();
        let rebuilt = stored
            .into_iter()
            .fold(SessionSummary::default(), SessionSummary::updated);
        assert_eq!(rebuilt, live);
        assert_eq!(rebuilt.session_started_ms, Some(40 * hour));
    }
}
