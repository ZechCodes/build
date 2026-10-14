//! Build-owned protocol causes contain only allowlisted enum values.

use serde_json::{json, Value};

use super::connection::ConnectionError;
use super::state::SessionEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtocolFailure {
    StateTransition {
        event: SessionEventKind,
        phase: DiagnosticPhase,
    },
    Connection(ConnectionFailureKind),
    EventHandling,
}

impl ProtocolFailure {
    pub(super) fn state_transition(event: &SessionEvent, phase: DiagnosticPhase) -> Self {
        Self::StateTransition {
            event: SessionEventKind::from_event(event),
            phase,
        }
    }

    pub(super) fn connection(error: &ConnectionError) -> Self {
        Self::Connection(ConnectionFailureKind::from_error(error))
    }

    /// This trusted text is assembled without any provider message or payload.
    pub(super) fn summary(&self) -> String {
        match self {
            Self::StateTransition { event, phase } => {
                let verb = if event.is_notification() {
                    "unexpected"
                } else {
                    "rejected"
                };
                format!(
                    "protocol_violation: {verb} {} in phase {}",
                    event.protocol_name(),
                    phase.as_str()
                )
            }
            Self::Connection(kind) => format!("protocol_violation: {}", kind.summary()),
            Self::EventHandling => {
                "protocol_violation: Codex app-server event handling failed".into()
            }
        }
    }

    pub(super) fn lifecycle_value(&self) -> Value {
        match self {
            Self::StateTransition { event, phase } => json!({
                "kind": "state_transition",
                "event": event.protocol_name(),
                "phase": phase.as_str(),
            }),
            Self::Connection(kind) => json!({
                "kind": "connection",
                "error_kind": kind.as_str(),
            }),
            Self::EventHandling => json!({"kind": "event_handling"}),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionEventKind {
    Start,
    SendTurn,
    Interrupt,
    Connection,
    ThreadStarted,
    TurnStarted,
    ItemObserved,
    TurnCompleted,
    ErrorNotification,
    VersionEvidence,
    CheckTimeouts,
    FailTurn,
    FailSession,
    Eof,
}

impl SessionEventKind {
    pub(super) fn from_event(event: &SessionEvent) -> Self {
        match event {
            SessionEvent::Start => Self::Start,
            SessionEvent::SendTurn(_) | SessionEvent::SendChosenTurn(_) => Self::SendTurn,
            SessionEvent::Interrupt => Self::Interrupt,
            SessionEvent::Connection(_) => Self::Connection,
            SessionEvent::ThreadStarted(_) => Self::ThreadStarted,
            SessionEvent::TurnStarted(_) => Self::TurnStarted,
            SessionEvent::ItemObserved(_) => Self::ItemObserved,
            SessionEvent::ObservedCompletion(_) => Self::TurnCompleted,
            SessionEvent::ObservedError(_) => Self::ErrorNotification,
            SessionEvent::VersionEvidence(_) => Self::VersionEvidence,
            SessionEvent::CheckTimeouts => Self::CheckTimeouts,
            SessionEvent::FailTurn(_) => Self::FailTurn,
            SessionEvent::FailSession(_) => Self::FailSession,
            SessionEvent::Eof => Self::Eof,
        }
    }

    pub(super) fn lifecycle_name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::SendTurn => "send_turn",
            Self::Interrupt => "interrupt",
            Self::Connection => "connection",
            Self::ThreadStarted => "thread_started",
            Self::TurnStarted => "turn_started",
            Self::ItemObserved => "item_observed",
            Self::TurnCompleted => "turn_completed",
            Self::ErrorNotification => "error_notification",
            Self::VersionEvidence => "version_evidence",
            Self::CheckTimeouts => "check_timeouts",
            Self::FailTurn => "fail_turn",
            Self::FailSession => "fail_session",
            Self::Eof => "eof",
        }
    }

    fn protocol_name(self) -> &'static str {
        match self {
            Self::ThreadStarted => "thread/started",
            Self::TurnStarted => "turn/started",
            Self::ItemObserved => "item notification",
            Self::TurnCompleted => "turn/completed",
            Self::ErrorNotification => "error notification",
            Self::Connection => "JSON-RPC response",
            _ => self.lifecycle_name(),
        }
    }

