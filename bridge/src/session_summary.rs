//! Session boundaries, shared by live writes and startup replay: the inbox's
//! per-owner message sessions, and the user's own device-wide session.

use serde::{Deserialize, Serialize};

pub const SESSION_GAP_MS: i64 = 12 * 60 * 60 * 1000;

/// The silence that ends the user's session: six hours without the user
/// doing anything. Shorter than the inbox gap on purpose, so an evening away
/// ends a working day while a long lunch does not.
pub const USER_SESSION_GAP_MS: i64 = 6 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_started_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
}

impl SessionSummary {
    pub fn updated(self, ts: i64) -> Self {
        self.updated_after_gap(ts, SESSION_GAP_MS)
    }

    /// [`Self::updated`] with the silence that ends a session named by the
    /// caller.
    pub fn updated_after_gap(mut self, ts: i64, gap_ms: i64) -> Self {
        let (Some(start), Some(last)) = (self.session_started_ms, self.last_activity_ms) else {
            self.session_started_ms = Some(ts);
            self.last_activity_ms = Some(ts);
            return self;
        };
        if ts >= last {
            if ts - last >= gap_ms {
                self.session_started_ms = Some(ts);
            }
            self.last_activity_ms = Some(ts);
        } else if ts < start && start - ts < gap_ms {
            // A late message may bridge to an older session we no longer hold.
            // Startup replay in timestamp order reconstructs the exact boundary.
            self.session_started_ms = Some(ts);
        }
        self
    }
}

/// The user's session, device-wide, and where the one before it ended.
///
/// Only the user's own actions update it; an agent working overnight never
/// starts one. `previous_session_ended_ms` is the last thing the user did
/// before the most recent silence of [`USER_SESSION_GAP_MS`] or more, which is
/// what "since you left" is measured from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSession {
    pub session_started_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
    pub previous_session_ended_ms: Option<i64>,
}

impl UserSession {
    pub fn updated(self, ts: i64) -> Self {
        let before = SessionSummary {
            session_started_ms: self.session_started_ms,
            last_activity_ms: self.last_activity_ms,
        };
        let after = before.updated_after_gap(ts, USER_SESSION_GAP_MS);
        let mut previous = self.previous_session_ended_ms;
        match (before.session_started_ms, before.last_activity_ms) {
            // A new session: what ended the old one is its last activity.
            (Some(_), Some(last)) if ts >= last && ts - last >= USER_SESSION_GAP_MS => {
                previous = Some(last);
            }
            // A late action from before the current session that does not
            // bridge to it belongs to the session it ended.
            (Some(start), Some(_))
                if ts < start
                    && start - ts >= USER_SESSION_GAP_MS
                    && previous.is_none_or(|ended| ts > ended) =>
            {
                previous = Some(ts);
            }
            _ => {}
        }
        UserSession {
            session_started_ms: after.session_started_ms,
            last_activity_ms: after.last_activity_ms,
            previous_session_ended_ms: previous,
        }
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

    const HOUR: i64 = 60 * 60 * 1000;

    #[test]
    fn user_session_starts_after_six_hours_and_remembers_where_the_last_ended() {
        let mut user = UserSession::default().updated(0);
        assert_eq!(user.previous_session_ended_ms, None);
        user = user.updated(5 * HOUR).updated(9 * HOUR);
        assert_eq!(user.session_started_ms, Some(0));
        user = user.updated(24 * HOUR);
        assert_eq!(user.session_started_ms, Some(24 * HOUR));
        assert_eq!(user.previous_session_ended_ms, Some(9 * HOUR));
        user = user.updated(25 * HOUR);
        assert_eq!(user.previous_session_ended_ms, Some(9 * HOUR));
    }

    #[test]
    fn user_gap_of_exactly_six_hours_splits_and_just_under_does_not() {
        let user = UserSession::default()
            .updated(0)
            .updated(USER_SESSION_GAP_MS - 1);
        assert_eq!(user.session_started_ms, Some(0));
        assert_eq!(user.previous_session_ended_ms, None);
        let user = user.updated(2 * USER_SESSION_GAP_MS - 1);
        assert_eq!(user.session_started_ms, Some(2 * USER_SESSION_GAP_MS - 1));
        assert_eq!(
            user.previous_session_ended_ms,
            Some(USER_SESSION_GAP_MS - 1)
        );
    }

    #[test]
    fn late_user_action_before_the_session_moves_where_the_last_one_ended() {
        let user = UserSession::default()
            .updated(0)
            .updated(20 * HOUR)
            .updated(HOUR);
        assert_eq!(user.session_started_ms, Some(20 * HOUR));
        assert_eq!(user.previous_session_ended_ms, Some(HOUR));
    }
}
