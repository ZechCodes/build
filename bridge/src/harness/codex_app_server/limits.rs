use std::time::Duration;

/// Every bound one Codex app-server session enforces, constructed once per session
/// and handed to each component as the group that component owns.
#[derive(Debug, Clone, Copy)]
pub struct AppServerLimits {
    pub inbound_frame_bytes: usize,
    pub outbound_frame_bytes: usize,
    pub stderr_line_bytes: usize,
    pub stderr_total_bytes: usize,
    pub pending_requests: usize,
    pub queued_turns: usize,
    pub queued_turn_bytes: usize,
    pub open_items: usize,
    pub open_item_bytes: usize,
    pub completed_items: usize,
    pub completed_item_bytes: usize,
    pub reconciliation: Duration,
}

/// The bounds the JSONL connection enforces on framing and request correlation.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    pub inbound_frame_bytes: usize,
    pub outbound_frame_bytes: usize,
    pub pending_requests: usize,
}

/// The bounds the child process enforces on retained stderr.
#[derive(Debug, Clone, Copy)]
pub struct ProcessLimits {
    pub stderr_line_bytes: usize,
    pub stderr_total_bytes: usize,
}

/// The bounds the session state machine enforces on queued input and reconciliation.
#[derive(Debug, Clone, Copy)]
pub struct StateLimits {
    pub queued_turns: usize,
    pub queued_turn_bytes: usize,
    pub reconciliation: Duration,
}

/// The bounds the activity translator enforces on open and completed items.
#[derive(Debug, Clone, Copy)]
pub struct TranslatorLimits {
    pub open_items: usize,
    pub open_item_bytes: usize,
    pub completed_items: usize,
    pub completed_item_bytes: usize,
}

impl AppServerLimits {
    pub fn connection(&self) -> ConnectionLimits {
        ConnectionLimits {
            inbound_frame_bytes: self.inbound_frame_bytes,
            outbound_frame_bytes: self.outbound_frame_bytes,
            pending_requests: self.pending_requests,
        }
    }

    pub fn process(&self) -> ProcessLimits {
        ProcessLimits {
            stderr_line_bytes: self.stderr_line_bytes,
            stderr_total_bytes: self.stderr_total_bytes,
        }
    }

    pub fn state(&self) -> StateLimits {
        StateLimits {
            queued_turns: self.queued_turns,
            queued_turn_bytes: self.queued_turn_bytes,
            reconciliation: self.reconciliation,
        }
    }

    pub fn translator(&self) -> TranslatorLimits {
        TranslatorLimits {
            open_items: self.open_items,
            open_item_bytes: self.open_item_bytes,
            completed_items: self.completed_items,
            completed_item_bytes: self.completed_item_bytes,
        }
    }
}

impl Default for AppServerLimits {
    fn default() -> Self {
        AppServerLimits {
            inbound_frame_bytes: 1024 * 1024,
            outbound_frame_bytes: 1024 * 1024,
            stderr_line_bytes: 16 * 1024,
            stderr_total_bytes: 32 * 1024,
            pending_requests: 64,
            queued_turns: 16,
            queued_turn_bytes: 256 * 1024,
            open_items: 256,
            open_item_bytes: 128 * 1024,
            completed_items: 256,
            completed_item_bytes: 128 * 1024,
            reconciliation: Duration::from_secs(5),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::connection::AppServerConnection;
    use super::super::process::StderrTail;
    use super::super::state::{CodexSessionState, SessionEvent};
    use super::super::translator::CodexActivityTranslator;
    use super::*;

    #[test]
    fn each_component_reads_only_the_bounds_it_enforces() {
        let limits = AppServerLimits::default();

        let connection = limits.connection();
        assert_eq!(connection.inbound_frame_bytes, 1024 * 1024);
        assert_eq!(connection.outbound_frame_bytes, 1024 * 1024);
        assert_eq!(connection.pending_requests, 64);

        let process = limits.process();
        assert_eq!(process.stderr_line_bytes, 16 * 1024);
        assert_eq!(process.stderr_total_bytes, 32 * 1024);

        let state = limits.state();
        assert_eq!(state.queued_turns, 16);
        assert_eq!(state.queued_turn_bytes, 256 * 1024);
        assert_eq!(state.reconciliation, Duration::from_secs(5));

        let translator = limits.translator();
        assert_eq!(translator.open_items, 256);
        assert_eq!(translator.open_item_bytes, 128 * 1024);
        assert_eq!(translator.completed_items, 256);
        assert_eq!(translator.completed_item_bytes, 128 * 1024);
    }

    #[test]
    fn one_limits_value_is_copied_into_every_component() {
        let limits = AppServerLimits::default();

        let connection = AppServerConnection::memory(limits.connection());
        let stderr_tail = StderrTail::new(limits.process());
        let translator = CodexActivityTranslator::new(limits.translator());
        let transition = CodexSessionState::new(
            PathBuf::from("/tmp/worktree"),
            Some("gpt-5.6-sol".to_string()),
            None,
            None,
        )
        .transition(SessionEvent::Start, Duration::ZERO, limits.state())
        .unwrap();

        assert_eq!(connection.pending_count(), 0);
        assert!(stderr_tail.epitaph().is_none());
        assert_eq!(translator.open_item_count(), 0);
        assert!(!transition.effects.is_empty());
        assert_eq!(limits.inbound_frame_bytes, 1024 * 1024);
    }
}