    fn is_notification(self) -> bool {
        matches!(
            self,
            Self::ThreadStarted
                | Self::TurnStarted
                | Self::ItemObserved
                | Self::TurnCompleted
                | Self::ErrorNotification
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiagnosticPhase {
    Starting,
    Initializing,
    AwaitingVersion,
    OpeningThread,
    Waiting,
    StartingTurn,
    Working,
    Ending,
    Ended,
}

impl DiagnosticPhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Initializing => "initializing",
            Self::AwaitingVersion => "awaiting_version",
            Self::OpeningThread => "opening_thread",
            Self::Waiting => "waiting",
            Self::StartingTurn => "starting_turn",
            Self::Working => "working",
            Self::Ending => "ending",
            Self::Ended => "ended",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionFailureKind {
    Closed,
    RequestIdExhausted,
    PendingLimit,
    FrameTooLarge,
    UnterminatedFrame,
    InvalidUtf8,
    InvalidJson,
    Protocol,
    Io,
}

impl ConnectionFailureKind {
    pub(super) fn from_error(error: &ConnectionError) -> Self {
        match error {
            ConnectionError::Closed => Self::Closed,
            ConnectionError::RequestIdExhausted => Self::RequestIdExhausted,
            ConnectionError::PendingLimit(_) => Self::PendingLimit,
            ConnectionError::FrameTooLarge { .. } => Self::FrameTooLarge,
            ConnectionError::UnterminatedFrame => Self::UnterminatedFrame,
            ConnectionError::InvalidUtf8(_) => Self::InvalidUtf8,
            ConnectionError::InvalidJson(_) => Self::InvalidJson,
            ConnectionError::Protocol(_) => Self::Protocol,
            ConnectionError::Io(_) => Self::Io,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::RequestIdExhausted => "request_id_exhausted",
            Self::PendingLimit => "pending_limit",
            Self::FrameTooLarge => "frame_too_large",
            Self::UnterminatedFrame => "unterminated_frame",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::InvalidJson => "invalid_json",
            Self::Protocol => "protocol",
            Self::Io => "io",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Closed => "Codex app-server connection closed",
            Self::RequestIdExhausted => "Codex app-server request ids exhausted",
            Self::PendingLimit => "Codex app-server pending request limit exceeded",
            Self::FrameTooLarge => "Codex app-server frame exceeded its size limit",
            Self::UnterminatedFrame => "Codex app-server sent an unterminated frame",
            Self::InvalidUtf8 => "Codex app-server sent invalid UTF-8",
            Self::InvalidJson => "Codex app-server sent invalid JSON",
            Self::Protocol => "Codex app-server sent an invalid protocol message",
            Self::Io => "Codex app-server I/O failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_cause_keeps_the_api_and_phase_without_the_event_payload() {
        let failure = ProtocolFailure::state_transition(
            &SessionEvent::TurnStarted("private-turn-value".into()),
            DiagnosticPhase::Working,
        );
        assert_eq!(
            failure.summary(),
            "protocol_violation: unexpected turn/started in phase working"
        );
        assert_eq!(
            failure.lifecycle_value(),
            json!({
                "kind":"state_transition", "event":"turn/started", "phase":"working"
            })
        );
        let rejected = ProtocolFailure::state_transition(
            &SessionEvent::VersionEvidence(Err("private-version-output".into())),
            DiagnosticPhase::AwaitingVersion,
        );
        assert_eq!(
            rejected.summary(),
            "protocol_violation: rejected version_evidence in phase awaiting_version"
        );
        assert!(!rejected.lifecycle_value().to_string().contains("private"));
    }

    #[test]
    fn typed_connection_causes_never_copy_error_payloads() {
        let invalid_json =
            serde_json::from_str::<Value>("not-json private-response-value").unwrap_err();
        let bytes = std::iter::once(u8::MAX).collect::<Vec<_>>();
        let invalid_utf8 = std::str::from_utf8(&bytes).unwrap_err();
        for (error, expected) in [
            (ConnectionError::Closed, "closed"),
            (ConnectionError::RequestIdExhausted, "request_id_exhausted"),
            (ConnectionError::PendingLimit(17), "pending_limit"),
            (
                ConnectionError::FrameTooLarge {
                    limit: 10,
                    observed_at_least: 20,
                },
                "frame_too_large",
            ),
            (ConnectionError::UnterminatedFrame, "unterminated_frame"),
            (ConnectionError::InvalidUtf8(invalid_utf8), "invalid_utf8"),
            (ConnectionError::InvalidJson(invalid_json), "invalid_json"),
            (
                ConnectionError::Protocol("private-protocol-value".into()),
                "protocol",
            ),
            (
                ConnectionError::Io(std::io::Error::other("private-io-value")),
                "io",
            ),
        ] {
            let failure = ProtocolFailure::connection(&error);
            assert_eq!(
                failure.lifecycle_value(),
                json!({"kind":"connection", "error_kind":expected})
            );
            assert!(!failure.summary().contains("private"));
            assert!(!failure.lifecycle_value().to_string().contains("private"));
        }
        assert!(
            ProtocolFailure::Connection(ConnectionFailureKind::InvalidJson)
                .summary()
                .contains("invalid JSON")
        );
    }
}
